// src/instructions/take.rs
use pinocchio::{
    account::AccountView,
    cpi::{Seed, Signer},
    error::ProgramError,
    ProgramResult,
};
use pinocchio_pubkey::derive_address;
use pinocchio_token::{
    instructions::{CloseAccount, Transfer},
    state::Account as TokenAccount,
};
use pinocchio_associated_token_account::instructions::CreateIdempotent;

use crate::{state::Escrow, ID};

/// Accounts (order is the API):
/// 0. taker (w, s)
/// 1. maker (w)
/// 2. mint_a
/// 3. mint_b
/// 4. escrow_account (w)  PDA
/// 5. vault (w)           ATA(escrow, mint_a)
/// 6. taker_ata_a (w)
/// 7. taker_ata_b (w)
/// 8. maker_ata_b (w)
/// 9. system_program
/// 10. token_program
/// 11. associated_token_program
pub fn process_take_instruction(accounts: &mut [AccountView]) -> ProgramResult {
    let [
        taker, maker, mint_a, mint_b,
        escrow_account, vault,
        taker_ata_a, taker_ata_b, maker_ata_b,
        system_program, token_program, _ata_program
    ] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    if !taker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    // Ownership check BEFORE borrowing state mutably
    if !escrow_account.owned_by(&ID) {
        return Err(ProgramError::InvalidAccountData);
    }

    // Load, cross-check, copy out primitives, drop the borrow
    let (amount_to_receive, bump) = {
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

    // Re-derive the PDA
    let derived = derive_address(
        &[b"escrow", maker.address().as_ref(), &[bump]],
        None,
        &ID.to_bytes(),
    );
    if derived != *escrow_account.address().as_array() {
        return Err(ProgramError::InvalidSeeds);
    }

    // Validate vault and read its balance
    let vault_amount = {
        let vault_state = TokenAccount::from_account_view(vault)?;
        if vault_state.owner() != escrow_account.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if vault_state.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        vault_state.amount()
    };

    // Ensure destination ATAs exist (idempotent)
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

    // Validate taker_ata_b
    {
        let taker_b_state = TokenAccount::from_account_view(taker_ata_b)?;
        if taker_b_state.owner() != taker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if taker_b_state.mint() != mint_b.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    // CPI #1: taker pays maker in mint B
    Transfer {
        from: taker_ata_b,
        to: maker_ata_b,
        authority: taker,
        multisig_signers: &[] as &[&AccountView],
        amount: amount_to_receive,
    }
    .invoke()?;

    // Build PDA signer
    let bump_bytes = [bump];
    let seed = [
        Seed::from(b"escrow"),
        Seed::from(maker.address().as_array()),
        Seed::from(&bump_bytes),
    ];
    let signer = Signer::from(&seed);

    // CPI #2: vault pays taker
    Transfer {
        from: vault,
        to: taker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }
    .invoke_signed(&[signer.clone()])?;

    // CPI #3: close the vault
    CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }
    .invoke_signed(&[signer.clone()])?;

    // Close the escrow account by hand
    maker.set_lamports(maker.lamports() + escrow_account.lamports());
    escrow_account.set_lamports(0);
    escrow_account.close()?;

    Ok(())
}
