use pinocchio::{
    AccountView,
    ProgramResult,
    cpi::{Seed, Signer},
    error::ProgramError,
};

use pinocchio_pubkey::derive_address;

use crate::state::Escrow;

pub fn process_take_instruction(
    accounts: &mut [AccountView],
) -> ProgramResult {

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
        _associated_token_program,
    ] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    if !taker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::IllegalOwner);
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

        (
            escrow.amount_to_receive(),
            escrow.bump,
        )
    };


    let escrow_pda = derive_address(
        &[
            b"escrow",
            maker.address().as_ref(),
            &[bump],
        ],
        None,
        &crate::ID.to_bytes(),
    );

    if escrow_pda != escrow_account.address().as_ref() {
        return Err(ProgramError::InvalidSeeds);
    }


    let vault_amount = {
        let vault_state =
            pinocchio_token::state::Account::from_account_view(vault)?;

        if vault_state.owner() != escrow_account.address() {
            return Err(ProgramError::IllegalOwner);
        }

        if vault_state.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }

        vault_state.amount()
    };


    {
        let taker_b =
            pinocchio_token::state::Account::from_account_view(taker_ata_b)?;

        if taker_b.owner() != taker.address() {
            return Err(ProgramError::IllegalOwner);
        }

        if taker_b.mint() != mint_b.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }


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
        from: taker_ata_b,
        to: maker_ata_b,
        authority: taker,
        multisig_signers: &[] as &[&AccountView],
        amount: amount_to_receive,
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


    maker.set_lamports(
        maker.lamports() + escrow_account.lamports()
    );

    escrow_account.set_lamports(0);

    escrow_account.close()?;

    Ok(())
}
