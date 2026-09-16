use pinocchio::{
    AccountView,
    ProgramResult,
    Address,
    cpi::{Seed, Signer},
    error::ProgramError,
};

use crate::state::Escrow;

pub fn process_cancel_instruction(
    accounts: &mut [AccountView],
    _data: &[u8],
) -> ProgramResult {
    let [
        maker,
        mint_a,
        escrow_account,
        vault,
        maker_ata,
        _token_program,
    ] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    // Check whether maker signed
    if !maker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    // Check whether escrow account is owned by our program
    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::IllegalOwner);
    }

    // Reading escrow state
    let bump;

    {
        let escrow_state = Escrow::load_mut(escrow_account)?;

        if escrow_state.maker() != *maker.address() {
            return Err(ProgramError::InvalidAccountData);
        }

        if escrow_state.mint_a() != *mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }

        bump = escrow_state.bump;
    }

    // PDA verification
    let bump_bytes = [bump];

    let escrow_account_pda = pinocchio_pubkey::derive_address(
        &[b"escrow", maker.address().as_ref(), &bump_bytes],
        None,
        &crate::ID.to_bytes(),
    );

    if Address::from(escrow_account_pda) != *escrow_account.address() {
        return Err(ProgramError::InvalidSeeds);
    }

    // Checking the vault
    let vault_amount;

    {
        let vault_state =
            pinocchio_token::state::Account::from_account_view(vault)?;

        if vault_state.owner() != escrow_account.address() {
            return Err(ProgramError::IllegalOwner);
        }

        if vault_state.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }

        vault_amount = vault_state.amount();
    }

    // Check maker's Token A account
    {
        let maker_ata_state =
            pinocchio_token::state::Account::from_account_view(maker_ata)?;

        if maker_ata_state.owner() != maker.address() {
            return Err(ProgramError::IllegalOwner);
        }

        if maker_ata_state.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    // Reconstruct PDA signer
    let seed = [
        Seed::from(b"escrow"),
        Seed::from(maker.address().as_array()),
        Seed::from(&bump_bytes),
    ];

    let seeds = Signer::from(&seed);

    // Token A: vault -> maker
    pinocchio_token::instructions::Transfer {
        from: vault,
        to: maker_ata,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }
    .invoke_signed(&[seeds.clone()])?;

    // Close vault
    pinocchio_token::instructions::CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }
    .invoke_signed(&[seeds])?;

    // Close escrow account
    maker.set_lamports(maker.lamports() + escrow_account.lamports());
    escrow_account.set_lamports(0);
    escrow_account.close()?;

    Ok(())
}