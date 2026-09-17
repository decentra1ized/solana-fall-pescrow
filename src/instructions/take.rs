use pinocchio::{
    AccountView, ProgramResult,
    cpi::{Seed, Signer},
    error::ProgramError,
};

use crate::state::Escrow;

/// The taker fulfils the maker's offer.
///
/// Token B moves taker -> maker, token A moves vault -> taker, and both the vault and
/// the escrow account are closed with their rent refunded to the maker. Either the
/// whole exchange happens or none of it does, which is the entire point of an escrow.
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

    // The escrow must be one of ours; otherwise its "state" is whatever bytes an
    // attacker put in an account they control.
    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::IllegalOwner);
    }

    // Read what we need into locals and drop the guard before any CPI below: the
    // runtime refuses a CPI touching an account whose data is still borrowed.
    let (amount_to_receive, amount_to_give, bump) = {
        let escrow_state = Escrow::load_mut(escrow_account)?;

        // The accounts passed in must be the ones this escrow was created for.
        // Without these checks a taker could pass a different maker (and be handed
        // the tokens) or a different mint pair (and pay in a worthless token).
        if escrow_state.maker() != *maker.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        if escrow_state.mint_a() != *mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        if escrow_state.mint_b() != *mint_b.address() {
            return Err(ProgramError::InvalidAccountData);
        }

        (
            escrow_state.amount_to_receive(),
            escrow_state.amount_to_give(),
            escrow_state.bump,
        )
    };

    // Re-derive with the stored bump rather than `find_program_address`: one hash
    // instead of a search loop, and it proves the passed account really is this
    // maker's escrow PDA.
    let expected_escrow = pinocchio_pubkey::derive_address(
        &[b"escrow", maker.address().as_ref(), &[bump]],
        None,
        crate::ID.as_array(),
    );
    if &expected_escrow != escrow_account.address().as_array() {
        return Err(ProgramError::InvalidSeeds);
    }

    // The vault must be the escrow's own token-A account, not some account the taker
    // controls. Scoped so the borrow is released before the CPIs.
    {
        let vault_state = pinocchio_token::state::Account::from_account_view(vault)?;
        if vault_state.owner() != escrow_account.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if vault_state.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        // The vault should hold what the maker promised. If it somehow holds less,
        // fail rather than hand the taker a partial fill they did not agree to.
        if vault_state.amount() < amount_to_give {
            return Err(ProgramError::InsufficientFunds);
        }
    }

    // The taker may not have an account for token A yet, and the maker may not have
    // one for token B. Idempotent creation succeeds whether or not they exist.
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

    // Taker pays the maker first. The taker signs for this one themselves, so a plain
    // invoke is enough. If the taker is short on token B this fails here and nothing
    // has moved out of the vault yet.
    pinocchio_token::instructions::Transfer {
        from: taker_ata_b,
        to: maker_ata_b,
        authority: taker,
        multisig_signers: &[] as &[&AccountView],
        amount: amount_to_receive,
    }
    .invoke()?;

    // The vault's authority is the escrow PDA, so the program signs on its behalf.
    let bump_bytes = [bump];
    let seed = [
        Seed::from(b"escrow"),
        Seed::from(maker.address().as_array()),
        Seed::from(&bump_bytes),
    ];
    let signer = Signer::from(&seed);

    pinocchio_token::instructions::Transfer {
        from: vault,
        to: taker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: amount_to_give,
    }
    .invoke_signed(&[signer.clone()])?;

    // The vault is empty now; close it and refund its rent to the maker, who paid it.
    pinocchio_token::instructions::CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }
    .invoke_signed(&[signer])?;

    // The escrow account is ours, not the token program's, so we close it by hand:
    // move every lamport to the maker, zero our own, then clear the account. Moving
    // the lamports first is required — closing with a balance is an unbalanced
    // instruction and the runtime rejects the whole transaction.
    close_account(escrow_account, maker)
}

/// Moves an account's lamports to `destination` and closes it.
///
/// Shared with `Cancel`: both instructions finish by tearing down the escrow the same
/// way, and a single copy means the lamport-before-close ordering is only stated once.
pub fn close_account(account: &mut AccountView, destination: &mut AccountView) -> ProgramResult {
    let lamports = account.lamports();
    let new_destination_balance = destination
        .lamports()
        .checked_add(lamports)
        .ok_or(ProgramError::ArithmeticOverflow)?;

    destination.set_lamports(new_destination_balance);
    account.set_lamports(0);
    account.close()
}
