use pinocchio::{
    AccountView, ProgramResult,
    cpi::{Seed, Signer},
    error::ProgramError,
};

use crate::instructions::take::close_account;
use crate::state::Escrow;

/// The maker reclaims their deposit and tears the offer down.
///
/// Only the maker recorded in the escrow may do this. Everything in the vault goes
/// back to the maker's own token-A account, and both the vault and the escrow are
/// closed with their rent refunded.
pub fn process_cancel_instruction(accounts: &mut [AccountView], _data: &[u8]) -> ProgramResult {
    let [
        maker,
        mint_a,
        escrow_account,
        vault,
        maker_ata_a,
        token_program @ ..
    ] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    let _ = token_program;

    if !maker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::IllegalOwner);
    }

    // Read the state, check it belongs to this signer, then drop the guard before
    // the CPIs below.
    let bump = {
        let escrow_state = Escrow::load_mut(escrow_account)?;

        // This is the check that stops a stranger cancelling somebody else's escrow
        // and walking off with the deposit. The signature check above only proves
        // *someone* signed; this proves it was the maker.
        if escrow_state.maker() != *maker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if escrow_state.mint_a() != *mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }

        escrow_state.bump
    };

    let expected_escrow = pinocchio_pubkey::derive_address(
        &[b"escrow", maker.address().as_ref(), &[bump]],
        None,
        crate::ID.as_array(),
    );
    if &expected_escrow != escrow_account.address().as_array() {
        return Err(ProgramError::InvalidSeeds);
    }

    // Validate both token accounts before moving anything.
    //
    // The vault's whole balance is returned, not the recorded `amount_to_give`. They
    // are normally identical, but draining the account is what lets the CloseAccount
    // below succeed: the token program refuses to close a non-empty account, so dust
    // sent to the vault by anyone would otherwise strand the escrow forever.
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

    {
        let maker_ata_state = pinocchio_token::state::Account::from_account_view(maker_ata_a)?;
        if maker_ata_state.owner() != maker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if maker_ata_state.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    let bump_bytes = [bump];
    let seed = [
        Seed::from(b"escrow"),
        Seed::from(maker.address().as_array()),
        Seed::from(&bump_bytes),
    ];
    let signer = Signer::from(&seed);

    pinocchio_token::instructions::Transfer {
        from: vault,
        to: maker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }
    .invoke_signed(&[signer.clone()])?;

    pinocchio_token::instructions::CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }
    .invoke_signed(&[signer])?;

    close_account(escrow_account, maker)
}
