//! Take instruction for the escrow program.
use pinocchio::{
    AccountView, Address, ProgramResult, cpi::{Seed, Signer},
    error::ProgramError, sysvars::{Sysvar, rent::Rent}
};
use pinocchio_system::instructions::CreateAccount;
use pinocchio_log::log;

use crate::state::Escrow;

/// 8 (amount_to_receive) + 8 (amount to give)
const TAKE_DATA_LEN: usize = 16;

/// Transfer funds from the vault to the taker, after taker transfers to maker
pub fn process_take_instruction(
    accounts: &mut [AccountView]
) -> ProgramResult {
    let [
        taker,
        maker,
        mint_a,
        mint_b,
        escrow_account,
        maker_ata_a,
        maker_ata_b,
        taker_ata_a,
        taker_ata_b,
        escrow_ata,
        system_program,
        token_program,
        _associated_token_program@ ..
    ] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    // Checking the token destinations
    {
        // Check maker_ata_b. It is the reciever of token b from taker
        let maker_ata_b_state = pinocchio_token::state::Account::from_account_view(maker_ata_b)?;
        if maker_ata_b_state.owner() != maker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if maker_ata_b_state.mint() != mint_b.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        // Check taker_ata_a. It is the reciever of token a from maker escrow.
        let taker_ata_a_state = pinocchio_token::state::Account::from_account_view(taker_ata_a)?;
        if taker_ata_a_state.owner() != taker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if taker_ata_a_state.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }

        // Check the escrow ata.
        let escrow_ata_state = pinocchio_token::state::Account::from_account_view(escrow_ata)?;
        if escrow_ata_state.owner() != escrow_account.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if escrow_ata_state.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    if !taker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    let (escrow_account_pda, bump) = Address::find_program_address(
        &[b"escrow", maker.address().as_ref()], &crate::ID
    );
    if escrow_account_pda != *escrow_account.address() {
        return Err(ProgramError::InvalidSeeds);
    }

    // Load the escrow details.
    let escrow_state = Escrow::load_mut(escrow_account)?;
    if escrow_state.mint_a() != *mint_a.address() {
        return Err(ProgramError::InvalidAccountData);
    }
    if escrow_state.mint_b() != *mint_b.address() {
        return Err(ProgramError::InvalidAccountData);
    }
    let escrow_maker_give_amount = escrow_state.amount_to_give();
    let escrow_maker_receive = escrow_state.amount_to_receive();
    drop(escrow_state);
    // Trasferring token B from taker to maker.
    pinocchio_token::instructions::Transfer {
        from: taker_ata_b,
        to: maker_ata_b,
        authority: taker,
        multisig_signers: &[] as &[&AccountView],
        amount: escrow_maker_receive,
    }.invoke()?;

    // Transfer token A from the vault to taker.
    let bump_binding = [bump];
    let vault_signer_seeds = [
        Seed::from(b"escrow"),
        Seed::from(maker.address().as_ref()),
        Seed::from(&bump_binding),
    ];
    let signer_seeds = [Signer::from(&vault_signer_seeds)];
    // Transfer tokan a from escrow to taker
    pinocchio_token::instructions::Transfer {
        from: escrow_ata,
        to: taker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: escrow_maker_give_amount,
    }.invoke_signed(&signer_seeds)?;

    // Closing the escrow ata holding the escrow tokens
    pinocchio_token::instructions::CloseAccount {
        account: escrow_ata,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }.invoke_signed(&signer_seeds)?;

    // Closing the escrow pda and reclaiming lamports.
    let escrow_lamports = escrow_account.lamports();
    maker.set_lamports(
        maker.lamports()
        .checked_add(escrow_lamports)
        .ok_or(ProgramError::ArithmeticOverflow)?
    );
    escrow_account.close()?;
    Ok(())
}
