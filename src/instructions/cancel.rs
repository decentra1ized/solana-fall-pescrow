//! Cancel — discriminator 2. The maker changes their mind and takes the A back.
//!
//! Account order. There is no IDL, so this comment *is* the API — keep the tests
//! in step with it.
//!
//!   0  maker            signer, writable   must equal escrow.maker(); gets the A and both rents
//!   1  mint_a                              must equal escrow.mint_a()
//!   2  escrow_account   writable           PDA ["escrow", maker], closed here
//!   3  vault            writable           ATA(escrow PDA, mint A), closed here
//!   4  maker_ata_a      writable           the A goes back here
//!   5  token_program
//!
//! Instruction data is the discriminator alone.
//!
//! On the checks, and which of them is actually load-bearing — measured, not
//! assumed (see the note in `tests/mod.rs`):
//!
//!   * `maker.is_signer()` is the one that stands alone. Remove it and a stranger
//!     can cancel any escrow by naming its real maker: the seeds are then correct,
//!     every other check passes, and the transaction succeeds. The tokens still go
//!     home to the maker, so the loss is griefing rather than theft — but it is
//!     griefing anyone can do to everyone, and nothing else in this instruction
//!     catches it.
//!
//!   * `escrow.maker() == maker.address()` and the PDA re-derivation are layers on
//!     a fact the runtime already enforces. The maker's address is one of the PDA
//!     seeds, so "the stored maker matches", "the derived address matches" and
//!     "`invoke_signed` yields the vault authority's signature" are the same
//!     statement three times. A stranger who passes themselves as the maker signs
//!     with seeds that derive some other PDA, and the token program rejects the
//!     transfer as an unauthorized signer even with both checks deleted.
//!
//! They stay because redundancy here is nearly free and buys two things: a caller
//! gets `InvalidAccountData` or `InvalidSeeds`, which name the problem, instead of
//! a bare "unauthorized signer" from a CPI three frames down; and the day anyone
//! changes the seed scheme — an `escrow_id` seed, say, to allow several escrows
//! per maker — the stored-maker check stops being redundant and starts being the
//! only thing tying this escrow to this signer.

use pinocchio::{AccountView, ProgramResult, error::ProgramError};

use crate::instructions::settle::{check_token_account, load_and_verify_escrow, settle_and_close};

pub fn process_cancel_instruction(accounts: &mut [AccountView], _data: &[u8]) -> ProgramResult {
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

    if !maker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    // Mint B plays no part here, so there is no account to check it against; the
    // escrow keeps whatever Make recorded and it is never read.
    let (_amount_to_receive, bump) = load_and_verify_escrow(escrow_account, maker, mint_a, None)?;

    let vault_amount = check_token_account(vault, escrow_account, mint_a)?;

    // The maker's own account has to be theirs and hold mint A. Make created it
    // before the deposit, so unlike Take there is nothing to create here.
    check_token_account(maker_ata_a, maker, mint_a)?;

    // The same ending as Take, pointed the other way: the vault goes back to the
    // maker instead of on to a taker.
    settle_and_close(
        escrow_account,
        vault,
        maker_ata_a,
        maker,
        vault_amount,
        bump,
    )
}
