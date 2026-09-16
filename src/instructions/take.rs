use pinocchio::{
    cpi::{Seed, Signer},
    error::ProgramError,
    AccountView, ProgramResult,
};

use crate::state::Escrow;

/// Accounts expected by `Take`, in order:
/// 0. `[writable, signer]` taker
/// 1. `[writable]` maker
/// 2. `[]` mint A
/// 3. `[]` mint B
/// 4. `[writable]` escrow account PDA
/// 5. `[writable]` vault ATA (owner = escrow PDA, mint = mint A)
/// 6. `[writable]` taker's ATA for mint A
/// 7. `[writable]` taker's ATA for mint B
/// 8. `[writable]` maker's ATA for mint B
/// 9. `[]` system program
/// 10. `[]` token program
/// 11. `[]` associated token program
pub fn process_take_instruction(accounts: &mut [AccountView]) -> ProgramResult {
    let [taker, maker, mint_a, mint_b, escrow_account, vault, taker_ata_a, taker_ata_b, maker_ata_b, system_program, token_program, _associated_token_program, ..] =
        accounts
    else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    if !taker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    // State is trusted only after verifying that this program owns the account.
    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::IllegalOwner);
    }

    // Copy everything needed later so the RefMut is dropped before any CPI.
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
        &[
            b"escrow".as_ref(),
            maker.address().as_ref(),
            bump_bytes.as_ref(),
        ],
        None,
        &crate::ID.to_bytes(),
    );
    if pinocchio::Address::from(expected_escrow) != *escrow_account.address() {
        return Err(ProgramError::InvalidSeeds);
    }

    // The complete vault balance is released to the taker.
    let vault_amount = {
        let vault_state = pinocchio_token::state::Account::from_account_view(vault)?;
        if vault_state.owner() != escrow_account.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if vault_state.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        vault_state.amount()
    };

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

    // The taker's source token account must belong to the taker and hold mint B.
    {
        let taker_ata_b_state = pinocchio_token::state::Account::from_account_view(taker_ata_b)?;
        if taker_ata_b_state.owner() != taker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if taker_ata_b_state.mint() != mint_b.address() {
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
    let escrow_signer = Signer::from(&signer_seeds);

    pinocchio_token::instructions::Transfer {
        from: vault,
        to: taker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }
    .invoke_signed(&[escrow_signer.clone()])?;

    pinocchio_token::instructions::CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }
    .invoke_signed(&[escrow_signer])?;

    let maker_lamports = maker
        .lamports()
        .checked_add(escrow_account.lamports())
        .ok_or(ProgramError::ArithmeticOverflow)?;
    maker.set_lamports(maker_lamports);
    escrow_account.set_lamports(0);
    escrow_account.close()?;

    Ok(())
}
