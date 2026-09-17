use pinocchio::{
    AccountView, ProgramResult,
    cpi::{Seed, Signer},
    error::ProgramError,
};

use crate::{instructions::shared::drain_vault_and_close_escrow, state::Escrow};

/// Accounts, in order (README §04 · Challenge 1):
/// 0. `taker`                    [signer, writable] - pays fees, creates own ATA for A if missing
/// 1. `maker`                    [writable]          - receives rent refunds; must equal escrow.maker()
/// 2. `mint_a`                                        - must equal escrow.mint_a()
/// 3. `mint_b`                                        - must equal escrow.mint_b()
/// 4. `escrow_account`           [writable]          - PDA, owned by this program, will be closed
/// 5. `vault`                    [writable]          - ATA(escrow PDA, mint A), will be closed
/// 6. `taker_ata_a`               [writable]         - ATA(taker, mint A), destination for A; may need to be created
/// 7. `taker_ata_b`               [writable]         - ATA(taker, mint B), source of B; must exist and have balance
/// 8. `maker_ata_b`               [writable]         - ATA(maker, mint B), destination for B; may need to be created
/// 9. `system_program`
/// 10. `token_program`
/// 11. `associated_token_program`
pub fn process_take_instruction(accounts: &mut [AccountView], _data: &[u8]) -> ProgramResult {
    let [
        taker,
        maker,
        mint_a,
        mint_b,
        escrow_account,
        vault,
        taker_ata_a,
        taker_ata_b,
        maker_ata_b,
        system_program,
        token_program,
        _associated_token_program @ ..,
    ] = accounts
    else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    if !taker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    // Trust the escrow's contents only once we know this program owns the account.
    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::InvalidAccountOwner);
    }

    // Cross-check the passed accounts against the stored state, then drop the
    // borrow: `escrow_account` is used as a CPI signer below, and a live
    // borrow on its data would make that fail with `AccountBorrowFailed`.
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

    // The bump is the canonical one Make stored: re-derive with it (a single
    // hash), never recompute with find_program_address here.
    let derived_escrow = pinocchio_pubkey::derive_address(
        &[b"escrow", maker.address().as_ref(), &[bump]],
        None,
        &crate::ID.to_bytes(),
    );
    if derived_escrow != *escrow_account.address().as_array() {
        return Err(ProgramError::InvalidSeeds);
    }

    // Validate the vault and read how much A it holds - that is what the
    // taker receives, not `amount_to_give` from state (which could in
    // principle differ from the vault's live balance).
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

    // taker_ata_a and maker_ata_b may not exist yet; CreateIdempotent is a
    // no-op if they already do.
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

    // taker_ata_b must already exist and hold enough B - validate it the
    // same way Make validated the maker's source ATA.
    {
        let taker_ata_b_state = pinocchio_token::state::Account::from_account_view(taker_ata_b)?;
        if taker_ata_b_state.owner() != taker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if taker_ata_b_state.mint() != mint_b.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    // CPI #1: taker pays maker `amount_to_receive` of mint B. The taker
    // signed the transaction, so this is a plain invoke().
    pinocchio_token::instructions::Transfer {
        from: taker_ata_b,
        to: maker_ata_b,
        authority: taker,
        multisig_signers: &[] as &[&AccountView],
        amount: amount_to_receive,
    }
    .invoke()?;

    // CPI #2 and #3, then closing the escrow by hand: vault pays taker all of
    // A, vault closes, escrow closes - all signed by the escrow PDA. This
    // tail is identical to Cancel's, factored into a shared helper.
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
        taker_ata_a,
        escrow_account,
        maker,
        vault_amount,
        &signer,
    )?;

    Ok(())
}
