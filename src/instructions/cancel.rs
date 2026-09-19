use pinocchio::{
    AccountView, ProgramResult,
    cpi::{Seed, Signer},
    error::ProgramError,
};

use crate::state::Escrow;

/// Cancel accounts, in exact order:
/// 0. maker
/// 1. mint_a
/// 2. escrow_account
/// 3. vault
/// 4. maker_ata_a
pub fn process_cancel_instruction(
    accounts: &mut [AccountView],
    _data: &[u8],
) -> ProgramResult {
    let [
        maker,
        mint_a,
        escrow_account,
        vault,
        maker_ata_a,
        _token_program,
    ] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    if !maker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::IllegalOwner);
    }

    let (stored_maker, bump) = {
        let escrow_state = Escrow::load_mut(escrow_account)?;

        if escrow_state.maker() != *maker.address() {
            return Err(ProgramError::InvalidAccountData);
        }

        if escrow_state.mint_a() != *mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }

        (
            escrow_state.maker(),
            escrow_state.bump,
        )
    };

    let bump_bytes = [bump];

    let derived_escrow = pinocchio_pubkey::derive_address(
        &[
            b"escrow",
            stored_maker.as_ref(),
            &bump_bytes,
        ],
        None,
        &crate::ID.to_bytes(),
    );

    if derived_escrow != escrow_account.address().to_bytes() {
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
        let maker_ata_state =
            pinocchio_token::state::Account::from_account_view(maker_ata_a)?;

        if maker_ata_state.owner() != maker.address() {
            return Err(ProgramError::IllegalOwner);
        }

        if maker_ata_state.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    let seed = [
        Seed::from(b"escrow"),
        Seed::from(maker.address().as_array()),
        Seed::from(&bump_bytes),
    ];

    let signer = Signer::from(&seed);

    pinocchio_token::instructions::Transfer {
        from: vault,
        to: maker_ata_a,
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
    .invoke_signed(&[signer])?;

    let escrow_lamports = escrow_account.lamports();

    maker.set_lamports(maker.lamports() + escrow_lamports);
    escrow_account.set_lamports(0);

    escrow_account.close()?;

    Ok(())
}