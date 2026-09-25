use pinocchio::{
    cpi::{Seed, Signer},
    error::ProgramError,
    AccountView, ProgramResult,
};

use pinocchio_associated_token_account::instructions::CreateIdempotent;
use pinocchio_pubkey::derive_address;
use pinocchio_token::{
    instructions::{CloseAccount, Transfer},
    state::Account as TokenAccount,
};

use crate::state::Escrow;

pub fn process_take_instruction(accounts: &mut [AccountView]) -> ProgramResult {
    let [taker, maker, mint_a, mint_b, escrow_account, vault, taker_ata_a, taker_ata_b, maker_ata_b, system_program, token_program, _associated_token_program] =
        accounts
    else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    if !taker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::IllegalOwner);
    }

    let (escrow_maker, escrow_mint_a, escrow_mint_b, amount_to_receive, bump) = {
        let escrow_state = Escrow::load_mut(escrow_account)?;

        (
            escrow_state.maker(),
            escrow_state.mint_a(),
            escrow_state.mint_b(),
            escrow_state.amount_to_receive(),
            escrow_state.bump,
        )
    };

    if escrow_maker != *maker.address()
        || escrow_mint_a != *mint_a.address()
        || escrow_mint_b != *mint_b.address()
    {
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

    if vault_amount == 0 {
        return Err(ProgramError::InsufficientFunds);
    }

    {
        let taker_b_state = TokenAccount::from_account_view(taker_ata_b)?;

        if *taker_b_state.owner() != *taker.address() || *taker_b_state.mint() != *mint_b.address()
        {
            return Err(ProgramError::InvalidAccountData);
        }

        if taker_b_state.amount() < amount_to_receive {
            return Err(ProgramError::InsufficientFunds);
        }
    }

    CreateIdempotent {
        funding_account: taker,
        account: taker_ata_a,
        wallet: taker,
        mint: mint_a,
        system_program,
        token_program,
    }
    .invoke()?;

    CreateIdempotent {
        funding_account: taker,
        account: maker_ata_b,
        wallet: maker,
        mint: mint_b,
        system_program,
        token_program,
    }
    .invoke()?;

    Transfer {
        from: taker_ata_b,
        to: maker_ata_b,
        authority: taker,
        multisig_signers: &[] as &[&AccountView],
        amount: amount_to_receive,
    }
    .invoke()?;

    let bump_bytes = [bump];

    let seed = [
        Seed::from(b"escrow"),
        Seed::from(maker.address().as_array()),
        Seed::from(&bump_bytes),
    ];

    let signer = Signer::from(&seed);

    Transfer {
        from: vault,
        to: taker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }
    .invoke_signed(std::slice::from_ref(&signer))?;

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
