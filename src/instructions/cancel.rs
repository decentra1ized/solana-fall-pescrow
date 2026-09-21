use pinocchio::{
    cpi::{Seed, Signer},
    error::ProgramError,
    AccountView, ProgramResult,
};
use pinocchio_pubkey::derive_address;

use crate::state::Escrow;

// Accounts: maker, mint_a, escrow, vault, maker_ata_a, token_program.
pub fn process_cancel_instruction(accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    let [maker, mint_a, escrow_account, vault, maker_ata_a, token_program] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    if !data.is_empty() {
        return Err(ProgramError::InvalidInstructionData);
    }

    if !maker.is_signer()
        || !escrow_account.owned_by(&crate::ID)
        || *token_program.address() != pinocchio_token::ID
    {
        return Err(ProgramError::InvalidAccountData);
    }

    let expected_maker_ata_a = pinocchio::Address::derive_program_address(
        &[
            maker.address().as_ref(),
            token_program.address().as_ref(),
            mint_a.address().as_ref(),
        ],
        &pinocchio_associated_token_account::ID,
    )
    .ok_or(ProgramError::InvalidSeeds)?
    .0;
    if expected_maker_ata_a != *maker_ata_a.address() {
        return Err(ProgramError::InvalidAccountData);
    }

    let (bump, escrow_maker) = {
        let escrow = Escrow::load_mut(escrow_account)?;
        if escrow.maker() != *maker.address() || escrow.mint_a() != *mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        (escrow.bump, escrow.maker())
    };

    let bump_bytes = [bump];
    let pda_seeds = [
        b"escrow".as_ref(),
        escrow_maker.as_ref(),
        bump_bytes.as_ref(),
    ];
    if pinocchio::Address::from(derive_address(&pda_seeds, None, crate::ID.as_array()))
        != *escrow_account.address()
    {
        return Err(ProgramError::InvalidSeeds);
    }

    let vault_amount = {
        let vault_state = pinocchio_token::state::Account::from_account_view(vault)?;
        if *vault_state.owner() != *escrow_account.address()
            || *vault_state.mint() != *mint_a.address()
        {
            return Err(ProgramError::InvalidAccountData);
        }
        vault_state.amount()
    };

    {
        let maker_ata_state = pinocchio_token::state::Account::from_account_view(maker_ata_a)?;
        if *maker_ata_state.owner() != *maker.address()
            || *maker_ata_state.mint() != *mint_a.address()
        {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    let seed = [
        Seed::from(b"escrow"),
        Seed::from(escrow_maker.as_array()),
        Seed::from(&bump_bytes),
    ];
    let signer = Signer::from(&seed);

    pinocchio_token::instructions::Transfer {
        from: vault,
        to: maker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }
    .invoke_signed(&[signer.clone()])?;

    pinocchio_token::instructions::CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }
    .invoke_signed(&[signer])?;

    maker.set_lamports(maker.lamports() + escrow_account.lamports());
    escrow_account.set_lamports(0);
    escrow_account.close()?;

    Ok(())
}
