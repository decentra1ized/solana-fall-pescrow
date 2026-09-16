//! Cancelling instruction for the escrow program.
use pinocchio::{
    AccountView, Address, ProgramResult, cpi::{Seed, Signer},
    error::ProgramError, sysvars::{Sysvar, rent::Rent}
};
use pinocchio_system::instructions::CreateAccount;
use pinocchio_log::log;

use crate::state::Escrow;

pub fn process_cancel_instruction(
    accounts: &mut [AccountView]
) -> ProgramResult {
    let [
        maker,
        mint_a,
        mint_b,
        escrow_account,
        maker_ata,
        escrow_ata,
        system_program,
        token_program,
        _associated_token_program@ ..
    ] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    // Scoped access to free them up for later access.
    {
        let maker_ata_state = pinocchio_token::state::Account::from_account_view(maker_ata)?;
        if maker_ata_state.owner() != maker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if maker_ata_state.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }
    if !maker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    let (escrow_account_pda, bump) = Address::find_program_address(&[b"escrow", maker.address().as_ref()], &crate::ID);
    if escrow_account_pda != *escrow_account.address() {
        return Err(ProgramError::InvalidSeeds);
    }

    let escrow_state = Escrow::load_mut(escrow_account)?;
    if escrow_state.mint_a() != *mint_a.address() {
        return Err(ProgramError::InvalidAccountData);
    }
    if escrow_state.mint_b() != *mint_b.address() {
        return Err(ProgramError::InvalidAccountData);
    }
    let escrow_maker_give_amount = escrow_state.amount_to_give();
    drop(escrow_state);

    // Transfer token A from the vault back to maker.
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
        to: maker_ata,
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

    log!("This is the end of Cancel instruction");
    Ok(())
}
