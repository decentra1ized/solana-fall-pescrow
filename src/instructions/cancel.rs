use pinocchio::{
    cpi::{Seed, Signer},
    error::ProgramError,
    AccountView,
    Address,
    ProgramResult,
};

use crate::state::Escrow;

/// Cancels an open escrow.
///
/// Only the original maker can cancel. Every Token A in the vault is
/// returned to the maker, and both the vault and escrow accounts close.
///
/// Expected accounts, in this exact order:
///
/// 0. maker          - writable signer and original escrow creator
/// 1. mint_a         - Token A mint
/// 2. escrow_account - writable escrow PDA
/// 3. vault          - writable Token A vault
/// 4. maker_ata_a    - writable maker's Token A account
/// 5. token_program  - SPL Token Program
pub fn process_cancel_instruction(
    accounts: &mut [AccountView],
    _data: &[u8],
) -> ProgramResult {
    let [
        maker,
        mint_a,
        escrow_account,
        vault,
        maker_ata_a,
        _token_program,
        _remaining @ ..
    ] = accounts
    else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    // Cancel returns tokens and rent to the maker, so the maker must
    // authorize the operation with their wallet signature.
    //
    // This is the most important Cancel security check. Without it,
    // a stranger could attempt to close another user's escrow.
    if !maker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    // These accounts will have tokens or lamports changed.
    if !maker.is_writable()
        || !escrow_account.is_writable()
        || !vault.is_writable()
        || !maker_ata_a.is_writable()
    {
        return Err(ProgramError::InvalidAccountData);
    }

    // The escrow state must belong to our program.
    //
    // This prevents a caller from passing a fake account containing
    // attacker-controlled escrow information.
    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::IllegalOwner);
    }

    // Copy the required escrow values into local variables.
    //
    // The scoped block releases the escrow account's data borrow before
    // we perform token CPIs or change the account's lamports.
    let (stored_maker, stored_mint_a, bump) = {
        let escrow_state = Escrow::load_mut(escrow_account)?;

        (
            escrow_state.maker(),
            escrow_state.mint_a(),
            escrow_state.bump,
        )
    };

    // Confirm the signer is the maker recorded inside the escrow.
    if stored_maker != *maker.address() {
        return Err(ProgramError::IllegalOwner);
    }

    // Confirm the supplied mint is the Token A mint stored in the escrow.
    if stored_mint_a != *mint_a.address() {
        return Err(ProgramError::InvalidAccountData);
    }

    // Rebuild and verify the escrow PDA using the original maker and
    // the canonical bump stored when Make created the escrow.
    let bump_bytes = [bump];

    let expected_escrow = Address::create_program_address(
        &[
            b"escrow",
            maker.address().as_ref(),
            &bump_bytes,
        ],
        &crate::ID,
    )
    .map_err(|_| ProgramError::InvalidSeeds)?;

    if expected_escrow != *escrow_account.address() {
        return Err(ProgramError::InvalidSeeds);
    }

    // Validate the vault and copy its complete Token A balance.
    //
    // The full balance must be transferred because a token account
    // cannot be closed while tokens remain inside it.
    let vault_amount = {
        let vault_state =
            pinocchio_token::state::Account::from_account_view(vault)?;

        if vault_state.owner() != escrow_account.address() {
            return Err(ProgramError::IllegalOwner);
        }

        if vault_state.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }

        vault_state.amount()
    };

    // Confirm the destination token account belongs to the maker and
    // holds the same Token A mint used by the vault.
    {
        let maker_ata_a_state =
            pinocchio_token::state::Account::from_account_view(
                maker_ata_a,
            )?;

        if maker_ata_a_state.owner() != maker.address() {
            return Err(ProgramError::IllegalOwner);
        }

        if maker_ata_a_state.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    // Build the escrow PDA signer.
    //
    // A PDA does not have a private key. Solana verifies these seeds
    // and lets our program sign for the escrow PDA through invoke_signed.
    let signer_seed = [
        Seed::from(b"escrow"),
        Seed::from(maker.address().as_array()),
        Seed::from(&bump_bytes),
    ];

    let escrow_signer = Signer::from(&signer_seed);

    // Return every Token A from the vault to the maker.
    pinocchio_token::instructions::Transfer {
        from: vault,
        to: maker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }
    .invoke_signed(&[escrow_signer.clone()])?;

    // The vault is now empty. Close it and return its rent deposit
    // to the maker.
    pinocchio_token::instructions::CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }
    .invoke_signed(&[escrow_signer])?;

    // Close the program-owned escrow state account manually.
    //
    // Its remaining lamports are refunded to the maker. Setting its
    // balance to zero allows the Solana runtime to remove the account.
    let refunded_lamports = maker
        .lamports()
        .checked_add(escrow_account.lamports())
        .ok_or(ProgramError::ArithmeticOverflow)?;

    maker.set_lamports(refunded_lamports);
    escrow_account.set_lamports(0);

    Ok(())
}