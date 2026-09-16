use pinocchio::{
    cpi::{Seed, Signer},
    error::ProgramError,
    AccountView, Address, ProgramResult,
};

use crate::state::Escrow;

pub fn process_take_instruction(accounts: &mut [AccountView], _data: &[u8]) -> ProgramResult {
    let [taker, maker, mint_a, mint_b, escrow_account, vault, taker_ata_a, taker_ata_b, maker_ata_b, system_program, token_program, _ata_program @ ..] =
        accounts
    else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    if !taker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }
    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::IllegalOwner);
    }

    //read escrow state, then drop the borrow
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

    // validate the vault and the taker's B account
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
        let t = pinocchio_token::state::Account::from_account_view(taker_ata_b)?;

        if t.owner() != taker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if t.mint() != mint_b.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        if t.amount() < amount_to_receive {
            return Err(ProgramError::InsufficientFunds);
        }
    }

    // make sure both ATAs exist
    pinocchio_associated_token_account::instructions::CreateIdempotent {
        funding_account: taker,
        account: taker_ata_a,
        wallet: taker,
        mint: mint_a,
        system_program,
        token_program,
    }
    .invoke()?;

    pinocchio_associated_token_account::instructions::CreateIdempotent {
        funding_account: taker,
        account: maker_ata_b,
        wallet: maker,
        mint: mint_b,
        system_program,
        token_program,
    }
    .invoke()?;

    // transfer B token from taker's ATA to maker's ATA
    pinocchio_token::instructions::Transfer {
        from: taker_ata_b,
        to: maker_ata_b,
        authority: taker,
        multisig_signers: &[] as &[&AccountView],
        amount: amount_to_receive,
    }
    .invoke()?;

    // the escrow PDA authorises the vault payout
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
