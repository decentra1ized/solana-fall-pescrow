use pinocchio::{
    cpi::{Seed, Signer},
    error::ProgramError,
    AccountView, Address, ProgramResult,
};

use crate::state::Escrow;

pub fn process_cancel_instruction(accounts: &mut [AccountView], _data: &[u8]) -> ProgramResult {
    let [maker, mint_a, escrow_account, vault, maker_ata_a, _token_program @ ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    if !maker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }
    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::IllegalOwner);
    }

    // read escrow state, then drop the borrow
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

    // prove escrow_account is the PDA for this maker
    let bump_bytes = [bump];

    if Address::derive_address(
        &[b"escrow", maker.address().as_ref(), &bump_bytes],
        None,
        &crate::ID,
    ) != *escrow_account.address()
    {
        return Err(ProgramError::InvalidSeeds);
    }

    // validate the vault and the maker's A account
    let vault_amount = {
        let v = pinocchio_token::state::Account::from_account_view(vault)?;

        if v.owner() != escrow_account.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if v.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }

        v.amount()
    };

    {
        let m = pinocchio_token::state::Account::from_account_view(maker_ata_a)?;

        if m.owner() != maker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if m.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    // the escrow PDA authorises the refund
    let seed = [
        Seed::from(b"escrow"),
        Seed::from(maker.address().as_array()),
        Seed::from(&bump_bytes),
    ];
    let signer = Signer::from(&seed);

    // return the deposit to the maker
    pinocchio_token::instructions::Transfer {
        from: vault,
        to: maker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }
    .invoke_signed(&[signer.clone()])?;

    // vault is empty now — hand its rent back to the maker
    pinocchio_token::instructions::CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }
    .invoke_signed(&[signer])?;

    // the escrow is ours, so we close it ourselves
    maker.set_lamports(maker.lamports() + escrow_account.lamports());
    escrow_account.set_lamports(0);
    escrow_account.close()?;

    Ok(())
}
