use pinocchio::{
    AccountView,
    ProgramResult,
    cpi::{Seed, Signer},
    error::ProgramError,
};
use pinocchio_pubkey::derive_address;

use crate::state::Escrow;

// Accounts:
// 0. taker
// 1. maker
// 2. mint_a
// 3. mint_b
// 4. escrow_account
// 5. vault
// 6. taker_ata_a
// 7. taker_ata_b
// 8. maker_ata_b
// 9. system_program
// 10. token_program
// 11. associated_token_program

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
        _associated_token_program @ ..
    ] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    // The taker must sign the transaction.
    if !taker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    // The escrow account must belong to this program before
    // we trust any state stored inside it.
    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::IllegalOwner);
    }

    // Load the escrow state and cross-check the accounts supplied
    // by the caller against the values stored by Make.
    //
    // Copy the values needed later into locals so the RefMut
    // borrow is released before any CPI involving escrow_account.
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

    // Re-derive the escrow PDA using the stored bump.
    // Unlike Make, Take does not call find_program_address because
    // the canonical bump was already stored in the escrow state.
    let escrow_pda = derive_address(
        &[
            b"escrow",
            maker.address().as_ref(),
            &[bump],
        ],
        None,
        &crate::ID.to_bytes(),
    );

    if escrow_pda != escrow_account.address().to_bytes() {
        return Err(ProgramError::InvalidSeeds);
    }

    // Validate the vault.
    //
    // The vault must be an SPL Token account owned by the escrow PDA
    // and holding mint A.
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

    // Create the taker's ATA for mint A if it does not already exist.
    pinocchio_associated_token_account::instructions::CreateIdempotent {
        funding_account: taker,
        account: taker_ata_a,
        wallet: taker,
        mint: mint_a,
        token_program,
        system_program,
    }
    .invoke()?;

    // Create the maker's ATA for mint B if it does not already exist.
    pinocchio_associated_token_account::instructions::CreateIdempotent {
        funding_account: taker,
        account: maker_ata_b,
        wallet: maker,
        mint: mint_b,
        token_program,
        system_program,
    }
    .invoke()?;

    // Validate the taker's mint-B token account.
    {
        let taker_ata_b_state =
            pinocchio_token::state::Account::from_account_view(taker_ata_b)?;

        if taker_ata_b_state.owner() != taker.address() {
            return Err(ProgramError::IllegalOwner);
        }

        if taker_ata_b_state.mint() != mint_b.address() {
            return Err(ProgramError::InvalidAccountData);
        }

        if taker_ata_b_state.amount() < amount_to_receive {
            return Err(ProgramError::InsufficientFunds);
        }
    }

    // Build the PDA signer.
    //
    // The escrow PDA has no private key. The program proves that it
    // controls the PDA by supplying the same seeds used to derive it.
    let bump_bytes = [bump];

    let seed = [
        Seed::from(b"escrow"),
        Seed::from(maker.address().as_array()),
        Seed::from(&bump_bytes),
    ];

    let signer = Signer::from(&seed);

    // CPI #1:
    // Taker pays the maker amount_to_receive of mint B.
    //
    // The taker signed the transaction, so this is a normal invoke().
    pinocchio_token::instructions::Transfer {
        from: taker_ata_b,
        to: maker_ata_b,
        authority: taker,
        multisig_signers: &[] as &[&AccountView],
        amount: amount_to_receive,
    }
    .invoke()?;

    // CPI #2:
    // The escrow PDA releases all of the A in the vault to the taker.
    //
    // The PDA must sign this transfer.
    pinocchio_token::instructions::Transfer {
        from: vault,
        to: taker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }
    .invoke_signed(&[signer.clone()])?;

    // CPI #3:
    // Close the now-empty vault and refund its rent to the maker.
    //
    // The escrow PDA is the vault authority, so it signs again.
    pinocchio_token::instructions::CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }
    .invoke_signed(&[signer.clone()])?;

    // The escrow account belongs directly to our program, so there
    // is no Token/System CPI needed to close it.
    //
    // First transfer its lamports to the maker so the transaction
    // remains balanced, then close the account.
    maker.set_lamports(maker.lamports() + escrow_account.lamports());
    escrow_account.set_lamports(0);
    escrow_account.close()?;

    Ok(())
}