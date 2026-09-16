// Accounts:
// 0  maker          (W, signer)
// 1  mint_a
// 2  escrow_account (W, PDA)
// 3  vault          (W)
// 4  maker_ata_a    (W)
// 5  token_program

use pinocchio::{
    AccountView, ProgramResult,
    cpi::{Seed, Signer},
    error::ProgramError,
};
use pinocchio_token::state::Account as TokenAccount;

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
        maker_ata_a,
        token_program,
    ] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    if !maker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    // Load escrow state and validate
    let bump = {
        if !escrow_account.owned_by(&crate::ID) {
            return Err(ProgramError::InvalidAccountData);
        }
        let escrow = Escrow::load_mut(escrow_account)?;
        if escrow.maker() != *maker.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        if escrow.mint_a() != *mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        escrow.bump
    };

    // Re-derive PDA
    let escrow_pda = pinocchio::address::Address::derive_address(
        &[b"escrow", maker.address().as_ref(), &[bump]],
        None,
        &crate::ID,
    );
    if escrow_pda != *escrow_account.address() {
        return Err(ProgramError::InvalidSeeds);
    }

    // Validate vault and read balance
    let vault_amount = {
        let vault_state = TokenAccount::from_account_view(vault)?;
        if vault_state.owner() != escrow_account.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if vault_state.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        vault_state.amount()
    };

    // Validate maker_ata_a
    {
        let maker_ata_state = TokenAccount::from_account_view(maker_ata_a)?;
        if maker_ata_state.owner() != maker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if maker_ata_state.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    // Build PDA signer
    let bump_bytes = [bump];
    let seed = [
        Seed::from(b"escrow"),
        Seed::from(maker.address().as_array()),
        Seed::from(&bump_bytes),
    ];
    let seeds = Signer::from(&seed);

    // CPI 1: transfer from vault to maker_ata_a
    pinocchio_token::instructions::Transfer {
        from: vault,
        to: maker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }
    .invoke_signed(&[seeds.clone()])?;

    // CPI 2: close vault
    pinocchio_token::instructions::CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }
    .invoke_signed(&[seeds.clone()])?;

    // Close escrow account
    let escrow_lamports = escrow_account.lamports();
    maker.set_lamports(maker.lamports() + escrow_lamports);
    escrow_account.set_lamports(0);
    escrow_account.close()?;

    Ok(())
}
