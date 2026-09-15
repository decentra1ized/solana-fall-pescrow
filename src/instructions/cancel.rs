use pinocchio::{
    AccountView, ProgramResult, cpi::{Seed, Signer}, error::ProgramError,
};
use pinocchio_pubkey::derive_address;

use crate::state::Escrow;

/// Cancel account order (see the guide, challenge 2):
///   0  maker                    [writable, signer] must equal escrow.maker()
///   1  mint_a                   must equal escrow.mint_a()
///   2  escrow_account           [writable] PDA, owned by this program
///   3  vault                    [writable] ATA(escrow PDA, mint A)
///   4  maker_ata_a              [writable] ATA(maker, mint A) - destination
///   5  token_program
pub fn process_cancel_instruction(
    accounts: &mut [AccountView],
) -> ProgramResult {

    let [
        maker,
        mint_a,
        escrow_account,
        vault,
        maker_ata_a,
        _token_program @ ..
    ] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    // The signer check is the critical authorization: only the maker can cancel.
    if !maker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::IllegalOwner);
    }

    // Copy the bump out of state, then drop the borrow guard.
    let bump = {
        let escrow = Escrow::load_mut(escrow_account)?;
        if escrow.maker() != *maker.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        if escrow.mint_a() != *mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        escrow.bump
    }; // RefMut guard dropped here

    // Re-derive the PDA from the stored canonical bump.
    if derive_address(
        &[b"escrow", maker.address().as_ref(), &[bump]],
        None,
        &crate::ID.to_bytes(),
    ) != escrow_account.address().to_bytes()
    {
        return Err(ProgramError::InvalidSeeds);
    }

    // Validate the vault and read its balance.
    let vault_amount = {
        let vault_state = pinocchio_token::state::Account::from_account_view(vault)?;
        if vault_state.owner() != escrow_account.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if vault_state.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        vault_state.amount()
    }; // Ref guard dropped here

    // Validate the maker's destination ATA.
    {
        let maker_ata_a_state = pinocchio_token::state::Account::from_account_view(maker_ata_a)?;
        if maker_ata_a_state.owner() != maker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if maker_ata_a_state.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    // Build the PDA signer: the vault's authority is the escrow PDA.
    let bump_bytes = [bump];
    let seed = [Seed::from(b"escrow"), Seed::from(maker.address().as_array()), Seed::from(&bump_bytes)];
    let signer = Signer::from(&seed);

    // The vault pays the maker everything it holds.
    pinocchio_token::instructions::Transfer {
        from: vault,
        to: maker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }.invoke_signed(&[signer.clone()])?;

    // Close the vault, refunding its rent to the maker.
    pinocchio_token::instructions::CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }.invoke_signed(&[signer])?;

    // Close the escrow account ourselves.
    maker.set_lamports(maker.lamports() + escrow_account.lamports());
    escrow_account.set_lamports(0);
    escrow_account.close()?;

    Ok(())
}