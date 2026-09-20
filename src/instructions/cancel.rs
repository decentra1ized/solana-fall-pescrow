// ============================================================
// ACCOUNT ORDER — client must match this exactly
// 0  maker            (writable, signer) - the one who created the escrow, wants it back
// 1  mint_a                              - the token that was deposited
// 2  escrow_account   (writable)         - the deal record, will be closed
// 3  vault            (writable)         - the "safe" holding token A
// 4  maker_ata_a      (writable)         - where maker gets token A back
// 5  token_program
// ============================================================

use pinocchio::{
    cpi::{Seed, Signer},
    error::ProgramError,
    AccountView, ProgramResult,
};
use pinocchio_pubkey::derive_address;
use pinocchio_token::state::Account as TokenAccount;

use crate::state::Escrow;

// Runs when the maker wants to cancel their own escrow and get their tokens back.
pub fn process_cancel_instruction(accounts: &mut [AccountView], _data: &[u8]) -> ProgramResult {
    // STEP 1 — Unpack accounts, check maker actually signed.
    let [maker, mint_a, escrow_account, vault, maker_ata_a, _token_program] = accounts else {
        return Err(ProgramError::Custom(1));
    };

    if !maker.is_signer() {
        return Err(ProgramError::Custom(2));
    }

    // STEP 2 — Load escrow, verify ownership, verify maker & mint match,
    // copy out the bump, then let the borrow drop.
    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::Custom(3));
    }

    let bump = {
        let escrow = Escrow::load_mut(escrow_account)?;
        if escrow.maker() != *maker.address() {
            return Err(ProgramError::Custom(3));
        }
        if escrow.mint_a() != *mint_a.address() {
            return Err(ProgramError::Custom(3));
        }
        escrow.bump
    }; // <- borrow released here

    // STEP 3 — Re-derive the PDA using the saved bump, confirm it matches.
    let expected = derive_address(
        &[b"escrow", maker.address().as_ref(), &[bump]],
        None,
        &crate::ID.to_bytes(),
    );
    if expected != *escrow_account.address().as_array() {
        return Err(ProgramError::Custom(4));
    }

    // STEP 4 — Validate the vault, read its balance, drop the borrow.
    // Also validate maker's own receiving account.
    let vault_amount = {
        let vault_state = TokenAccount::from_account_view(vault)?;
        if vault_state.owner() != escrow_account.address() {
            return Err(ProgramError::Custom(5));
        }
        if vault_state.mint() != mint_a.address() {
            return Err(ProgramError::Custom(3));
        }
        vault_state.amount()
    };

    {
        let s = TokenAccount::from_account_view(maker_ata_a)?;
        if s.owner() != maker.address() {
            return Err(ProgramError::Custom(5));
        }
        if s.mint() != mint_a.address() {
            return Err(ProgramError::Custom(3));
        }
    }

    // STEP 5 — Build the PDA "signature" — same pattern as Take.
    let bump_bytes = [bump];
    let seed = [
        Seed::from(b"escrow"),
        Seed::from(maker.address().as_array()),
        Seed::from(&bump_bytes),
    ];
    let signer = Signer::from(&seed);

    // STEP 6 — CPI: vault pays maker back (the only transfer in Cancel).
    pinocchio_token::instructions::Transfer {
        from: vault,
        to: maker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }
    .invoke_signed(&[signer.clone()])?;

    // STEP 7 — Close the now-empty vault, rent goes back to maker.
    pinocchio_token::instructions::CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }
    .invoke_signed(&[signer])?;

    // STEP 8 — Close the escrow account by hand, exactly like Take step 10.
    let maker_lamports = maker.lamports();
    let escrow_lamports = escrow_account.lamports();
    maker.set_lamports(maker_lamports + escrow_lamports);

    escrow_account.set_lamports(0);
    escrow_account.close()?;

    Ok(())
}
