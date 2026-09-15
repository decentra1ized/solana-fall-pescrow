// src/instructions/cancel.rs
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

use crate::{state::Escrow, ID};

/// Accounts (order is the API):
/// 0. maker (w, s)      must sign, must equal escrow.maker()
/// 1. mint_a            must equal escrow.mint_a()
/// 2. escrow_account (w) PDA, will be closed
/// 3. vault (w)          ATA(escrow, mint_a), will be closed
/// 4. maker_ata_a (w)    destination for the returned A
/// 5. token_program
pub fn process_cancel_instruction(accounts: &mut [AccountView]) -> ProgramResult {
    // 1. Destructure and check signer
    let [
        maker, mint_a,
        escrow_account, vault,
        maker_ata_a,
        _token_program
    ] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    if !maker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    // 2. Ownership check BEFORE borrowing state mutably
    if !escrow_account.owned_by(&ID) {
        return Err(ProgramError::InvalidAccountData);
    }

    // Load, cross-check, copy out primitives, drop the borrow
    let bump = {
        let escrow = Escrow::load_mut(escrow_account)?;

        if escrow.maker() != *maker.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        if escrow.mint_a() != *mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }

        escrow.bump
    };

    // 3. Re-derive the PDA from the stored bump
    let derived = derive_address(
        &[b"escrow", maker.address().as_ref(), &[bump]],
        None,
        &ID.to_bytes(),
    );
    if derived != *escrow_account.address().as_array() {
        return Err(ProgramError::InvalidSeeds);
    }

    // 4a. Validate vault and read its balance
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

    // 4b. Validate maker_ata_a
    {
        let maker_ata_state = TokenAccount::from_account_view(maker_ata_a)?;
        if maker_ata_state.owner() != maker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if maker_ata_state.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    // 5. Build the PDA signer
    let bump_bytes = [bump];
    let seed = [
        Seed::from(b"escrow"),
        Seed::from(maker.address().as_array()),
        Seed::from(&bump_bytes),
    ];
    let signer = Signer::from(&seed);

    // 6. CPI #1: vault pays maker
    Transfer {
        from: vault,
        to: maker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }
    .invoke_signed(&[signer.clone()])?;

    // 7. CPI #2: close the vault
    CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }
    .invoke_signed(&[signer.clone()])?;

    // 8. Close the escrow account by hand
    maker.set_lamports(maker.lamports() + escrow_account.lamports());
    escrow_account.set_lamports(0);
    escrow_account.close()?;

    Ok(())
}
