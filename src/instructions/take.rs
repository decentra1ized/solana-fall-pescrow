//! Take: the taker pays the maker in mint B and receives the vault's mint A.
//!
//! Account order is the API — there is no IDL. Keep the test in sync with this:
//!
//!   0  taker                     signer, writable
//!   1  maker                     writable, must equal escrow.maker()
//!   2  mint_a                    must equal escrow.mint_a()
//!   3  mint_b                    must equal escrow.mint_b()
//!   4  escrow_account            writable, PDA, closed here
//!   5  vault                     writable, ATA(escrow, mint_a), closed here
//!   6  taker_ata_a               writable, created if missing
//!   7  taker_ata_b               writable, source of B
//!   8  maker_ata_b               writable, created if missing
//!   9  system_program
//!  10  token_program
//!  11  associated_token_program

use pinocchio::{
    AccountView, ProgramResult,
    cpi::{Seed, Signer},
    error::ProgramError,
};

use crate::state::Escrow;
use super::shared::settle;

pub fn process_take_instruction(accounts: &mut [AccountView]) -> ProgramResult {
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
        _associated_token_program @ ..
    ] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    if !taker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    // Check ownership before trusting a single byte of the state: without this,
    // anyone could pass a look-alike account carrying a maker of their choosing.
    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::IllegalOwner);
    }

    // Copy what we need out and drop the guard: the CPIs below sign as this
    // account, and a live borrow makes them fail with AccountBorrowFailed.
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

    // Single hash from the stored canonical bump, not a find_program_address
    // loop. Proves this escrow belongs to this maker under this bump.
    // derive_address returns the raw bytes, so wrap them before comparing.
    let expected = pinocchio::Address::from(pinocchio_pubkey::derive_address(
        &[b"escrow", maker.address().as_ref(), &[bump]],
        None,
        &crate::ID.to_bytes(),
    ));
    if expected != *escrow_account.address() {
        return Err(ProgramError::InvalidSeeds);
    }

    // The vault's real balance, not escrow.amount_to_give(): a fee-bearing mint
    // could have delivered less than the maker sent.
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

    // Neither destination is guaranteed to exist; idempotent so an existing one
    // is not an error.
    pinocchio_associated_token_account::instructions::CreateIdempotent {
        funding_account: taker,
        account: taker_ata_a,
        wallet: taker,
        mint: mint_a,
        system_program,
        token_program,
    }.invoke()?;

    pinocchio_associated_token_account::instructions::CreateIdempotent {
        funding_account: taker,
        account: maker_ata_b,
        wallet: maker,
        mint: mint_b,
        system_program,
        token_program,
    }.invoke()?;

    {
        let taker_ata_b_state = pinocchio_token::state::Account::from_account_view(taker_ata_b)?;
        if taker_ata_b_state.owner() != taker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if taker_ata_b_state.mint() != mint_b.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    // The taker signed the transaction, so a plain invoke.
    pinocchio_token::instructions::Transfer {
        from: taker_ata_b,
        to: maker_ata_b,
        authority: taker,
        multisig_signers: &[] as &[&AccountView],
        amount: amount_to_receive,
    }.invoke()?;

    // Copy the address out first. A Seed borrowing from `maker` would keep an
    // immutable borrow alive, and `settle` needs `maker` mutably to move the
    // rent lamports.
    let maker_key = *maker.address().as_array();
    let bump_bytes = [bump];
    let seed = [
        Seed::from(b"escrow"),
        Seed::from(&maker_key),
        Seed::from(&bump_bytes),
    ];
    let signer = Signer::from(&seed);

    settle(
        vault,
        taker_ata_a,
        escrow_account,
        maker,
        vault_amount,
        &signer,
    )
}
