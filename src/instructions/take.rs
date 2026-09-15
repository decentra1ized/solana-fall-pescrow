//! Take — discriminator 1. The taker pays the asking price and the deal closes.
//!
//! Account order. There is no IDL, so this comment *is* the API — keep the tests
//! in step with it.
//!
//!   0   taker                      signer, writable   pays the fees and any rent
//!   1   maker                      writable           gets paid, and both rents back
//!   2   mint_a                                        must equal escrow.mint_a()
//!   3   mint_b                                        must equal escrow.mint_b()
//!   4   escrow_account             writable           PDA ["escrow", maker], closed here
//!   5   vault                      writable           ATA(escrow PDA, mint A), closed here
//!   6   taker_ata_a                writable           gets the A; created if missing
//!   7   taker_ata_b                writable           pays the B; must already exist
//!   8   maker_ata_b                writable           gets the B; created if missing
//!   9   system_program
//!   10  token_program
//!   11  associated_token_program
//!
//! Instruction data is the discriminator alone. Every term of the deal is already
//! in the escrow account, written there by Make — a taker who could restate the
//! amounts could restate the price.

use pinocchio::{AccountView, ProgramResult, error::ProgramError};
use pinocchio_associated_token_account::instructions::CreateIdempotent;
use pinocchio_token::instructions::Transfer;

use crate::instructions::settle::{check_token_account, load_and_verify_escrow, settle_and_close};

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

    // Ownership, stored maker, both mints, and the PDA derivation. Returns the
    // price and the canonical bump with the borrow already released.
    let (amount_to_receive, bump) =
        load_and_verify_escrow(escrow_account, maker, mint_a, Some(mint_b))?;

    // How much A the taker gets. Read from the vault, not from the escrow's
    // amount_to_give — see the note on `settle_and_close`.
    let vault_amount = check_token_account(vault, escrow_account, mint_a)?;

    // The two destinations may not exist yet: the taker may never have held A, and
    // the maker may never have held B. Idempotent, so passing an existing account
    // is fine. The taker funds them — they are the one who wants this trade.
    CreateIdempotent {
        funding_account: taker,
        account: taker_ata_a,
        wallet: taker,
        mint: mint_a,
        system_program,
        token_program,
    }
    .invoke()?;

    CreateIdempotent {
        funding_account: taker,
        account: maker_ata_b,
        wallet: maker,
        mint: mint_b,
        system_program,
        token_program,
    }
    .invoke()?;

    // The source of B is the taker's to prove. CreateIdempotent vouches for the two
    // accounts above by deriving them; this one arrives unchecked.
    check_token_account(taker_ata_b, taker, mint_b)?;

    // CPI 1 — the taker pays the maker. The taker signed the transaction, so this
    // is a plain invoke: no PDA is involved on this side of the trade.
    Transfer {
        from: taker_ata_b,
        to: maker_ata_b,
        authority: taker,
        multisig_signers: &[] as &[&AccountView],
        amount: amount_to_receive,
    }
    .invoke()?;

    // CPIs 2 and 3, plus the escrow close: the vault pays the taker, and everything
    // winds up with the rent going back to the maker.
    settle_and_close(
        escrow_account,
        vault,
        taker_ata_a,
        maker,
        vault_amount,
        bump,
    )
}
