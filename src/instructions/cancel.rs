use pinocchio::{
    AccountView, ProgramResult,
    cpi::{Seed, Signer},
    error::ProgramError,
};

use crate::{instructions::shared::drain_vault_and_close_escrow, state::Escrow};

/// Accounts, in order (README §05 · Challenge 2):
/// 0. `maker`          [signer, writable] - must sign; must equal escrow.maker()
/// 1. `mint_a`                             - must equal escrow.mint_a()
/// 2. `escrow_account` [writable]         - PDA, will be closed
/// 3. `vault`          [writable]         - will be closed
/// 4. `maker_ata_a`    [writable]         - destination for the returned A
/// 5. `token_program`
pub fn process_cancel_instruction(accounts: &mut [AccountView], _data: &[u8]) -> ProgramResult {
    let [maker, mint_a, escrow_account, vault, maker_ata_a, _token_program @ ..] = accounts
    else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    // The critical authorization check: without it, anyone could cancel (and
    // drain) any escrow back to its maker.
    if !maker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::InvalidAccountOwner);
    }

    // `is_signer` proves who sent the transaction; these two prove it is the
    // right escrow for that signer - without them a signer could cancel
    // *someone else's* escrow into their own wallet.
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

    let derived_escrow = pinocchio_pubkey::derive_address(
        &[b"escrow", maker.address().as_ref(), &[bump]],
        None,
        &crate::ID.to_bytes(),
    );
    if derived_escrow != *escrow_account.address().as_array() {
        return Err(ProgramError::InvalidSeeds);
    }

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

    {
        let maker_ata_a_state = pinocchio_token::state::Account::from_account_view(maker_ata_a)?;
        if maker_ata_a_state.owner() != maker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if maker_ata_a_state.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    // Copy the address out to an owned local first: a `Seed` built straight
    // from `maker.address()` would keep an immutable borrow on `maker` alive
    // for as long as `signer` lives, which conflicts with passing `maker` as
    // `&mut AccountView` into the helper below.
    let maker_address = *maker.address().as_array();
    let bump_bytes = [bump];
    let seed = [
        Seed::from(b"escrow"),
        Seed::from(&maker_address),
        Seed::from(&bump_bytes),
    ];
    let signer = Signer::from(&seed);

    drain_vault_and_close_escrow(
        vault,
        maker_ata_a,
        escrow_account,
        maker,
        vault_amount,
        &signer,
    )?;

    Ok(())
}
