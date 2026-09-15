use pinocchio::{
    AccountView, ProgramResult,
    cpi::{Seed, Signer},
    error::ProgramError,
};
use pinocchio_token::instructions::{CloseAccount, Transfer};

use crate::state::Escrow;

/// `Cancel` — the maker calls off the deal and reclaims the vault.
///
/// Accounts (all positional, exactly as the client must order them):
///   0. `[writable, signer]` maker          — must sign and must equal `escrow.maker()`
///   1. `[]`                 mint_a         — must equal `escrow.mint_a()`
///   2. `[writable]`         escrow_account — PDA ["escrow", maker, bump]; closed here
///   3. `[writable]`         vault          — ATA(escrow PDA, mint A); closed here
///   4. `[writable]`         maker_ata_a    — ATA(maker, mint A); destination for the refund
///   5. `[]`                 token_program
pub fn process_cancel_instruction(accounts: &mut [AccountView]) -> ProgramResult {
    let [maker, mint_a, escrow_account, vault, maker_ata_a, _token_program @ ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    // 1. Authorization. This is the check that keeps strangers out: without it, anybody
    //    could pass someone else's escrow and drain it back to the stored maker.
    if !maker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    // 2. The escrow account must be one of ours before we read its bytes as `Escrow`.
    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::InvalidAccountOwner);
    }

    // 3. Signing is not enough: the signer must be the maker this escrow recorded.
    //    Scoped so the `RefMut` guard is dropped before any CPI or `close()`.
    let bump = {
        let escrow_state = Escrow::load_mut(escrow_account)?;

        if escrow_state.maker() != *maker.address() {
            return Err(ProgramError::InvalidAccountOwner);
        }
        if escrow_state.mint_a() != *mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }

        escrow_state.bump
    };

    // 4. Re-derive the PDA from the stored bump (a single hash, no search loop).
    let bump_seed = [bump];
    let derived_escrow = pinocchio_pubkey::derive_address(
        &[b"escrow".as_ref(), maker.address().as_ref(), &bump_seed],
        None,
        &crate::ID.to_bytes(),
    );
    if derived_escrow != *escrow_account.address().as_array() {
        return Err(ProgramError::InvalidSeeds);
    }

    // 5. The vault must be the token account this escrow PDA controls, holding mint A.
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

    // The refund destination is supplied, not created: check it is really the maker's.
    {
        let maker_ata_a_state = pinocchio_token::state::Account::from_account_view(maker_ata_a)?;
        if maker_ata_a_state.owner() != maker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if maker_ata_a_state.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    // 6. The escrow PDA signs for the vault it controls.
    let seed = [
        Seed::from(b"escrow"),
        Seed::from(maker.address().as_array()),
        Seed::from(&bump_seed),
    ];
    let signer = Signer::from(&seed);

    // 7. Drain the vault back to the maker, then close it.
    Transfer {
        from: vault,
        to: maker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }
    .invoke_signed(&[signer.clone()])?;

    CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }
    .invoke_signed(&[signer])?;

    // 8. Close the escrow state account by hand — same helper `Take` uses.
    crate::instructions::close_program_account(escrow_account, maker)?;

    Ok(())
}
