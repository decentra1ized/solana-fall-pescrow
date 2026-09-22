use pinocchio::{
    AccountView, ProgramResult, cpi::{Seed, Signer}, error::ProgramError
};
use pinocchio_pubkey::derive_address;

use crate::instructions::shared::{check_token_account, drain_and_close};
use crate::state::Escrow;

/// Accounts:
///   0 maker (signer, writable), 1 mint_a, 2 escrow_account (writable),
///   3 vault (writable), 4 maker_ata_a (writable), 5 token_program
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
        _token_program@ ..
    ] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    // Without this, anyone can cancel someone else's escrow.
    if !maker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::IllegalOwner);
    }

    // is_signer says who sent the transaction; the stored maker says it is their escrow.
    let bump = {
        let escrow_state = Escrow::load_mut(escrow_account)?;
        if escrow_state.maker() != *maker.address() || escrow_state.mint_a() != *mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        escrow_state.bump
    };

    let bump_bytes = [bump];
    let seed = [b"escrow".as_ref(), maker.address().as_ref(), &bump_bytes];
    if derive_address(&seed, None, &crate::ID.to_bytes()) != *escrow_account.address().as_array() {
        return Err(ProgramError::InvalidSeeds);
    }

    let vault_amount = check_token_account(vault, escrow_account.address(), mint_a.address())?;
    check_token_account(maker_ata_a, maker.address(), mint_a.address())?;

    // Copy the address out so the Signer does not keep borrowing `maker`, which the
    // rent refund below needs mutably.
    let maker_address = *maker.address().as_array();
    let seed = [Seed::from(b"escrow"), Seed::from(&maker_address), Seed::from(&bump_bytes)];
    let seeds = Signer::from(&seed);

    drain_and_close(vault, maker_ata_a, escrow_account, maker, vault_amount, seeds)
}
