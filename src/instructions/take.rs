//! Take — the taker pays the maker in mint B and receives the vault's mint A.
//!
//! Accounts, in this exact order. There is no IDL, so this comment is the API:
//!
//!   #   account                    w  s   notes
//!   0   taker                      ✓  ✓   pays fees, funds any ATA it has to create
//!   1   maker                      ✓      receives rent refunds; must equal escrow.maker()
//!   2   mint_a                            must equal escrow.mint_a()
//!   3   mint_b                            must equal escrow.mint_b()
//!   4   escrow_account             ✓      PDA ["escrow", maker]; closed here
//!   5   vault                      ✓      ATA(escrow PDA, mint A); closed here
//!   6   taker_ata_a                ✓      destination for A; created if missing
//!   7   taker_ata_b                ✓      source of B; must already hold enough
//!   8   maker_ata_b                ✓      destination for B; created if missing
//!   9   system_program
//!  10   token_program
//!  11   associated_token_program
//!
//! Instruction data is the discriminator byte alone: the terms already live in
//! the escrow account.

use pinocchio::{AccountView, ProgramResult, error::ProgramError};

use crate::instructions::shared::drain_vault_and_close;
use crate::state::Escrow;

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
        _associated_token_program @ ..
    ] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    if !taker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    // Nothing inside the escrow is trustworthy until we know this program wrote
    // it. Without this check anyone could pass a look-alike account carrying a
    // maker and mints of their choosing.
    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::IllegalOwner);
    }

    // Cross-check the passed accounts against the recorded terms, copy out what
    // we need, and let the guard drop: the CPIs below touch this same account,
    // and a live borrow would fail with AccountBorrowFailed.
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

    // Re-derive from the stored canonical bump: a single hash, not a search.
    // This is what proves the escrow belongs to this maker.
    let maker_key = *maker.address();
    let expected = pinocchio_pubkey::derive_address(
        &[b"escrow", maker_key.as_ref(), &[bump]],
        None,
        crate::ID.as_array(),
    );
    if expected != *escrow_account.address().as_array() {
        return Err(ProgramError::InvalidSeeds);
    }

    // Take everything the vault actually holds, not the amount recorded at Make
    // time — the balance on the account is the truth.
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

    // Either destination may not exist yet. CreateIdempotent is safe to call
    // when it does, so the taker never has to know which case it is in.
    pinocchio_associated_token_account::instructions::CreateIdempotent {
        funding_account: taker,
        account: taker_ata_a,
        wallet: taker,
        mint: mint_a,
        token_program,
        system_program,
    }
    .invoke()?;

    pinocchio_associated_token_account::instructions::CreateIdempotent {
        funding_account: taker,
        account: maker_ata_b,
        wallet: maker,
        mint: mint_b,
        token_program,
        system_program,
    }
    .invoke()?;

    {
        let taker_ata_b_state = pinocchio_token::state::Account::from_account_view(taker_ata_b)?;
        if taker_ata_b_state.owner() != taker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if taker_ata_b_state.mint() != mint_b.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    // The taker signed the transaction, so paying the maker is a plain invoke.
    pinocchio_token::instructions::Transfer {
        from: taker_ata_b,
        to: maker_ata_b,
        authority: taker,
        multisig_signers: &[] as &[&AccountView],
        amount: amount_to_receive,
    }
    .invoke()?;

    // The vault answers only to the PDA, so the rest is signed.
    drain_vault_and_close(
        escrow_account,
        vault,
        taker_ata_a,
        maker,
        &maker_key,
        bump,
        vault_amount,
    )
}
