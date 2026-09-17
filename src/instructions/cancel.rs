use pinocchio::{
    AccountView, ProgramResult, cpi::{Seed, Signer}, error::ProgramError
};
use pinocchio_pubkey::derive_address;

use crate::state::Escrow;

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

    {
        let maker_ata_state = pinocchio_token::state::Account::from_account_view(maker_ata_a)?;
        if maker_ata_state.owner() != maker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if maker_ata_state.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    if !maker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::IllegalOwner);
    }

    let bump = {
        let escrow_state = Escrow::load_mut(escrow_account)?;
        if escrow_state.maker() != *maker.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        if escrow_state.mint_a() != *mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        escrow_state.bump
    };

    if derive_address(&[b"escrow", maker.address().as_ref(), &[bump]], None, &crate::ID.to_bytes()) 
        != escrow_account.address().as_ref() {
        return Err(ProgramError::InvalidSeeds);
    }

    let vault_amount = {
        let vault_account = pinocchio_token::state::Account::from_account_view(vault)?;
        if vault_account.owner() != escrow_account.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if vault_account.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        vault_account.amount()
    };

    let bump_bytes = [bump];
    let seed = [Seed::from(b"escrow"), Seed::from(maker.address().as_array()), Seed::from(&bump_bytes)];
    let seeds = Signer::from(&seed);

    pinocchio_token::instructions::Transfer {
        from: vault,
        to: maker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }.invoke_signed(&[seeds.clone()])?;

    pinocchio_token::instructions::CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }.invoke_signed(&[seeds.clone()])?;

    maker.set_lamports(maker.lamports() + escrow_account.lamports());
    escrow_account.set_lamports(0);
    escrow_account.close()?;

    Ok(())
}
