use pinocchio::{
    cpi::{Seed, Signer},
    error::ProgramError,
    AccountView, ProgramResult,
};
use pinocchio_pubkey::derive_address;
use pinocchio_token::{
    instructions::{CloseAccount, Transfer},
    state::Account as TokenAccount,
};

use crate::state::Escrow;

pub fn process_cancel_instruction(accounts: &mut [AccountView]) -> ProgramResult {
    let [maker, mint_a, escrow_account, vault, maker_ata_a, _token_program] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    if !maker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::IllegalOwner);
    }

    let (escrow_maker, escrow_mint_a, bump) = {
        let escrow_state = Escrow::load_mut(escrow_account)?;

        (
            escrow_state.maker(),
            escrow_state.mint_a(),
            escrow_state.bump,
        )
    };

    if escrow_maker != *maker.address() || escrow_mint_a != *mint_a.address() {
        return Err(ProgramError::InvalidAccountData);
    }

    let escrow_address = derive_address(
        &[b"escrow", maker.address().as_ref(), &[bump]],
        None,
        &crate::ID.to_bytes(),
    );

    if escrow_address != escrow_account.address().to_bytes() {
        return Err(ProgramError::InvalidSeeds);
    }

    let vault_amount = {
        let vault_state = TokenAccount::from_account_view(vault)?;

        if *vault_state.owner() != *escrow_account.address()
            || *vault_state.mint() != *mint_a.address()
        {
            return Err(ProgramError::InvalidAccountData);
        }

        vault_state.amount()
    };

    {
        let maker_ata_state = TokenAccount::from_account_view(maker_ata_a)?;

        if *maker_ata_state.owner() != *maker.address()
            || *maker_ata_state.mint() != *mint_a.address()
        {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    let bump_bytes = [bump];

    let seed = [
        Seed::from(b"escrow"),
        Seed::from(maker.address().as_array()),
        Seed::from(&bump_bytes),
    ];

    let signer = Signer::from(&seed);

    if vault_amount > 0 {
        Transfer {
            from: vault,
            to: maker_ata_a,
            authority: escrow_account,
            multisig_signers: &[] as &[&AccountView],
            amount: vault_amount,
        }
        .invoke_signed(std::slice::from_ref(&signer))?;
    }

    CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }
    .invoke_signed(&[signer])?;

    let escrow_lamports = escrow_account.lamports();
    maker.set_lamports(maker.lamports() + escrow_lamports);
    escrow_account.set_lamports(0);
    escrow_account.close()?;

    Ok(())
}
