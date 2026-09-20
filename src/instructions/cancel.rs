use pinocchio::{
    AccountView, ProgramResult, cpi::{Seed, Signer}, error::ProgramError,
};

use crate::state::Escrow;

pub fn process_cancel_instruction(
    accounts: &mut [AccountView],
    _data: &[u8],
) -> ProgramResult {

    let [
        maker,
        mint_a,
        mint_b,
        escrow_account,
        maker_ata,
        escrow_ata,
        _token_program,
        _system_program,
        _associated_token_program@ ..
    ] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    
    // maker must authorize the cancel instruction
    if !maker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    let bump = {
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

        escrow.bump
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

    let vault_amount = {
        let vault = pinocchio_token::state::Account::from_account_view(escrow_ata)?;
        if *vault.owner() != *escrow_account.address() {
            return Err(ProgramError::IllegalOwner);
        }

        if *vault.mint() != *mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        vault.amount()
    };

    {
        let maker_ata_state = pinocchio_token::state::Account::from_account_view(maker_ata)?;
        if *maker_ata_state.owner() != *maker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if *maker_ata_state.mint() != *mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    let bump_bytes = [bump];

    let seed = [
        Seed::from(b"escrow"),
        Seed::from(maker.address().as_ref()),
        Seed::from(&bump_bytes),
    ];

    let seeds = Signer::from(&seed);

    pinocchio_token::instructions::Transfer {
        from: escrow_ata,
        to: maker_ata,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }.invoke_signed(&[seeds.clone()])?;

    pinocchio_token::instructions::CloseAccount {
        account: escrow_account,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }.invoke_signed(&[seeds.clone()])?;

    let escrow_lamports = escrow_account.lamports();

    let new_lamports = maker.lamports().checked_add(escrow_lamports).ok_or(ProgramError::ArithmeticOverflow)?;
    maker.set_lamports(new_lamports);
    escrow_account.set_lamports(0);

    escrow_account.close()?;

    Ok(())
}