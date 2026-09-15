//! Cancel: the maker reclaims their deposit before anyone takes the deal.
//!
//! Account order is the API — there is no IDL. Keep the test in sync with this:
//!
//!   0  maker            signer, writable, must equal escrow.maker()
//!   1  mint_a           must equal escrow.mint_a()
//!   2  escrow_account   writable, PDA, closed here
//!   3  vault            writable, ATA(escrow, mint_a), closed here
//!   4  maker_ata_a      writable, destination for the returned A
//!   5  token_program

use pinocchio::{
    AccountView, ProgramResult,
    cpi::{Seed, Signer},
    error::ProgramError,
};

use crate::state::Escrow;
use super::shared::settle;

pub fn process_cancel_instruction(accounts: &mut [AccountView]) -> ProgramResult {
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

    // The authorization check. Without it anyone can drain any escrow; without
    // the stored-maker check below they can drain it to an account of theirs.
    if !maker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::IllegalOwner);
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

    // is_signer proves who sent the transaction; this proves it is the right
    // escrow for that signer.
    let expected = pinocchio::Address::from(pinocchio_pubkey::derive_address(
        &[b"escrow", maker.address().as_ref(), &[bump]],
        None,
        &crate::ID.to_bytes(),
    ));
    if expected != *escrow_account.address() {
        return Err(ProgramError::InvalidSeeds);
    }

    let vault_amount = {
        let vault_state = pinocchio_token::state::Account::from_account_view(vault)?;
        if vault_state.owner() != escrow_account.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if vault_state.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        vault_state.amount()
    };

    {
        let maker_ata_state = pinocchio_token::state::Account::from_account_view(maker_ata_a)?;
        if maker_ata_state.owner() != maker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if maker_ata_state.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    let maker_key = *maker.address().as_array();
    let bump_bytes = [bump];
    let seed = [
        Seed::from(b"escrow"),
        Seed::from(&maker_key),
        Seed::from(&bump_bytes),
    ];
    let signer = Signer::from(&seed);

    settle(vault, maker_ata_a, escrow_account, maker, vault_amount, &signer)
}
