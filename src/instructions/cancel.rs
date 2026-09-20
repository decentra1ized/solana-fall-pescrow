use pinocchio::{
    cpi::{Seed, Signer},
    error::ProgramError,
    AccountView, ProgramResult,
};
use pinocchio_pubkey::derive_address;
use pinocchio_token::instructions::Transfer;

use crate::state::Escrow;

pub fn process_cancel_instruction(accounts: &mut [AccountView], _data: &[u8]) -> ProgramResult {
    let [maker, mint_a, escrow_account, vault, maker_ata_a, _token_program] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    if !maker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }
    let bump = {
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
        escrow.bump
    };
    {
        let maker_ata_state = pinocchio_token::state::Account::from_account_view(maker_ata_a)?;
        if maker_ata_state.owner() != maker.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        if maker_ata_state.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }
    let vault_amount = {
        let vault_account = pinocchio_token::state::Account::from_account_view(vault)?;
        if vault_account.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        };
        if vault_account.owner() != escrow_account.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        vault_account.amount()
    };
    let bump_bytes = [bump];
    let escrow_pda = derive_address(
        &[b"escrow", maker.address().as_ref(), &bump_bytes],
        None,
        &crate::ID.to_bytes(),
    );
    if escrow_pda != *escrow_account.address().as_array() {
        return Err(ProgramError::InvalidAccountData);
    };
    let seeds = [
        Seed::from(b"escrow"),
        Seed::from(maker.address().as_array()),
        Seed::from(&bump_bytes),
    ];

    let signer_seeds = Signer::from(&seeds);

    Transfer {
        from: vault,
        to: maker_ata_a,
        authority: escrow_account,
        amount: vault_amount,
        multisig_signers: &[] as &[&AccountView],
    }
    .invoke_signed(&[signer_seeds.clone()])?;

    pinocchio_token::instructions::CloseAccount {
        account: vault,
        authority: escrow_account,
        destination: maker,
        multisig_signers: &[] as &[AccountView],
    }
    .invoke_signed(&[signer_seeds])?;
    maker.set_lamports(maker.lamports() + escrow_account.lamports());
    escrow_account.set_lamports(0);
    escrow_account.close()?;
    Ok(())
}
