use pinocchio::{ account::AccountView, cpi::{Seed, Signer}, error::ProgramError,};

use pinocchio_pubkey::derive_address;

use pinocchio_token::{ instructions::{CloseAccount, Transfer}, state::Account as TokenAccount,};

use crate::state::Escrow;

pub fn process_cancel_instruction(
    accounts: &mut [AccountView],
) -> Result<(), ProgramError> {
    let [
        maker,
        mint_a,
        escrow_account,
        vault,
        maker_ata_a,
        _token_program,
    ] = accounts
    else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    // 1. Maker must sign.
    if !maker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    // 2. Verify escrow ownership before trusting its state.
    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::InvalidAccountOwner);
    }

    let bump = {
        let escrow = Escrow::load_mut(escrow_account)?;

        if escrow.maker() != *maker.address() {
            return Err(ProgramError::InvalidAccountData);
        }

        if escrow.mint_a() != *mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }

        escrow.bump
    };

    // 3. Re-derive escrow PDA using the stored bump.
    let escrow_address = derive_address(
        &[
            b"escrow",
            maker.address().as_ref(),
            &[bump],
        ],
        None,
        &crate::ID.to_bytes(),
    );

    if escrow_address != *escrow_account.address().as_array() {
        return Err(ProgramError::InvalidAccountData);
    }

    // 4. Validate vault and capture its balance.
    let vault_amount = {
        let vault_state = TokenAccount::from_account_view(vault)?;

        if vault_state.owner() != escrow_account.address() {
            return Err(ProgramError::InvalidAccountData);
        }

        if vault_state.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }

        vault_state.amount()
    };

    // Validate maker's destination ATA.
    {
        let maker_ata_state = TokenAccount::from_account_view(maker_ata_a)?;

        if maker_ata_state.owner() != maker.address() {
            return Err(ProgramError::InvalidAccountData);
        }

        if maker_ata_state.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    // 5. Build escrow PDA signer.
    let bump_bytes = [bump];

    let seed = [
        Seed::from(b"escrow"),
        Seed::from(maker.address().as_array()),
        Seed::from(&bump_bytes),
    ];

    let signer = Signer::from(&seed);

    // 6. Return all A from vault to maker.
    Transfer {
        from: vault,
        to: maker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }
    .invoke_signed(&[signer.clone()])?;

    // 7. Close vault.
    CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }
    .invoke_signed(&[signer.clone()])?;

    // 8. Close escrow .
    maker.set_lamports(
        maker
            .lamports()
            .checked_add(escrow_account.lamports())
            .ok_or(ProgramError::ArithmeticOverflow)?,
    );

    escrow_account.set_lamports(0);
    escrow_account.close()?;

    Ok(())
}