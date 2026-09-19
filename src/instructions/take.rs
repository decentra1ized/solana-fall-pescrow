use pinocchio::{
    cpi::{Seed, Signer},
    error::ProgramError,
    AccountView, ProgramResult,
};
use pinocchio_pubkey::derive_address;
use pinocchio_token::instructions::Transfer;

use crate::state::Escrow;

pub fn process_take_instruction(accounts: &mut [AccountView], _data: &[u8]) -> ProgramResult {
    let [taker, maker, mint_a, mint_b, escrow_account, vault, taker_ata_a, taker_ata_b, maker_ata_b, system_program, token_program, _associated_token_program] =
        accounts
    else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    if !taker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }
    let (amount_to_receive, bump) = {
        if !escrow_account.owned_by(&crate::ID) {
            return Err(ProgramError::InvalidAccountData);
        };
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
    let escrow_pda = derive_address(
        &[b"escrow", maker.address().as_ref(), &[bump]],
        None,
        &crate::ID.to_bytes(),
    );
    if escrow_pda != *escrow_account.address().as_array() {
        return Err(ProgramError::InvalidSeeds);
    }
    let vault_amount = {
        let vault_state = pinocchio_token::state::Account::from_account_view(vault)?;
        if vault_state.owner() != escrow_account.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        if vault_state.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        vault_state.amount()
    };

    pinocchio_associated_token_account::instructions::CreateIdempotent {
        funding_account: taker,
        wallet: maker,
        account: maker_ata_b,
        mint: mint_b,
        token_program,
        system_program,
    }
    .invoke()?;

    {
        let taker_ata_b_account = pinocchio_token::state::Account::from_account_view(taker_ata_b)?;
        if taker_ata_b_account.owner() != taker.address() {
            return Err(ProgramError::InvalidAccountData);
        };
        if taker_ata_b_account.mint() != mint_b.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    Transfer {
        from: taker_ata_b,
        to: maker_ata_b,
        amount: amount_to_receive,
        authority: taker,
        multisig_signers: &[] as &[&AccountView],
    }
    .invoke()?;

    pinocchio_associated_token_account::instructions::CreateIdempotent {
        funding_account: taker,
        account: taker_ata_a,
        wallet: taker,
        mint: mint_a,
        token_program,
        system_program,
    }
    .invoke()?;
    let bump_bytes = [bump];
    let seeds = [
        Seed::from(b"escrow"),
        Seed::from(maker.address().as_ref()),
        Seed::from(&bump_bytes),
    ];
    let signer_seeds = Signer::from(&seeds);

    Transfer {
        from: vault,
        to: taker_ata_a,
        authority: escrow_account,
        amount: vault_amount,
        multisig_signers: &[] as &[&AccountView],
    }
    .invoke_signed(&[signer_seeds.clone()])?;

    pinocchio_token::instructions::CloseAccount {
        account: vault,
        authority: escrow_account,
        destination: maker,
        multisig_signers: &[] as &[&AccountView],
    }
    .invoke_signed(&[signer_seeds])?;
    maker.set_lamports(maker.lamports() + escrow_account.lamports());
    escrow_account.set_lamports(0);
    escrow_account.close()?;
    Ok(())
}
