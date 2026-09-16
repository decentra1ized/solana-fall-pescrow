use pinocchio::{
    AccountView, Address, ProgramResult, cpi::{Seed, Signer}, error::ProgramError,
};

use crate::state::Escrow;

// Accounts: maker(s,w), mint_a, escrow(w), vault(w), maker_ata_a(w), system_program, token_program
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
        _system_program,
        _token_program@ ..
    ] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    if !maker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    // Only trust escrow data once we know this program owns the account.
    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::InvalidAccountData);
    }

    let (stored_maker, stored_mint_a, bump) = {
        let escrow_state = Escrow::load(escrow_account)?;
        (escrow_state.maker(), escrow_state.mint_a(), escrow_state.bump)
    };

    if stored_maker != *maker.address() {
        return Err(ProgramError::InvalidAccountData);
    }
    if stored_mint_a != *mint_a.address() {
        return Err(ProgramError::InvalidAccountData);
    }

    // Escrow already exists, so the canonical bump was validated once at `Make`. Re-derive
    // with the stored bump via a single hash (no `find_program_address` search) and compare.
    let escrow_account_pda =
        Address::derive_address(&[b"escrow", maker.address().as_ref()], Some(bump), &crate::ID);
    if escrow_account_pda != *escrow_account.address() {
        return Err(ProgramError::InvalidSeeds);
    }

    // Validate the vault and read its real balance; that, not `amount_to_give` from state,
    // is what actually moves.
    let vault_amount = {
        let vault_state = pinocchio_token::state::Account::from_account_view(vault)?;
        if vault_state.owner() != escrow_account.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if vault_state.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        vault_state.amount()
    };

    {
        let maker_ata_a_state = pinocchio_token::state::Account::from_account_view(maker_ata_a)?;
        if maker_ata_a_state.owner() != maker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if maker_ata_a_state.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    let bump_bytes = [bump];
    let seed = [Seed::from(b"escrow"), Seed::from(maker.address().as_array()), Seed::from(&bump_bytes)];
    let seeds = [Signer::from(&seed)];

    // Return the deposited tokens to the maker, then close the now-empty vault and the
    // escrow account, refunding both rents to the maker.
    pinocchio_token::instructions::Transfer {
        from: vault,
        to: maker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }.invoke_signed(&seeds)?;

    pinocchio_token::instructions::CloseAccount::new(vault, maker, escrow_account)
        .invoke_signed(&seeds)?;

    let escrow_lamports = escrow_account.lamports();
    let maker_lamports = maker.lamports().checked_add(escrow_lamports)
        .ok_or(ProgramError::ArithmeticOverflow)?;
    maker.set_lamports(maker_lamports);
    escrow_account.close()?;

    Ok(())
}