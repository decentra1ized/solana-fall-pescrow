// Cancel: the maker takes back the deposit before anyone takes the deal.
// Instruction data is the discriminator `2` only.
//
// Account order is the API here: there is no IDL, so the test must pass these in
// exactly this order. This handler matches on exactly six accounts, no trailing rest.
//
//   #   Account          W  S  Notes
//   0   maker            x  x  Must sign. Must equal escrow.maker()
//   1   mint_a                 Must equal escrow.mint_a()
//   2   escrow_account   x     PDA ["escrow", maker], owned by us, closed here
//   3   vault            x     ATA(escrow PDA, mint A), closed here
//   4   maker_ata_a      x     Destination for the returned A
//   5   token_program
use pinocchio::{
    cpi::{Seed, Signer},
    error::ProgramError,
    AccountView, ProgramResult,
};

use crate::state::Escrow;

pub fn process_cancel_instruction(accounts: &mut [AccountView], _: &[u8]) -> ProgramResult {
    let [maker, mint_a, escrow_account, vault, maker_ata_a, _token_program] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    if !maker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::IllegalOwner);
    }

    // Cross-check the passed accounts against the terms stored at Make.
    let bump = {
        let escrow = Escrow::load_mut(escrow_account)?;
        if escrow.maker() != *maker.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        if escrow.mint_a() != *mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        escrow.bump
    };

    // Re-derive the PDA from the bump Make stored: a single hash, no search.
    let escrow_pda = pinocchio_pubkey::derive_address(
        &[b"escrow", maker.address().as_ref(), &[bump]],
        None,
        &crate::ID.to_bytes(),
    );
    if escrow_pda != *escrow_account.address().as_array() {
        return Err(ProgramError::InvalidSeeds);
    }

    // Validate the vault: owner = escrow PDA, mint = mint A. Its balance goes back to the maker.
    let vault_amount;
    {
        let vault = pinocchio_token::state::Account::from_account_view(vault)?;
        if vault.owner() != escrow_account.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if vault.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        vault_amount = vault.amount();
    }

    // Validate maker_ata_a: owner = maker, mint = mint A.
    {
        let maker_ata_a_state = pinocchio_token::state::Account::from_account_view(maker_ata_a)?;
        if maker_ata_a_state.owner() != maker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if maker_ata_a_state.mint() != mint_a.address() {
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

    // The vault returns the A to the maker. Its authority is the escrow PDA, so this must be signed.
    pinocchio_token::instructions::Transfer {
        from: vault,
        to: maker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }
    .invoke_signed(&[signer.clone()])?;

    // An emptied token account still holds rent, so the token program closes it and
    // refunds the maker, who paid for it at Make.
    pinocchio_token::instructions::CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }
    .invoke_signed(&[signer.clone()])?;

    // The escrow account is owned by this program, so no CPI: move its lamports out first,
    // or the runtime rejects the instruction as unbalanced.
    maker.set_lamports(maker.lamports() + escrow_account.lamports());
    escrow_account.set_lamports(0);
    escrow_account.close()?;

    Ok(())
}
