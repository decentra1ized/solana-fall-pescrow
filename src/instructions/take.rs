use pinocchio::{
    AccountView, ProgramResult, error::ProgramError, cpi::{Seed, Signer}
};
use pinocchio_pubkey::derive_address;

use crate::state::Escrow;
use crate::instructions::shared::drain_and_close_vault_and_escrow;

// Accounts expected:
//  0 taker                 W, S
//  1 maker                 W
//  2 mint_a                
//  3 mint_b                
//  4 escrow_account        W, PDA
//  5 vault                 W
//  6 taker_ata_a           W
//  7 taker_ata_b           W
//  8 maker_ata_b           W
//  9 system_program        
// 10 token_program         
// 11 associated_token_program

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

    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::InvalidAccountData);
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

    pinocchio_associated_token_account::instructions::CreateIdempotent {
        funding_account: taker,
        account: taker_ata_a,
        wallet: taker,
        mint: mint_a,
        token_program: token_program,
        system_program: system_program,
    }.invoke()?;

    pinocchio_associated_token_account::instructions::CreateIdempotent {
        funding_account: taker,
        account: maker_ata_b,
        wallet: maker,
        mint: mint_b,
        token_program: token_program,
        system_program: system_program,
    }.invoke()?;

    {
        let taker_ata_b_state = pinocchio_token::state::Account::from_account_view(taker_ata_b)?;
        if taker_ata_b_state.owner() != taker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if taker_ata_b_state.mint() != mint_b.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    pinocchio_token::instructions::Transfer {
        from: taker_ata_b,
        to: maker_ata_b,
        authority: taker,
        multisig_signers: &[] as &[&AccountView],
        amount: amount_to_receive,
    }.invoke()?;

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
        taker_ata_a,
        vault_amount,
        signer,
    )?;

    Ok(())
}
