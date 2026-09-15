use pinocchio:: {
    cpi::{Seed,Signer},
    error::ProgramError,
    AccountView,
    ProgramResult,
};

use pinocchio_pubkey::derive_address;

use crate::state::Escrow;

pub fn process_take_instruction(accounts: &mut[AccountView], _data: &[u8]) -> ProgramResult{
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
        _associated_token_program
    ] = accounts 
    else{
       return Err(ProgramError::NotEnoughAccountKeys);
    };

    //  taker must sign
    if !taker.is_signer(){
        return Err(ProgramError::MissingRequiredSignature);
    }

    //escrow must belong to our program
    if !escrow_account.owned_by(&crate::ID){
        return Err(ProgramError::IllegalOwner);
    }

    //Read + validate escrow state
    let (amount_to_receive, bump) = {
        let escrow_state = Escrow::load_mut(escrow_account)?;

        if escrow_state.maker() != *maker.address(){
            return Err(ProgramError::InvalidAccountData);
        }

        if *mint_a.address() != escrow_state.mint_a(){
            return Err(ProgramError::InvalidAccountData);
        }

        if *mint_b.address() != escrow_state.mint_b(){
            return Err(ProgramError::InvalidAccountData);
        }

        (escrow_state.amount_to_receive(), escrow_state.bump)
    };

    //Re-derive and verify escrow PDA using stored bump 

    let bump_bytes = [bump];

    let expected_escrow = derive_address(
        &[b"escrow", maker.address().as_ref(), &bump_bytes],
        None,
        &crate::ID.to_bytes(),
    );

    if expected_escrow != escrow_account.address().to_bytes(){
        return Err(ProgramError::InvalidSeeds);
    }

    //Vault must belong to escrow PDA and hold mint a 
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

    //validate taker's mint b 

    {
        let taker_ata_b_state = pinocchio_token::state::Account::from_account_view(taker_ata_b)?;

        if taker_ata_b_state.owner() != taker.address() {
            return Err(ProgramError::IllegalOwner);
        }

        if taker_ata_b_state.mint() != mint_b.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    //creating taker's mint a ata 
    pinocchio_associated_token_account::instructions::CreateIdempotent {
        funding_account: taker,
        account: taker_ata_a,
        wallet: taker,
        mint: mint_a,
        system_program,
        token_program,
    }
    .invoke()?;

    //create maker's mint b ata 
    pinocchio_associated_token_account::instructions::CreateIdempotent {
        funding_account: taker,
        account: maker_ata_b,
        wallet: maker,
        mint: mint_b,
        system_program,
        token_program,
    }
    .invoke()?;


    //taker pays mint b to maker
     pinocchio_token::instructions::Transfer {
        from: taker_ata_b,
        to: maker_ata_b,
        authority: taker,
        multisig_signers: &[] as &[&AccountView],
        amount: amount_to_receive,
    }
    .invoke()?;

    //build escrow pda signer 
    let seeds = [
        Seed::from(b"escrow"),
        Seed::from(maker.address().as_array()),
        Seed::from(&bump_bytes),
    ];

    let signer = Signer::from(&seeds);

    //transfer mint a from vault to taker
     pinocchio_token::instructions::Transfer {
        from: vault,
        to: taker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }
    .invoke_signed(&[signer.clone()])?;

    //close vault
      pinocchio_token::instructions::CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }
    .invoke_signed(&[signer])?;

    //close escrow manually , no cpi required since the account belongs to our program 
    maker.set_lamports(maker.lamports() + escrow_account.lamports());

    escrow_account.set_lamports(0);

    escrow_account.close()?;

    Ok(())

}