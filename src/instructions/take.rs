use pinocchio::{
    AccountView, Address, ProgramResult, cpi::{Seed, Signer}, error::ProgramError,
};

use crate::state::Escrow;

// Accounts: taker(s,w), maker(w), mint_a, mint_b, escrow(w), vault(w),
// taker_ata_a(w), taker_ata_b(w), maker_ata_b(w), system_program, token_program, associated_token_program
pub fn process_take_instruction(
    accounts: &mut [AccountView],
    _data: &[u8],
) -> ProgramResult {

    let [
        taker,
        maker,
        mint_a,
        mint_b,
        escrow_account,
        vault,
        taker_ata_a,
        taker_ata_b,
        maker_ata_b,
        system_program,
        token_program,
        _associated_token_program@ ..
    ] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    if !taker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    // Only trust escrow data once we know this program owns the account.
    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::InvalidAccountData);
    }

    // Read what we need out of the escrow account, then drop the borrow before any CPI.
    let (stored_maker, stored_mint_a, stored_mint_b, amount_to_receive, bump) = {
        let escrow_state = Escrow::load(escrow_account)?;
        (
            escrow_state.maker(),
            escrow_state.mint_a(),
            escrow_state.mint_b(),
            escrow_state.amount_to_receive(),
            escrow_state.bump,
        )
    };

    if stored_maker != *maker.address() {
        return Err(ProgramError::InvalidAccountData);
    }
    if stored_mint_a != *mint_a.address() || stored_mint_b != *mint_b.address() {
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

    // Checked before the CPIs below so an invalid `taker_ata_b` fails cheaply.
    {
        let taker_ata_b_state = pinocchio_token::state::Account::from_account_view(taker_ata_b)?;
        if taker_ata_b_state.owner() != taker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if taker_ata_b_state.mint() != mint_b.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    let bump_bytes = [bump];
    let seed = [Seed::from(b"escrow"), Seed::from(maker.address().as_array()), Seed::from(&bump_bytes)];
    let seeds = [Signer::from(&seed)];

    pinocchio_associated_token_account::instructions::CreateIdempotent {
        funding_account: taker,
        account: taker_ata_a,
        wallet: taker,
        mint: mint_a,
        token_program,
        system_program,
    }.invoke()?;

    pinocchio_associated_token_account::instructions::CreateIdempotent {
        funding_account: taker,
        account: maker_ata_b,
        wallet: maker,
        mint: mint_b,
        token_program,
        system_program,
    }.invoke()?;

    // Pay the maker what was asked for, then release the vault's tokens to the taker.
    pinocchio_token::instructions::Transfer {
        from: taker_ata_b,
        to: maker_ata_b,
        authority: taker,
        multisig_signers: &[] as &[&AccountView],
        amount: amount_to_receive,
    }.invoke()?;

    pinocchio_token::instructions::Transfer {
        from: vault,
        to: taker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }.invoke_signed(&seeds)?;

    // Vault is empty now; close it and the escrow account, refunding rent to the maker.
    pinocchio_token::instructions::CloseAccount::new(vault, maker, escrow_account)
        .invoke_signed(&seeds)?;

    let escrow_lamports = escrow_account.lamports();
    let maker_lamports = maker.lamports().checked_add(escrow_lamports)
        .ok_or(ProgramError::ArithmeticOverflow)?;
    maker.set_lamports(maker_lamports);
    escrow_account.close()?;

    Ok(())
}