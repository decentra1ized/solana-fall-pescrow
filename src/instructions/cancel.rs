use pinocchio::{
    AccountView, ProgramResult,
    cpi::{Seed, Signer},
    error::ProgramError,
};
use pinocchio_pubkey::derive_address;
use pinocchio_token::{
    instructions::{CloseAccount, Transfer},
    state::Account,
};

use crate::state::Escrow;

/// Accounts for Cancel:
/// 0. `[signer, writable]` maker: Must sign. Must equal escrow.maker()
/// 1. `[]` mint_a: Must equal escrow.mint_a()
/// 2. `[writable]` escrow_account: PDA, will be closed
/// 3. `[writable]` vault: ATA(escrow PDA, mint A), will be closed
/// 4. `[writable]` maker_ata_a: Destination for the returned A
/// 5. `[]` token_program
pub fn process_cancel_instruction(accounts: &mut [AccountView]) -> ProgramResult {
    let [
        maker,
        mint_a,
        escrow_account,
        vault,
        maker_ata_a,
        _token_program @ ..,
    ] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    // 1. Critical authorization check
    if !maker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    // 2. Program ownership check
    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::IllegalOwner);
    }

    // 3. Cross-check against state & extract bump (scoped borrow)
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

    // 4. Re-derive and check PDA
    let derived_pda = derive_address(
        &[b"escrow", maker.address().as_ref(), &[bump]],
        None,
        &crate::ID.to_bytes(),
    );

    if &derived_pda != escrow_account.address().as_ref() {
        return Err(ProgramError::InvalidSeeds);
    }

    // 5. Validate vault & maker_ata_a (scoped borrows)
    let vault_amount = {
        let vault_state = Account::from_account_view(vault)?;
        if vault_state.owner() != escrow_account.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if vault_state.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        vault_state.amount()
    };

    {
        let maker_ata_a_state = Account::from_account_view(maker_ata_a)?;
        if maker_ata_a_state.owner() != maker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if maker_ata_a_state.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    // 6. Build PDA signer
    let bump_bytes = [bump];
    let seed = [
        Seed::from(b"escrow"),
        Seed::from(maker.address().as_array()),
        Seed::from(&bump_bytes),
    ];
    let signer = Signer::from(&seed);

    // 7. CPI #1: Refund Mint A from vault to maker_ata_a
    Transfer {
        from: vault,
        to: maker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }
    .invoke_signed(&[signer.clone()])?;

    // 8. CPI #2: Close vault ATA, refund rent to maker
    CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }
    .invoke_signed(&[signer.clone()])?;

    // 9. Close escrow account by hand, transferring lamports to maker
    maker.set_lamports(maker.lamports() + escrow_account.lamports());
    escrow_account.set_lamports(0);
    escrow_account.close()?;

    Ok(())
}
