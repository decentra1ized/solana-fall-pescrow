use pinocchio::{
    AccountView, ProgramResult,
    cpi::{Seed, Signer},
    error::ProgramError,
};
use pinocchio_associated_token_account::instructions::CreateIdempotent;
use pinocchio_token::instructions::{CloseAccount, Transfer};

use crate::state::Escrow;

/// `Take` — the taker pays the maker's asking price and receives the vault.
///
/// Accounts (all positional, exactly as the client must order them):
///   0.  `[writable, signer]` taker                    — pays fees, funds any missing ATA
///   1.  `[writable]`         maker                    — receives rent refunds; must equal `escrow.maker()`
///   2.  `[]`                 mint_a                   — must equal `escrow.mint_a()`
///   3.  `[]`                 mint_b                   — must equal `escrow.mint_b()`
///   4.  `[writable]`         escrow_account           — PDA ["escrow", maker, bump]; closed here
///   5.  `[writable]`         vault                    — ATA(escrow PDA, mint A); closed here
///   6.  `[writable]`         taker_ata_a              — ATA(taker, mint A); created if missing
///   7.  `[writable]`         taker_ata_b              — ATA(taker, mint B); must exist and be funded
///   8.  `[writable]`         maker_ata_b              — ATA(maker, mint B); created if missing
///   9.  `[]`                 system_program
///   10. `[]`                 token_program
///   11. `[]`                 associated_token_program
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

    // 1. Authorization: only a signing taker may settle a deal.
    if !taker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    // 2. The escrow account must actually be one of ours before we read it as `Escrow`.
    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::InvalidAccountOwner);
    }

    // 3. Read the deal terms. The `RefMut` guard blocks every CPI on this account for as
    //    long as it lives, so it is scoped and only `Copy` primitives leave the block.
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

    // 4. Re-derive the PDA from the stored bump: a single hash instead of the
    //    find_program_address loop `Make` paid for once.
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

    // 6. Make sure both destination ATAs exist. Idempotent: a no-op when they already do.
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

    // The taker's mint B account is supplied, not created: check it is really theirs.
    {
        let taker_ata_b_state = pinocchio_token::state::Account::from_account_view(taker_ata_b)?;
        if taker_ata_b_state.owner() != taker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if taker_ata_b_state.mint() != mint_b.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    // 7. CPI #1 — the taker pays the asking price to the maker. The taker signed the
    //    transaction, so a plain `invoke` is enough.
    Transfer {
        from: taker_ata_b,
        to: maker_ata_b,
        authority: taker,
        multisig_signers: &[] as &[&AccountView],
        amount: amount_to_receive,
    }
    .invoke()?;

    // The escrow PDA signs for everything the vault does.
    let seed = [
        Seed::from(b"escrow"),
        Seed::from(maker.address().as_array()),
        Seed::from(&bump_seed),
    ];
    let signer = Signer::from(&seed);

    // 8. CPI #2 — the vault pays out to the taker.
    Transfer {
        from: vault,
        to: taker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }
    .invoke_signed(&[signer.clone()])?;

    // 9. CPI #3 — the now-empty vault closes, its rent going back to the maker.
    CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }
    .invoke_signed(&[signer])?;

    // 10. Close the escrow state account by hand: move the lamports out first, or the
    //     runtime rejects the instruction as unbalanced, then zero out the account.
    crate::instructions::close_program_account(escrow_account, maker)?;

    Ok(())
}
