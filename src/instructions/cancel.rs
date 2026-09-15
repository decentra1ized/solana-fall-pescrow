use pinocchio::{
    AccountView, Address, ProgramResult,
    cpi::{Seed, Signer},
    error::ProgramError,
};
use pinocchio_pubkey::derive_address;

use crate::state::Escrow;

/// Cancel instruction:
///
/// Accounts:
/// 0: `[signer, writable]`  maker (must sign, must equal escrow.maker())
/// 1: `[]`                  mint_a (must equal escrow.mint_a())
/// 2: `[writable]`          escrow_account (PDA, closed)
/// 3: `[writable]`          vault (ATA(escrow PDA, mint_a), closed)
/// 4: `[writable]`          maker_ata_a (destination for returned token A)
/// 5: `[]`                  token_program
pub fn process_cancel_instruction(
    accounts: &mut [AccountView],
    _data: &[u8],
) -> ProgramResult {
    let [
        maker,
        mint_a,
        escrow_account,
        vault,
        maker_ata_a,
        _token_program @ ..
    ] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    // Critical authorization check: maker must sign.
    if !maker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::InvalidAccountOwner);
    }

    // Load escrow state, cross-check maker and mint_a, extract bump.
    // Scoped so the RefMut guard is released before any CPI.
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

    // Re-derive and check the PDA using the stored canonical bump.
    let bump_bytes = [bump];
    let expected_escrow = derive_address(
        &[b"escrow", maker.address().as_ref(), &bump_bytes],
        None,
        &crate::ID.to_bytes(),
    );
    if Address::from(expected_escrow) != *escrow_account.address() {
        return Err(ProgramError::InvalidSeeds);
    }

    // Validate the vault and read the token amount to return.
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

    // Validate maker's destination ATA for token A.
    {
        let maker_ata_a_state = pinocchio_token::state::Account::from_account_view(maker_ata_a)?;
        if maker_ata_a_state.owner() != maker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if maker_ata_a_state.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    // Build the PDA signer.
    let seed = [
        Seed::from(b"escrow"),
        Seed::from(maker.address().as_array()),
        Seed::from(&bump_bytes),
    ];
    let signer = Signer::from(&seed);

    // CPI 1: Vault returns token A to maker_ata_a.
    pinocchio_token::instructions::Transfer {
        from: vault,
        to: maker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }.invoke_signed(&[signer.clone()])?;

    // CPI 2: Close the vault and return rent to maker.
    pinocchio_token::instructions::CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }.invoke_signed(&[signer.clone()])?;

    // Close the escrow account: transfer lamports to maker, zero balance, close.
    maker.set_lamports(maker.lamports() + escrow_account.lamports());
    escrow_account.set_lamports(0);
    escrow_account.close()?;

    Ok(())
}
