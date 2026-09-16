use pinocchio::{
    cpi::{Seed, Signer},
    error::ProgramError,
    AccountView, Address, ProgramResult,
};

use crate::state::Escrow;

/// Take accounts, in order:
/// 0. taker (writable signer)
/// 1. maker (writable)
/// 2. mint A
/// 3. mint B
/// 4. escrow PDA (writable)
/// 5. vault ATA (writable)
/// 6. taker ATA for mint A (writable)
/// 7. taker ATA for mint B (writable)
/// 8. maker ATA for mint B (writable)
/// 9. system program
/// 10. token program
/// 11. associated-token program
pub fn process_take_instruction(accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    let [taker, maker, mint_a, mint_b, escrow_account, vault, taker_ata_a, taker_ata_b, maker_ata_b, system_program, token_program, associated_token_program] =
        accounts
    else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    if !data.is_empty() {
        return Err(ProgramError::InvalidInstructionData);
    }
    if !taker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }
    if !taker.is_writable()
        || !maker.is_writable()
        || !escrow_account.is_writable()
        || !vault.is_writable()
        || !taker_ata_a.is_writable()
        || !taker_ata_b.is_writable()
        || !maker_ata_b.is_writable()
    {
        return Err(ProgramError::Immutable);
    }
    if system_program.address() != &pinocchio_system::ID
        || token_program.address() != &pinocchio_token::ID
        || associated_token_program.address() != &pinocchio_associated_token_account::ID
    {
        return Err(ProgramError::IncorrectProgramId);
    }
    if !mint_a.owned_by(&pinocchio_token::ID) || !mint_b.owned_by(&pinocchio_token::ID) {
        return Err(ProgramError::IllegalOwner);
    }
    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::IllegalOwner);
    }

    let (amount_to_receive, bump) = {
        let escrow = Escrow::load_mut(escrow_account)?;
        if escrow.maker() != *maker.address()
            || escrow.mint_a() != *mint_a.address()
            || escrow.mint_b() != *mint_b.address()
        {
            return Err(ProgramError::InvalidAccountData);
        }
        (escrow.amount_to_receive(), escrow.bump)
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
    verify_ata(taker_ata_a, taker.address(), mint_a.address())?;
    verify_ata(taker_ata_b, taker.address(), mint_b.address())?;
    verify_ata(maker_ata_b, maker.address(), mint_b.address())?;

    let vault_amount = {
        let vault_state = pinocchio_token::state::Account::from_account_view(vault)?;
        if vault_state.owner() != escrow_account.address() || vault_state.mint() != mint_a.address()
        {
            return Err(ProgramError::InvalidAccountData);
        }
        vault_state.amount()
    };
    {
        let taker_ata_b_state = pinocchio_token::state::Account::from_account_view(taker_ata_b)?;
        if taker_ata_b_state.owner() != taker.address()
            || taker_ata_b_state.mint() != mint_b.address()
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
    pinocchio_associated_token_account::instructions::CreateIdempotent {
        funding_account: taker,
        account: maker_ata_b,
        wallet: maker,
        mint: mint_b,
        system_program,
        token_program,
    }
    .invoke()?;

    // CreateIdempotent verifies these too, but keeping the relationships explicit here
    // makes the native program's account invariants visible at the call site.
    {
        let taker_ata_a_state = pinocchio_token::state::Account::from_account_view(taker_ata_a)?;
        if taker_ata_a_state.owner() != taker.address()
            || taker_ata_a_state.mint() != mint_a.address()
        {
            return Err(ProgramError::InvalidAccountData);
        }
    }
    {
        let maker_ata_b_state = pinocchio_token::state::Account::from_account_view(maker_ata_b)?;
        if maker_ata_b_state.owner() != maker.address()
            || maker_ata_b_state.mint() != mint_b.address()
        {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    pinocchio_token::instructions::Transfer {
        from: taker_ata_b,
        to: maker_ata_b,
        authority: taker,
        multisig_signers: &[] as &[&AccountView],
        amount: amount_to_receive,
    }
    .invoke()?;

    let signer_seeds = [
        Seed::from(b"escrow"),
        Seed::from(maker.address().as_array()),
        Seed::from(&bump_bytes),
    ];
    let signer = Signer::from(&signer_seeds);

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
