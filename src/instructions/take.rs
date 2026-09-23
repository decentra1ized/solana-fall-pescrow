/*

ACCOUNT ORDER IS THE API
{
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
    associated_token_program,
}

 */

use crate::state::Escrow;
use pinocchio::cpi::{Seed, Signer};
use pinocchio::error::ProgramError;
use pinocchio::{AccountView, ProgramResult};
use pinocchio_pubkey::derive_address;

pub fn process_take_instruction(accounts: &mut [AccountView], _data: &[u8]) -> ProgramResult {
    //	destructure accounts: &mut [Accounts]
    let [taker, maker, mint_a, mint_b, escrow_account, vault, taker_ata_a, taker_ata_b, maker_ata_b, system_program, token_program, _associated_token_program] =
        accounts
    else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    //	verify taker.is_signer()
    if !taker.is_signer() {
        return Err(ProgramError::IllegalOwner);
    };

    //	load escrow
    Escrow::load_mut(escrow_account)?;

    //	verify escrow program ownership
    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::IllegalOwner);
    };

    let (amount_to_receive, amount_to_give, bump) = {
        let escrow = Escrow::load_mut(escrow_account)?;
        //		verify maker address
        if escrow.maker() != *maker.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        //		verify mint_a address
        if escrow.mint_a() != *mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        //		verify mint_b address
        if escrow.mint_b() != *mint_b.address() {
            return Err(ProgramError::InvalidAccountData);
        }

        (
            escrow.amount_to_receive(),
            escrow.amount_to_give(),
            escrow.bump,
        )
    };

    //	re-derive PDA
    let derived_address = derive_address(
        &[b"escrow", maker.address().as_ref(), &[bump]],
        None,
        &crate::ID.to_bytes(),
    );

    //	check PDA
    if derived_address != escrow_account.address().to_bytes() {
        return Err(ProgramError::InvalidSeeds);
    };

    let vault_amount = {
        let vault_state = pinocchio_token::state::Account::from_account_view(vault)?;

        if vault_state.owner() != escrow_account.address() {
            return Err(ProgramError::IllegalOwner);
        }

        if vault_state.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }

        let vault_amount = vault_state.amount();

        if vault_amount != amount_to_give {
            return Err(ProgramError::InvalidAccountData);
        }

        vault_amount
    };

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

    pinocchio_token::instructions::Transfer {
        amount: amount_to_receive,
        authority: taker,
        from: taker_ata_b,
        to: maker_ata_b,
        multisig_signers: &[] as &[&AccountView],
    }
    .invoke()?;

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
        amount: vault_amount,
    }
    .invoke_signed(&[signer.clone()])?;

    pinocchio_token::instructions::CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }
    .invoke_signed(&[signer.clone()])?;

    maker.set_lamports(maker.lamports() + escrow_account.lamports());
    escrow_account.set_lamports(0);
    escrow_account.close()?;

    Ok(())
}
