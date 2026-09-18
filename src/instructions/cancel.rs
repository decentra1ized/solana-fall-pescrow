use pinocchio::{
    AccountView, cpi::{Seed, Signer}, error::ProgramError, Address, ProgramResult,
};

use crate::state::Escrow;

pub fn process_cancel_instruction(accounts: &mut [AccountView]) -> ProgramResult {
    // Accounts: maker, mint_a, escrow_account, vault, maker_ata_a, token_program
    let [
        maker,
        mint_a,
        escrow_account,
        vault,
        maker_ata_a,
        _token_program,
    ] = accounts
    else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    // The critical authorization check
    if !maker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    if escrow_account.owned_by(&crate::ID) == false {
        return Err(ProgramError::InvalidAccountData);
    }

    // 1. Load escrow, verify maker & mint_a match the stored state, pull bump, drop borrow
    let bump = {
        let escrow = Escrow::load_mut(escrow_account)?;
        if escrow.maker() != *maker.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        if escrow.mint_a() != *mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        escrow.bump
    }; // <- RefMut guard dropped here

    // 2. Re-derive the PDA
    let (escrow_pda, _) =
        Address::find_program_address(&[b"escrow", maker.address().as_ref()], &crate::ID);
    if escrow_pda != *escrow_account.address() {
        return Err(ProgramError::InvalidSeeds);
    }

    // Validate the vault, read its balance
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

    // 3. Build the PDA signer
    let bump_bytes = [bump];
    let seed = [
        Seed::from(b"escrow"),
        Seed::from(maker.address().as_ref()),
        Seed::from(&bump_bytes),
    ];
    let signer = Signer::from(&seed);

    // 4. Vault pays maker back
    pinocchio_token::instructions::Transfer {
        from: vault,
        to: maker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }
    .invoke_signed(&[signer.clone()])?;

    // 5. Close the vault
    pinocchio_token::instructions::CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }
    .invoke_signed(&[signer.clone()])?;

    // 6. Close the escrow account, by hand
    maker.set_lamports(maker.lamports() + escrow_account.lamports());
    escrow_account.set_lamports(0);
    escrow_account.close()?;

    Ok(())
}
