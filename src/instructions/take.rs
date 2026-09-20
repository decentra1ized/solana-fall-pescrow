use pinocchio::{
    AccountView, ProgramResult, cpi::{Seed, Signer}, error::ProgramError,
};

use crate::state::Escrow;

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
        taker_ata_a,
        taker_ata_b,
        maker_ata_b,
        escrow_ata,
        token_program,
        system_program,
        _associated_token_program@ ..
    ] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    // Put the snippet HERE
    if !taker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

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
    }; // ← RefMut guard dropped here
    
    // verify pda using stored bump
    let derived_escrow = pinocchio_pubkey::derive_address(
        &[
            b"escrow",
            maker.address().as_ref(),
            &[bump],
            ],
            None,
            &crate::ID.to_bytes(),
        );

        if derived_escrow.as_ref() != escrow_account.address().as_ref() {
            return Err(ProgramError::InvalidSeeds);
        }
    
    // validate escrow vault
    let vault_amount = {
        let vault =
        pinocchio_token::state::Account::from_account_view(escrow_ata)?;

        if vault.owner() != escrow_account.address() {
            return Err(ProgramError::IllegalOwner);
        }
        
        if vault.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }

        vault.amount()
    };

    // Create destination AtAs idempotently
    // taker_ata_a: taker's mint A ATA
    // maker_ata_b: taker's mint B ATA
    pinocchio_associated_token_account::instructions::CreateIdempotent {
        funding_account: taker,
        account: taker_ata_b,
        wallet: taker,
        mint: mint_b,
        token_program: token_program,
        system_program: system_program,
    }.invoke()?;

    pinocchio_associated_token_account::instructions::CreateIdempotent {
        funding_account: taker,
        account: taker_ata_a,
        wallet: taker,
        mint: mint_a,
        token_program: token_program,
        system_program: system_program,
    }.invoke()?;
    
    // validate taker's mint B ATA
    {
        let taker_ata_b_state = pinocchio_token::state::Account::from_account_view(taker_ata_b)?;
        
        if taker_ata_b_state.owner() != maker.address() {
            return Err(ProgramError::IllegalOwner);
        }

        if taker_ata_b_state.mint() != mint_b.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    // validate maker's mint B ATA
    {
        let maker_ata_b_state = pinocchio_token::state::Account::from_account_view(maker_ata_b)?;
        
        if maker_ata_b_state.owner() != taker.address() {
            return Err(ProgramError::IllegalOwner);
        }

        if maker_ata_b_state.mint() != mint_b.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    pinocchio_token::instructions::Transfer {
        from: taker_ata_b,
        to: maker_ata_b,
        authority: taker,
        multisig_signers: &[] as &[&AccountView],
        amount: amount_to_receive,
    }.invoke()?;

    let bump_bytes = [bump];

    let seed = [Seed::from(b"escrow"), Seed::from(maker.address().as_array()), Seed::from(&bump_bytes)];
    let seeds = Signer::from(&seed);

    pinocchio_token::instructions::Transfer {
        from: escrow_ata,
        to: taker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }.invoke_signed(&[seeds.clone()])?;

    
    let escrow_lamports = escrow_account.lamports();

    let new_lamports = maker.lamports().checked_add(escrow_lamports).ok_or(ProgramError::ArithmeticOverflow)?;
    maker.set_lamports(new_lamports);
    escrow_account.set_lamports(0);

    escrow_account.close()?;

    Ok(())
}