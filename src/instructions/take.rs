use pinocchio::{
    cpi::{Seed, Signer},
    error::ProgramError,
    AccountView, Address, ProgramResult,
};
use pinocchio_pubkey::derive_address;

use crate::state::Escrow;

// Accounts: taker, maker, mint_a, mint_b, escrow, vault, taker_ata_a,
// taker_ata_b, maker_ata_b, system_program, token_program, associated_token_program.
pub fn process_take_instruction(accounts: &mut [AccountView], _data: &[u8]) -> ProgramResult {
    let [taker, maker, mint_a, mint_b, escrow_account, vault, taker_ata_a, taker_ata_b, maker_ata_b, system_program, token_program, _associated_token_program] =
        accounts
    else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    if !_data.is_empty() {
        return Err(ProgramError::InvalidInstructionData);
    }

    if !taker.is_signer()
        || !escrow_account.owned_by(&crate::ID)
        || *system_program.address() != pinocchio_system::ID
        || *token_program.address() != pinocchio_token::ID
        || *_associated_token_program.address() != pinocchio_associated_token_account::ID
    {
        return Err(ProgramError::InvalidAccountData);
    }

    let (amount_to_receive, bump, escrow_maker) = {
        let escrow = Escrow::load_mut(escrow_account)?;
        if escrow.maker() != *maker.address()
            || escrow.mint_a() != *mint_a.address()
            || escrow.mint_b() != *mint_b.address()
        {
            return Err(ProgramError::InvalidAccountData);
        }
        (escrow.amount_to_receive(), escrow.bump, escrow.maker())
    };

    let bump_bytes = [bump];
    let pda_seeds = [
        b"escrow".as_ref(),
        escrow_maker.as_ref(),
        bump_bytes.as_ref(),
    ];
    if Address::from(derive_address(&pda_seeds, None, crate::ID.as_array()))
        != *escrow_account.address()
    {
        return Err(ProgramError::InvalidSeeds);
    }

    let expected_vault = Address::derive_program_address(
        &[
            escrow_account.address().as_ref(),
            token_program.address().as_ref(),
            mint_a.address().as_ref(),
        ],
        &pinocchio_associated_token_account::ID,
    )
    .ok_or(ProgramError::InvalidSeeds)?
    .0;
    if expected_vault != *vault.address() {
        return Err(ProgramError::InvalidAccountData);
    }

    let expected_taker_ata_a = Address::derive_program_address(
        &[
            taker.address().as_ref(),
            token_program.address().as_ref(),
            mint_a.address().as_ref(),
        ],
        &pinocchio_associated_token_account::ID,
    )
    .ok_or(ProgramError::InvalidSeeds)?
    .0;
    let expected_taker_ata_b = Address::derive_program_address(
        &[
            taker.address().as_ref(),
            token_program.address().as_ref(),
            mint_b.address().as_ref(),
        ],
        &pinocchio_associated_token_account::ID,
    )
    .ok_or(ProgramError::InvalidSeeds)?
    .0;
    let expected_maker_ata_b = Address::derive_program_address(
        &[
            maker.address().as_ref(),
            token_program.address().as_ref(),
            mint_b.address().as_ref(),
        ],
        &pinocchio_associated_token_account::ID,
    )
    .ok_or(ProgramError::InvalidSeeds)?
    .0;
    if expected_taker_ata_a != *taker_ata_a.address()
        || expected_taker_ata_b != *taker_ata_b.address()
        || expected_maker_ata_b != *maker_ata_b.address()
    {
        return Err(ProgramError::InvalidAccountData);
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
        let taker_ata_b_state = pinocchio_token::state::Account::from_account_view(taker_ata_b)?;
        if *taker_ata_b_state.owner() != *taker.address()
            || *taker_ata_b_state.mint() != *mint_b.address()
        {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    pinocchio_associated_token_account::instructions::CreateIdempotent {
        funding_account: taker,
        account: taker_ata_a,
        wallet: taker,
        mint: mint_a,
        system_program,
        token_program,
    }
    .invoke()?;

    {
        let taker_ata_a_state = pinocchio_token::state::Account::from_account_view(taker_ata_a)?;
        if *taker_ata_a_state.owner() != *taker.address()
            || *taker_ata_a_state.mint() != *mint_a.address()
        {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    pinocchio_associated_token_account::instructions::CreateIdempotent {
        funding_account: taker,
        account: maker_ata_b,
        wallet: maker,
        mint: mint_b,
        system_program,
        token_program,
    }
    .invoke()?;

    pinocchio_token::instructions::Transfer {
        from: taker_ata_b,
        to: maker_ata_b,
        authority: taker,
        multisig_signers: &[] as &[&AccountView],
        amount: amount_to_receive,
    }
    .invoke()?;

    let seed = [
        Seed::from(b"escrow"),
        Seed::from(escrow_maker.as_array()),
        Seed::from(&bump_bytes),
    ];
    let signer = Signer::from(&seed);

    pinocchio_token::instructions::Transfer {
        from: vault,
        to: taker_ata_a,
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
