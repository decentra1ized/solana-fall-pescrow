use pinocchio::{
    AccountView, ProgramResult, cpi::{Seed, Signer}, error::ProgramError,
};

use crate::instructions::shared::{check_token_account, drain_vault_and_close, verify_escrow_pda};
use crate::state::Escrow;

/// Cancel. Instruction data: just the discriminator `2`.
///
/// Accounts (order is the API):
///   0  maker            writable, signer  MUST sign, must equal escrow.maker()
///   1  mint_a                             must equal escrow.mint_a()
///   2  escrow_account   writable          PDA ["escrow", maker, bump], closed
///   3  vault            writable          ATA(escrow PDA, mint A), closed
///   4  maker_ata_a      writable          destination for the returned A
///   5  token_program
pub fn process_cancel_instruction(
    accounts: &mut [AccountView],
    _data: &[u8],
) -> ProgramResult {
    let [
        maker,
        mint_a,
        escrow_account,
        vault,
        maker_ata_a,
        _token_program @ ..
    ] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    // The critical authorization check.
    if !maker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::InvalidAccountOwner);
    }

    let bump = {
        let escrow = Escrow::load_mut(escrow_account)?;
        if escrow.maker() != *maker.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        if escrow.mint_a() != *mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        escrow.bump
    };

    verify_escrow_pda(escrow_account, maker, bump)?;

    let vault_amount = check_token_account(vault, escrow_account, mint_a)?;
    check_token_account(maker_ata_a, maker, mint_a)?;

    // Copy the maker key so the seeds do not keep `maker` borrowed; the close needs it mutably.
    let maker_key: [u8; 32] = *maker.address().as_array();
    let bump_bytes = [bump];
    let seed = [
        Seed::from(b"escrow"),
        Seed::from(&maker_key),
        Seed::from(&bump_bytes),
    ];
    let signer = Signer::from(&seed);

    drain_vault_and_close(vault, maker_ata_a, escrow_account, maker, vault_amount, signer)
}
