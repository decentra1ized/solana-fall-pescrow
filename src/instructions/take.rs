use pinocchio::{
    AccountView, ProgramResult,
    cpi::{Seed, Signer},
    error::ProgramError,
};

use crate::instructions::shared::drain_and_close;
use crate::state::Escrow;

/// Account order (the account order is the API; there is no IDL):
///
/// |  # | Account                   | Writable | Signer |
/// | -- | ------------------------- | -------- | ------ |
/// |  0 | taker                     | yes      | yes    |
/// |  1 | maker                     | yes      |        |
/// |  2 | mint_a                    |          |        |
/// |  3 | mint_b                    |          |        |
/// |  4 | escrow_account            | yes      |        |
/// |  5 | vault                     | yes      |        |
/// |  6 | taker_ata_a               | yes      |        |
/// |  7 | taker_ata_b               | yes      |        |
/// |  8 | maker_ata_b               | yes      |        |
/// |  9 | system_program            |          |        |
/// | 10 | token_program             |          |        |
/// | 11 | associated_token_program  |          |        |
///
/// Instruction data: the discriminator byte only. Everything else is in state.
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
        _associated_token_program,
        ..
    ] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    if !taker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    // Trust the escrow data only after the owner check. Without it, anyone can
    // pass a look-alike account with a maker of their choice.
    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::IllegalOwner);
    }

    // Cross-check the passed accounts against the stored deal, copy out what the
    // CPIs need, then let the guard drop before the first invoke.
    let (amount_to_receive, bump) = {
        let escrow_state = Escrow::load_mut(escrow_account)?;
        if escrow_state.maker() != *maker.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        if escrow_state.mint_a() != *mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        if escrow_state.mint_b() != *mint_b.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        (escrow_state.amount_to_receive(), escrow_state.bump)
    };

    // Copy the maker address into a local. The seeds below would otherwise hold
    // a borrow of `maker`, which must stay free for the rent refund.
    let maker_key = maker.address().to_bytes();
    let bump_bytes = [bump];

    // The bump is the canonical one Make stored, so this is a single hash and
    // not a `find_program_address` loop.
    let seeds: [&[u8]; 3] = [b"escrow", &maker_key, &bump_bytes];
    if pinocchio_pubkey::derive_address(&seeds, None, &crate::ID.to_bytes())
        != escrow_account.address().to_bytes()
    {
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

    // The two destination token accounts may not exist yet. `CreateIdempotent`
    // is safe to call when they do.
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

    // CPI 1: the taker pays the maker. The taker signed the transaction, so a
    // plain invoke is enough.
    pinocchio_token::instructions::Transfer {
        from: taker_ata_b,
        to: maker_ata_b,
        authority: taker,
        multisig_signers: &[] as &[&AccountView],
        amount: amount_to_receive,
    }.invoke()?;

    // CPI 2 and 3 plus the manual close: the vault pays the taker, then both
    // program accounts go away and the maker gets the rent back.
    let seed = [
        Seed::from(b"escrow"),
        Seed::from(&maker_key),
        Seed::from(&bump_bytes),
    ];
    let signer = Signer::from(&seed);

    drain_and_close(vault, taker_ata_a, escrow_account, maker, vault_amount, &[signer])
}
