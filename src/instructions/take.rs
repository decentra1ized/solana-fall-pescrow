use pinocchio::{
    AccountView, ProgramResult, cpi::{Seed, Signer}, error::ProgramError
};
use pinocchio_pubkey::derive_address;

use crate::state::Escrow;

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
        _associated_token_program@ ..
    ] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    {
        let taker_ata_state = pinocchio_token::state::Account::from_account_view(taker_ata_b)?;
        if taker_ata_state.owner() != taker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if taker_ata_state.mint() != mint_b.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    if !taker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::IllegalOwner);
    }

    let (amount_to_receive, bump) = {
        let escrow_state = Escrow::load_mut(escrow_account)?;
        if escrow_state.maker() != *maker.address() { 
            return Err(ProgramError::InvalidAccountData);   
        }
        if escrow_state.mint_a() != *mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        if escrow_state.mint_b() != *mint_b.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        (escrow_state.amount_to_receive(), escrow_state.bump)
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

    pinocchio_token::instructions::Transfer {
        from: taker_ata_b,
        to: maker_ata_b,
        authority: taker,
        multisig_signers: &[] as &[&AccountView],
        amount: amount_to_receive,
    }.invoke()?;

    let bump_bytes = [bump];
    let seed = [Seed::from(b"escrow"), Seed::from(maker.address().as_array()), Seed::from(&bump_bytes)];
    let seeds = Signer::from(&seed);

    pinocchio_token::instructions::Transfer {
        from: vault,
        to: taker_ata_a,
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