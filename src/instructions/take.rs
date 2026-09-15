use pinocchio::{
    AccountView, ProgramResult, cpi::{Seed, Signer}, error::ProgramError,
};
use pinocchio_pubkey::derive_address;

use crate::state::Escrow;

/// Take account order (see the guide, challenge 1):
///   0  taker                    [writable, signer]
///   1  maker                    [writable]  must equal escrow.maker()
///   2  mint_a                   must equal escrow.mint_a()
///   3  mint_b                   must equal escrow.mint_b()
///   4  escrow_account           [writable] PDA, owned by this program
///   5  vault                    [writable] ATA(escrow PDA, mint A)
///   6  taker_ata_a              [writable] ATA(taker, mint A) - may need creating
///   7  taker_ata_b              [writable] ATA(taker, mint B) - source of B
///   8  maker_ata_b              [writable] ATA(maker, mint B) - may need creating
///   9  system_program
///   10 token_program
///   11 associated_token_program
pub fn process_take_instruction(
    accounts: &mut [AccountView],
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
        _associated_token_program @ ..
    ] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    if !taker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::IllegalOwner);
    }

    // Copy the immutable fields out of state, then drop the borrow guard:
    // we CPI with `escrow_account` (and later close it) and a live borrow
    // would fail.
    let (amount_to_receive, bump) = {
        let escrow = Escrow::load_mut(escrow_account)?;
        if escrow.maker() != *maker.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        if escrow.mint_a() != *mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        if escrow.mint_b() != *mint_b.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        (escrow.amount_to_receive(), escrow.bump)
    }; // RefMut guard dropped here

    // Re-derive the PDA from the stored canonical bump (single hash, no loop).
    if derive_address(
        &[b"escrow", maker.address().as_ref(), &[bump]],
        None,
        &crate::ID.to_bytes(),
    ) != escrow_account.address().to_bytes()
    {
        return Err(ProgramError::InvalidSeeds);
    }

    // Validate the vault and read how much A the taker will receive.
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

    // Destination ATAs may not exist yet; create them idempotently.
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

    // The taker's B account must already hold mint B and belong to the taker.
    {
        let taker_ata_b_state = pinocchio_token::state::Account::from_account_view(taker_ata_b)?;
        if taker_ata_b_state.owner() != taker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if taker_ata_b_state.mint() != mint_b.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    // CPI #1: the taker pays the maker `amount_to_receive` of mint B.
    pinocchio_token::instructions::Transfer {
        from: taker_ata_b,
        to: maker_ata_b,
        authority: taker,
        multisig_signers: &[] as &[&AccountView],
        amount: amount_to_receive,
    }.invoke()?;

    // Build the PDA signer: the vault's authority is the escrow PDA.
    let bump_bytes = [bump];
    let seed = [Seed::from(b"escrow"), Seed::from(maker.address().as_array()), Seed::from(&bump_bytes)];
    let signer = Signer::from(&seed);

    // CPI #2: the vault pays the taker everything it holds.
    pinocchio_token::instructions::Transfer {
        from: vault,
        to: taker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }.invoke_signed(&[signer.clone()])?;

    // CPI #3: close the vault, refunding its rent to the maker.
    pinocchio_token::instructions::CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }.invoke_signed(&[signer])?;

    // Close the escrow account ourselves: move lamports to the maker first,
    // then zero and close it.
    maker.set_lamports(maker.lamports() + escrow_account.lamports());
    escrow_account.set_lamports(0);
    escrow_account.close()?;

    Ok(())
}