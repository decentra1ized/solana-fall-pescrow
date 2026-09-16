use pinocchio::{
    cpi::{Seed, Signer},
    error::ProgramError,
    AccountView, Address, ProgramResult,
};

use crate::state::Escrow;

/// Cancel accounts, in order:
/// 0. maker (writable signer)
/// 1. mint A
/// 2. escrow PDA (writable)
/// 3. vault ATA (writable)
/// 4. maker ATA for mint A (writable)
/// 5. token program
pub fn process_cancel_instruction(accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    let [maker, mint_a, escrow_account, vault, maker_ata_a, token_program] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    if !data.is_empty() {
        return Err(ProgramError::InvalidInstructionData);
    }
    if !maker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }
    if !maker.is_writable()
        || !escrow_account.is_writable()
        || !vault.is_writable()
        || !maker_ata_a.is_writable()
    {
        return Err(ProgramError::Immutable);
    }
    if token_program.address() != &pinocchio_token::ID {
        return Err(ProgramError::IncorrectProgramId);
    }
    if !mint_a.owned_by(&pinocchio_token::ID) {
        return Err(ProgramError::IllegalOwner);
    }
    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::IllegalOwner);
    }

    let bump = {
        let escrow = Escrow::load_mut(escrow_account)?;
        if escrow.maker() != *maker.address() || escrow.mint_a() != *mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        escrow.bump
    };

    let bump_bytes = [bump];
    let expected_escrow = pinocchio_pubkey::derive_address(
        &[b"escrow", maker.address().as_ref(), bump_bytes.as_ref()],
        None,
        &crate::ID.to_bytes(),
    );
    if expected_escrow != escrow_account.address().to_bytes() {
        return Err(ProgramError::InvalidSeeds);
    }

    verify_ata(vault, escrow_account.address(), mint_a.address())?;
    verify_ata(maker_ata_a, maker.address(), mint_a.address())?;

    let vault_amount = {
        let vault_state = pinocchio_token::state::Account::from_account_view(vault)?;
        if vault_state.owner() != escrow_account.address() || vault_state.mint() != mint_a.address()
        {
            return Err(ProgramError::InvalidAccountData);
        }
        vault_state.amount()
    };
    {
        let maker_ata_a_state = pinocchio_token::state::Account::from_account_view(maker_ata_a)?;
        if maker_ata_a_state.owner() != maker.address()
            || maker_ata_a_state.mint() != mint_a.address()
        {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    let signer_seeds = [
        Seed::from(b"escrow"),
        Seed::from(maker.address().as_array()),
        Seed::from(&bump_bytes),
    ];
    let signer = Signer::from(&signer_seeds);

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

    let refunded_lamports = maker
        .lamports()
        .checked_add(escrow_account.lamports())
        .ok_or(ProgramError::ArithmeticOverflow)?;
    maker.set_lamports(refunded_lamports);
    escrow_account.set_lamports(0);
    escrow_account.close()?;

    Ok(())
}

fn verify_ata(account: &AccountView, wallet: &Address, mint: &Address) -> ProgramResult {
    let (expected, _) = Address::find_program_address(
        &[wallet.as_ref(), pinocchio_token::ID.as_ref(), mint.as_ref()],
        &pinocchio_associated_token_account::ID,
    );
    if expected != *account.address() {
        return Err(ProgramError::InvalidSeeds);
    }
    Ok(())
}
