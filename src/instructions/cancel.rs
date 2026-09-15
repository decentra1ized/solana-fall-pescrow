use pinocchio::{
    AccountView, ProgramResult, error::ProgramError, cpi::{Seed, Signer}
};
use pinocchio_pubkey::derive_address;

use crate::state::Escrow;
use crate::instructions::shared::drain_and_close_vault_and_escrow;

// Accounts expected:
//  0 maker                 W, S
//  1 mint_a
//  2 escrow_account        W, PDA
//  3 vault                 W
//  4 maker_ata_a           W
//  5 token_program

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

    if !maker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::InvalidAccountData);
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

    let pda = derive_address(
        &[b"escrow", maker.address().as_ref(), &[bump]],
        None,
        &crate::ID.to_bytes()
    );

    if pda != *escrow_account.address().as_ref() {
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
        let maker_ata_a_state = pinocchio_token::state::Account::from_account_view(maker_ata_a)?;
        if maker_ata_a_state.owner() != maker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if maker_ata_a_state.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    let bump_bytes = [bump];
    let maker_bytes = *maker.address().as_array();
    let seed = [
        Seed::from(b"escrow"),
        Seed::from(&maker_bytes),
        Seed::from(&bump_bytes),
    ];
    let signer = Signer::from(&seed);

    drain_and_close_vault_and_escrow(
        maker,
        escrow_account,
        vault,
        maker_ata_a,
        vault_amount,
        signer,
    )?;

    Ok(())
}
