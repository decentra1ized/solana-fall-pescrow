use pinocchio::{AccountView, ProgramResult, error::ProgramError};

use crate::{instructions::{derive_escrow_pda, dismiss_escrow}, state::Escrow};

/// Account order:
///   0  maker                     W S   must sign; must equal escrow.maker()
///   1  mint_a                          must equal escrow.mint_a()
///   2  escrow_account            W     PDA, will be closed
///   3  vault                     W     will be closed
///   4  maker_ata_a               W     destination for the returned A
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
        _token_program,
    ] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    if !maker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    let bump = {
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
        escrow.bump
    };

    if derive_escrow_pda(maker, bump) != *escrow_account.address() {
        return Err(ProgramError::InvalidSeeds);
    }

    // The maker's A account must exist, be theirs, and be for mint A.
    {
        let maker_ata_a_state = pinocchio_token::state::Account::from_account_view(maker_ata_a)?;
        if maker_ata_a_state.owner() != maker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if maker_ata_a_state.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    dismiss_escrow(
        escrow_account,
        vault,
        maker_ata_a,
        maker,
        mint_a,
        bump,
    )
}