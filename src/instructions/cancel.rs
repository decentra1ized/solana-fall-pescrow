use pinocchio::{
    cpi::{Seed, Signer},
    error::ProgramError,
    AccountView, ProgramResult,
};

use crate::state::Escrow;

/// Accounts expected by Cancel:
/// 0. `[signer, writable]` maker: must sign, must match escrow.maker
/// 1. `[]` mint_a: token A mint (must match escrow.mint_a)
/// 2. `[writable]` escrow_account: PDA ["escrow", maker], will be closed
/// 3. `[writable]` vault: ATA(escrow PDA, mint A), will be closed
/// 4. `[writable]` maker_ata_a: ATA(maker, mint A), destination for returned token A
/// 5. `[]` token_program
pub fn process_cancel_instruction(accounts: &mut [AccountView]) -> ProgramResult {
    let [maker, mint_a, escrow_account, vault, maker_ata_a, _token_program, _rem @ ..] = accounts
    else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    if !maker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::InvalidAccountData);
    }

    // Load escrow state and cross-check accounts.
    // Scoped so the RefMut borrow guard drops before any CPI.
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

    // Re-derive the PDA using the stored bump (single hash, no loop).
    let expected_pda = pinocchio_pubkey::derive_address(
        &[b"escrow", maker.address().as_ref(), &[bump]],
        None,
        &crate::ID.to_bytes(),
    );
    if expected_pda != *escrow_account.address().as_ref() {
        return Err(ProgramError::InvalidSeeds);
    }

    // Validate the vault and read the deposited token balance.
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

    // Validate maker's destination token account.
    {
        let maker_ata_a_state = pinocchio_token::state::Account::from_account_view(maker_ata_a)?;
        if maker_ata_a_state.owner() != maker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if maker_ata_a_state.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    // Build the PDA signer seeds.
    let bump_bytes = [bump];
    let seed = [
        Seed::from(b"escrow"),
        Seed::from(maker.address().as_array()),
        Seed::from(&bump_bytes),
    ];
    let signer = Signer::from(&seed);

    // CPI 1: Vault transfers token A back to maker, signed by the escrow PDA.
    pinocchio_token::instructions::Transfer {
        from: vault,
        to: maker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }
    .invoke_signed(core::slice::from_ref(&signer))?;

    // CPI 2: Close the vault account, sending remaining rent lamports to maker.
    pinocchio_token::instructions::CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }
    .invoke_signed(core::slice::from_ref(&signer))?;

    // Close the escrow account: transfer lamports to maker, then close.
    maker.set_lamports(maker.lamports() + escrow_account.lamports());
    escrow_account.set_lamports(0);
    escrow_account.close()?;

    Ok(())
}
