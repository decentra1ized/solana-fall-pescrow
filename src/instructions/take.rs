use pinocchio::{
    AccountView, ProgramResult, cpi::{Seed, Signer}, error::ProgramError,
};

use crate::instructions::shared::{check_token_account, drain_vault_and_close, verify_escrow_pda};
use crate::state::Escrow;

/// Take. Instruction data: just the discriminator `1`.
///
/// Accounts (order is the API):
///   0  taker                     writable, signer  pays fees, funds missing ATAs
///   1  maker                     writable          must equal escrow.maker(), receives rent
///   2  mint_a                                      must equal escrow.mint_a()
///   3  mint_b                                      must equal escrow.mint_b()
///   4  escrow_account            writable          PDA ["escrow", maker, bump], closed
///   5  vault                     writable          ATA(escrow PDA, mint A), closed
///   6  taker_ata_a               writable          ATA(taker, mint A), created if missing
///   7  taker_ata_b               writable          ATA(taker, mint B), source of B
///   8  maker_ata_b               writable          ATA(maker, mint B), created if missing
///   9  system_program
///   10 token_program
///   11 associated_token_program
pub fn process_take_instruction(
    accounts: &mut [AccountView],
    _data: &[u8],
) -> ProgramResult {
    let [
        taker,
        maker,
        mint_a,
        mint_b,
        escrow_account,
        vault,
        taker_ata_a,
        taker_ata_b,
        maker_ata_b,
        system_program,
        token_program,
        _associated_token_program @ ..
    ] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    if !taker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    // Trust the escrow data only once we know this program owns the account.
    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::InvalidAccountOwner);
    }

    let (amount_to_receive, bump) = {
        let escrow = Escrow::load_mut(escrow_account)?;
        if escrow.maker() != *maker.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        if escrow.mint_a() != *mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        if escrow.mint_b() != *mint_b.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        (escrow.amount_to_receive(), escrow.bump)
    }; // RefMut guard dropped here, before any CPI

    verify_escrow_pda(escrow_account, maker, bump)?;

    let vault_amount = check_token_account(vault, escrow_account, mint_a)?;

    pinocchio_associated_token_account::instructions::CreateIdempotent {
        funding_account: taker,
        account: taker_ata_a,
        wallet: taker,
        mint: mint_a,
        system_program,
        token_program,
    }
    .invoke()?;

    pinocchio_associated_token_account::instructions::CreateIdempotent {
        funding_account: taker,
        account: maker_ata_b,
        wallet: maker,
        mint: mint_b,
        system_program,
        token_program,
    }
    .invoke()?;

    check_token_account(taker_ata_b, taker, mint_b)?;

    // CPI #1: taker pays maker. The taker signed the transaction, so plain invoke().
    pinocchio_token::instructions::Transfer {
        from: taker_ata_b,
        to: maker_ata_b,
        authority: taker,
        multisig_signers: &[] as &[&AccountView],
        amount: amount_to_receive,
    }
    .invoke()?;

    // CPIs #2 and #3 are signed by the escrow PDA, using the stored bump.
    // Copy the maker key so the seeds do not keep `maker` borrowed; the close needs it mutably.
    let maker_key: [u8; 32] = *maker.address().as_array();
    let bump_bytes = [bump];
    let seed = [
        Seed::from(b"escrow"),
        Seed::from(&maker_key),
        Seed::from(&bump_bytes),
    ];
    let signer = Signer::from(&seed);

    drain_vault_and_close(vault, taker_ata_a, escrow_account, maker, vault_amount, signer)
}
