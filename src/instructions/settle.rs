//! The tail both Take and Cancel share.
//!
//! Take and Cancel differ only in who is paid and who has to sign. Once that is
//! settled, the ending is identical: move everything out of the vault, close the
//! vault, close the escrow, and refund both rents to the maker who paid them.
//! Take sends the vault to the taker, Cancel sends it back to the maker; that is
//! the only parameter.

use pinocchio::{
    AccountView, ProgramResult,
    cpi::{Seed, Signer},
    error::ProgramError,
};
use pinocchio_token::instructions::{CloseAccount, Transfer};

use crate::state::Escrow;

/// Read the escrow, check it against the accounts the caller passed, and return
/// the two values the rest of the instruction needs.
///
/// The ownership check comes first and is the load-bearing one: until we know
/// this program owns the account, its bytes are whatever the caller wanted them
/// to be, and `maker()` is just a number they chose. Checking the stored maker
/// against an account inside an escrow we do not own proves nothing at all.
///
/// The returned `RefMut` guard is dropped before this function returns, which is
/// the point of reading the fields into locals here: any CPI on `escrow_account`
/// while that guard lives fails with `AccountBorrowFailed`, and so does `close`.
pub fn load_and_verify_escrow(
    escrow_account: &mut AccountView,
    maker: &AccountView,
    mint_a: &AccountView,
    mint_b: Option<&AccountView>,
) -> Result<(u64, u8), ProgramError> {
    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::IllegalOwner);
    }

    let (amount_to_receive, bump) = {
        let escrow = Escrow::load_mut(escrow_account)?;

        if escrow.maker() != *maker.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        if escrow.mint_a() != *mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        // Cancel never touches mint B, so it has no account to check against.
        if let Some(mint_b) = mint_b {
            if escrow.mint_b() != *mint_b.address() {
                return Err(ProgramError::InvalidAccountData);
            }
        }

        (escrow.amount_to_receive(), escrow.bump)
    }; // guard dropped here, before anything below can CPI or close

    // Re-derive with the stored bump: one hash, not the loop `find_program_address`
    // runs. Make already paid for that search and wrote the answer into state.
    //
    // This is what ties the escrow to *this* maker. Without it a caller could pass
    // any program-owned account of the right length whose bytes happen to name the
    // maker they control.
    let bump_bytes = [bump];
    let derived = pinocchio_pubkey::derive_address(
        &[b"escrow", maker.address().as_ref(), &bump_bytes],
        None,
        &crate::ID.to_bytes(),
    );
    if derived != *escrow_account.address().as_array() {
        return Err(ProgramError::InvalidSeeds);
    }

    Ok((amount_to_receive, bump))
}

/// Validate a token account the way Make validated the maker's: right owner,
/// right mint. Returns its balance.
///
/// Scoped so the data borrow is released before the caller CPIs with the same
/// account — the mistake `make.rs` puts a comment on, made once here instead of
/// four times across two instructions.
pub fn check_token_account(
    account: &AccountView,
    expected_owner: &AccountView,
    expected_mint: &AccountView,
) -> Result<u64, ProgramError> {
    let state = pinocchio_token::state::Account::from_account_view(account)?;
    if state.owner() != expected_owner.address() {
        return Err(ProgramError::IllegalOwner);
    }
    if state.mint() != expected_mint.address() {
        return Err(ProgramError::InvalidAccountData);
    }
    Ok(state.amount())
}

/// Empty the vault into `vault_destination`, close the vault, close the escrow.
/// Both rents go to `maker`, who paid them in Make.
///
/// `vault_amount` is the vault's *actual* balance, read by the caller — not
/// `escrow.amount_to_give()`. They are equal on the happy path, which is exactly
/// why using the stored figure is a bug that no passing test would reveal: a
/// vault topped up by a stray transfer would leave the remainder stranded in a
/// closed account, and `CloseAccount` refuses a non-empty account anyway.
pub fn settle_and_close(
    escrow_account: &mut AccountView,
    vault: &AccountView,
    vault_destination: &AccountView,
    maker: &mut AccountView,
    vault_amount: u64,
    bump: u8,
) -> ProgramResult {
    // Copy the maker's address out before building the seeds. The seeds borrow
    // whatever they are built from, and we need `maker` mutably further down to
    // credit it the escrow's rent; copying 32 bytes ends that argument.
    let maker_key = *maker.address().as_array();
    let bump_bytes = [bump];
    let seed = [
        Seed::from(b"escrow"),
        Seed::from(&maker_key),
        Seed::from(&bump_bytes),
    ];
    let signer = Signer::from(&seed);

    // The vault's authority is the escrow PDA, so both of these are invoke_signed.
    // The program proves it knows the seeds; no wallet can produce this signature.
    Transfer {
        from: vault,
        to: vault_destination,
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

    // The escrow is ours, so there is no CPI for it — we move the lamports
    // ourselves. Order matters: credit the maker first, zero the escrow, then
    // close. Skip the move and the runtime rejects the whole instruction because
    // lamports vanished; `close` zeroes them along with the data and the owner.
    let rent = escrow_account.lamports();
    let credited = maker
        .lamports()
        .checked_add(rent)
        .ok_or(ProgramError::ArithmeticOverflow)?;
    maker.set_lamports(credited);
    escrow_account.set_lamports(0);
    escrow_account.close()
}
