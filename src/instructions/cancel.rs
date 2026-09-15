//! Cancel — the maker takes their deposit back before anyone takes the deal.
//!
//! Accounts, in this exact order:
//!
//!   #   account            w  s   notes
//!   0   maker              ✓  ✓   must sign, and must equal escrow.maker()
//!   1   mint_a                     must equal escrow.mint_a()
//!   2   escrow_account     ✓      PDA ["escrow", maker]; closed here
//!   3   vault              ✓      ATA(escrow PDA, mint A); closed here
//!   4   maker_ata_a        ✓      destination for the returned A
//!   5   token_program
//!
//! Instruction data is the discriminator byte alone.

use pinocchio::{AccountView, ProgramResult, error::ProgramError};

use crate::instructions::shared::drain_vault_and_close;
use crate::state::Escrow;

pub fn process_cancel_instruction(accounts: &mut [AccountView], _data: &[u8]) -> ProgramResult {
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

    // Three separate checks, and all three earn their place. is_signer proves
    // who sent the transaction; the stored-maker check proves they are the
    // right person for this escrow; the PDA re-derivation proves it is the
    // right escrow for them. Drop the signer check and anyone can push a
    // maker's deposit back to them uninvited. Drop the maker check too and they
    // can route it to themselves.
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

    let maker_key = *maker.address();
    let expected = pinocchio_pubkey::derive_address(
        &[b"escrow", maker_key.as_ref(), &[bump]],
        None,
        crate::ID.as_array(),
    );
    if expected != *escrow_account.address().as_array() {
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

    drain_vault_and_close(
        escrow_account,
        vault,
        maker_ata_a,
        maker,
        &maker_key,
        bump,
        vault_amount,
    )
}
