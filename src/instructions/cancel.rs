use pinocchio::{
    AccountView,
    ProgramResult,
    cpi::{Seed, Signer},
    error::ProgramError,
};

use crate::state::Escrow;
use pinocchio_pubkey::derive_address;

// Account order:
// 0  maker
// 1  mint_a
// 2  escrow_account
// 3  vault
// 4  maker_ata_a
// 5  token_program
pub fn process_cancel_instruction(
    accounts: &mut [AccountView],
) -> ProgramResult {
    let [
        maker,
        mint_a,
        escrow_account,
        vault,
        maker_ata_a,
        _token_program,
        ..
    ] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    // 1. Only the maker may cancel the escrow.
    if !maker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    // 2. Never trust escrow data before checking its owner.
    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::InvalidAccountOwner);
    }

    // Read the stored maker, mint and bump, then drop the borrow.
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

    // 3. Re-derive the escrow PDA using the stored bump.
    let escrow_pda = derive_address(
        &[b"escrow", maker.address().as_ref(), &[bump]],
        None,
        &crate::ID.to_bytes(),
    );

    if escrow_pda != escrow_account.address().to_bytes() {
        return Err(ProgramError::InvalidSeeds);
    }

    // 4. Validate the vault and remember its balance.
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

    // Validate the maker's destination token account.
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

    // 5. Build the escrow PDA signer.
    let bump_bytes = [bump];
    let seed = [
        Seed::from(b"escrow"),
        Seed::from(maker.address().as_array()),
        Seed::from(&bump_bytes),
    ];
    let signer = Signer::from(&seed);

    // 6. Return all token A from the vault to the maker.
    pinocchio_token::instructions::Transfer {
        from: vault,
        to: maker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }
    .invoke_signed(&[signer.clone()])?;

    // 7. Close the empty vault and refund its rent to the maker.
    pinocchio_token::instructions::CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }
    .invoke_signed(&[signer.clone()])?;

    // 8. Close the escrow account itself and refund its rent.
    maker.set_lamports(maker.lamports() + escrow_account.lamports());
    escrow_account.set_lamports(0);
    escrow_account.close()?;

    Ok(())
}