use pinocchio::{
    cpi::{Seed, Signer},
    error::ProgramError,
    AccountView,
    Address,
    ProgramResult,
};

use crate::state::Escrow;

/// Completes an escrow trade.
///
/// The taker gives Token B to the maker.
/// In return, the taker receives every Token A stored in the vault.
///
/// Expected accounts, in this exact order:
///
/// 0. taker                    - writable signer paying Token B
/// 1. maker                    - writable original escrow creator
/// 2. mint_a                   - Token A mint
/// 3. mint_b                   - Token B mint
/// 4. escrow_account           - writable escrow PDA
/// 5. vault                    - writable Token A vault
/// 6. taker_ata_a              - writable taker's Token A account
/// 7. taker_ata_b              - writable taker's Token B account
/// 8. maker_ata_b              - writable maker's Token B account
/// 9. system_program           - Solana System Program
/// 10. token_program           - SPL Token Program
/// 11. associated_token_program - Associated Token Account Program
pub fn process_take_instruction(
    accounts: &mut [AccountView],
    _data: &[u8],
) -> ProgramResult {
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
        _remaining @ ..
    ] = accounts
    else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    // The taker is spending Token B, so the taker must authorize
    // this transaction with their wallet signature.
    if !taker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    // These accounts will have tokens or lamports changed.
    // Reject the transaction if the client marked any of them read-only.
    if !taker.is_writable()
        || !maker.is_writable()
        || !escrow_account.is_writable()
        || !vault.is_writable()
        || !taker_ata_a.is_writable()
        || !taker_ata_b.is_writable()
        || !maker_ata_b.is_writable()
    {
        return Err(ProgramError::InvalidAccountData);
    }

    // The escrow state must belong to this program.
    // Otherwise, someone could supply fake escrow data.
    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::IllegalOwner);
    }

    // Read the escrow information into ordinary local variables.
    //
    // This block is important. Escrow::load_mut() borrows the account
    // data. When the block ends, the borrow is released before any CPI
    // tries to use the escrow account.
    let (
        stored_maker,
        stored_mint_a,
        stored_mint_b,
        amount_to_receive,
        bump,
    ) = {
        let escrow_state = Escrow::load_mut(escrow_account)?;

        (
            escrow_state.maker(),
            escrow_state.mint_a(),
            escrow_state.mint_b(),
            escrow_state.amount_to_receive(),
            escrow_state.bump,
        )
    };

    // Confirm the supplied maker is the person who created this escrow.
    if stored_maker != *maker.address() {
        return Err(ProgramError::InvalidAccountData);
    }

    // Confirm both supplied token mints match the escrow agreement.
    if stored_mint_a != *mint_a.address()
        || stored_mint_b != *mint_b.address()
    {
        return Err(ProgramError::InvalidAccountData);
    }

    // Rebuild the escrow PDA using the exact seeds stored by Make.
    //
    // The stored bump lets us validate one specific address instead of
    // searching through every possible bump again.
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

    // Validate the vault and copy its complete token balance.
    //
    // We transfer the actual vault balance instead of only the stored
    // amount. The vault must be completely empty before it can close.
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

    // Confirm the taker's Token B account actually belongs to the taker
    // and holds the correct mint.
    {
        let taker_ata_b_state =
            pinocchio_token::state::Account::from_account_view(
                taker_ata_b,
            )?;

        if taker_ata_b_state.owner() != taker.address() {
            return Err(ProgramError::IllegalOwner);
        }

        if taker_ata_b_state.mint() != mint_b.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    // Create the taker's Token A account if it does not already exist.
    //
    // CreateIdempotent is safe to call when the account already exists,
    // so the client does not need separate "check then create" logic.
    pinocchio_associated_token_account::instructions::CreateIdempotent {
        funding_account: taker,
        account: taker_ata_a,
        wallet: taker,
        mint: mint_a,
        system_program,
        token_program,
    }
    .invoke()?;

    // Create the maker's Token B account if it does not already exist.
    //
    // The taker funds this account because the taker is completing
    // the trade and is the transaction's writable signer.
    pinocchio_associated_token_account::instructions::CreateIdempotent {
        funding_account: taker,
        account: maker_ata_b,
        wallet: maker,
        mint: mint_b,
        system_program,
        token_program,
    }
    .invoke()?;

    // Validate the taker's newly created or existing Token A account.
    {
        let taker_ata_a_state =
            pinocchio_token::state::Account::from_account_view(
                taker_ata_a,
            )?;

        if taker_ata_a_state.owner() != taker.address() {
            return Err(ProgramError::IllegalOwner);
        }

        if taker_ata_a_state.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    // Validate the maker's newly created or existing Token B account.
    {
        let maker_ata_b_state =
            pinocchio_token::state::Account::from_account_view(
                maker_ata_b,
            )?;

        if maker_ata_b_state.owner() != maker.address() {
            return Err(ProgramError::IllegalOwner);
        }

        if maker_ata_b_state.mint() != mint_b.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    // First half of the trade:
    //
    // Move the agreed amount of Token B from the taker to the maker.
    // This uses invoke() because the taker signed the transaction normally.
    pinocchio_token::instructions::Transfer {
        from: taker_ata_b,
        to: maker_ata_b,
        authority: taker,
        multisig_signers: &[] as &[&AccountView],
        amount: amount_to_receive,
    }
    .invoke()?;

    // Build the escrow PDA's signer seeds.
    //
    // A PDA has no private key. invoke_signed() lets the Solana runtime
    // confirm these seeds recreate the escrow address and temporarily
    // authorize the PDA to control its vault.
    let signer_seed = [
        Seed::from(b"escrow"),
        Seed::from(maker.address().as_array()),
        Seed::from(&bump_bytes),
    ];

    let escrow_signer = Signer::from(&signer_seed);

    // Second half of the trade:
    //
    // The escrow PDA sends every Token A in the vault to the taker.
    pinocchio_token::instructions::Transfer {
        from: vault,
        to: taker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }
    .invoke_signed(&[escrow_signer.clone()])?;

    // The vault is now empty, so close it and return its rent deposit
    // to the maker who originally created the escrow.
    pinocchio_token::instructions::CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }
    .invoke_signed(&[escrow_signer])?;

    // Close the program-owned escrow state account manually.
    //
    // Moving all its lamports back to the maker leaves the escrow
    // account empty, allowing the runtime to remove it.
    let refunded_lamports = maker
        .lamports()
        .checked_add(escrow_account.lamports())
        .ok_or(ProgramError::ArithmeticOverflow)?;

    maker.set_lamports(refunded_lamports);
    escrow_account.set_lamports(0);

    Ok(())
}