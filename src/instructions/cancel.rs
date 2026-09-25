use crate::state::Escrow;
use pinocchio::cpi::{Seed, Signer};
use pinocchio::error::ProgramError;
use pinocchio::{AccountView, ProgramResult};
use pinocchio_pubkey::derive_address;

pub fn process_cancel_instruction(accounts: &mut [AccountView], _data: &[u8]) -> ProgramResult {
    //	destructure accounts: &mut [AccountView]
    let [maker, mint_a, escrow_account, vault, maker_ata_a, _token_program] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    //	check maker.is_signer()
    if !maker.is_signer() {
        return Err(ProgramError::IllegalOwner);
    };

    //	load escrow account
    Escrow::load_mut(escrow_account)?;

    //	verify program ownership
    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::IllegalOwner);
    };

    let (amount_to_give, bump) = {
        let escrow = Escrow::load_mut(escrow_account)?;
        //	verify escrow.maker() == maker.address()
        if escrow.maker() != *maker.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        //  verify escrow.mint_a() == mint_a.address()
        if escrow.mint_a() != *mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }

        (escrow.amount_to_give(), escrow.bump)
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

    //	VALIDATE VAULT
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

    //	BUILD PDA SIGNER
    let bump_bytes = [bump];
    let seed = [
        Seed::from(b"escrow"),
        Seed::from(maker.address().as_array()),
        Seed::from(&bump_bytes),
    ];
    let signer = Signer::from(&seed);

    //	TRANSFER BACK TO TAKER FROM VAULT
    pinocchio_token::instructions::Transfer {
        from: vault,
        to: maker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }
    .invoke_signed(&[signer.clone()])?;

    //	CLOSE PROGRAM ACCOUNT
    pinocchio_token::instructions::CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }
    .invoke_signed(&[signer.clone()])?;

    //	CLOSE ESCROW ACCOUNT - We created it, we close it ourselve (program does not close this for us)
    maker.set_lamports(maker.lamports() + escrow_account.lamports());
    escrow_account.set_lamports(0);
    escrow_account.close()?;

    Ok(())
}
