use pinocchio::{AccountView, ProgramResult, error::ProgramError};

use crate::{
    instructions::{derive_escrow_pda, dismiss_escrow},
    state::Escrow,
};

/// Account order:
///   0  taker                     W S   pays fees; funds own ATA for A if missing
///   1  maker                     W     receives rent refunds; must equal escrow.maker()
///   2  mint_a                          must equal escrow.mint_a()
///   3  mint_b                          must equal escrow.mint_b()
///   4  escrow_account            W     PDA, owned by this program, will be closed
///   5  vault                     W     ATA(escrow PDA, mint A), will be closed
///   6  taker_ata_a               W     ATA(taker, mint A), destination for A (created if missing)
///   7  taker_ata_b               W     ATA(taker, mint B), source of B
///   8  maker_ata_b               W     ATA(maker, mint B), destination for B (created if missing)
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
        _associated_token_program,
    ] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    if !taker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    // Load the escrow, verify the passed accounts against the stored state,
    // and copy out what we need. The scope ends so the RefMut guard drops.
    let (amount_to_receive, bump) = {
        if !escrow_account.owned_by(&crate::ID) {
            return Err(ProgramError::InvalidAccountData);
        }
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
    };

    // Re-derive the PDA from the stored canonical bump (single hash).
    if derive_escrow_pda(maker, bump) != *escrow_account.address() {
        return Err(ProgramError::InvalidSeeds);
    }

    // Read the vault balance; it is exactly what the taker will receive.
    // The shared tail validates the vault and moves its full balance.

    // The taker's B ATA must exist, be theirs, and be for mint B.
    {
        let taker_ata_b_state = pinocchio_token::state::Account::from_account_view(taker_ata_b)?;
        if taker_ata_b_state.owner() != taker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if taker_ata_b_state.mint() != mint_b.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    // Destinations for A and B may not exist yet; create them idempotently.
    pinocchio_associated_token_account::instructions::CreateIdempotent {
        funding_account: taker,
        account: taker_ata_a,
        wallet: taker,
        mint: mint_a,
        token_program,
        system_program,
    }.invoke()?;

    pinocchio_associated_token_account::instructions::CreateIdempotent {
        funding_account: taker,
        account: maker_ata_b,
        wallet: maker,
        mint: mint_b,
        token_program,
        system_program,
    }.invoke()?;

    // CPI #1: taker pays maker `amount_to_receive` of B. Plain invoke.
    pinocchio_token::instructions::Transfer {
        from: taker_ata_b,
        to: maker_ata_b,
        authority: taker,
        multisig_signers: &[] as &[&AccountView],
        amount: amount_to_receive,
    }.invoke()?;

    // Shared tail: vault pays the taker, then close vault + escrow.
    dismiss_escrow(
        escrow_account,
        vault,
        taker_ata_a,
        maker,
        mint_a,
        bump,
    )
}