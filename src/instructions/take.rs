use pinocchio::{
    AccountView, ProgramResult, cpi::{Seed, Signer}, error::ProgramError
};
use pinocchio_associated_token_account::instructions::CreateIdempotent;
use pinocchio_pubkey::derive_address;
use pinocchio_token::instructions::Transfer;

use crate::instructions::shared::{check_token_account, drain_and_close};
use crate::state::Escrow;

/// Accounts (this order is the API, there is no IDL):
///   0 taker (signer, writable), 1 maker (writable), 2 mint_a, 3 mint_b,
///   4 escrow_account (writable), 5 vault (writable), 6 taker_ata_a (writable),
///   7 taker_ata_b (writable), 8 maker_ata_b (writable),
///   9 system_program, 10 token_program, 11 associated_token_program
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

    // Only trust the stored terms if this program owns the account.
    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::IllegalOwner);
    }

    // Copy out what we need and drop the guard: the CPIs below touch this account.
    let (amount_to_receive, bump) = {
        let escrow_state = Escrow::load_mut(escrow_account)?;
        if escrow_state.maker() != *maker.address()
            || escrow_state.mint_a() != *mint_a.address()
            || escrow_state.mint_b() != *mint_b.address()
        {
            return Err(ProgramError::InvalidAccountData);
        }
        (escrow_state.amount_to_receive(), escrow_state.bump)
    };

    // Make stored the canonical bump, so this is one hash instead of a bump search.
    let bump_bytes = [bump];
    let seed = [b"escrow".as_ref(), maker.address().as_ref(), &bump_bytes];
    if derive_address(&seed, None, &crate::ID.to_bytes()) != *escrow_account.address().as_array() {
        return Err(ProgramError::InvalidSeeds);
    }

    // What the taker gets is whatever the vault actually holds, not amount_to_give.
    let vault_amount = check_token_account(vault, escrow_account.address(), mint_a.address())?;

    // The taker may not have an account for A yet, and the maker may not have one for B.
    CreateIdempotent {
        funding_account: taker,
        account: taker_ata_a,
        wallet: taker,
        mint: mint_a,
        system_program: system_program,
        token_program: token_program,
    }.invoke()?;

    CreateIdempotent {
        funding_account: taker,
        account: maker_ata_b,
        wallet: maker,
        mint: mint_b,
        system_program: system_program,
        token_program: token_program,
    }.invoke()?;

    check_token_account(taker_ata_b, taker.address(), mint_b.address())?;

    // The taker signed the transaction, so paying the maker is a plain invoke.
    Transfer {
        from: taker_ata_b,
        to: maker_ata_b,
        authority: taker,
        multisig_signers: &[] as &[&AccountView],
        amount: amount_to_receive,
    }.invoke()?;

    // Copy the address out so the Signer does not keep borrowing `maker`, which the
    // rent refund below needs mutably.
    let maker_address = *maker.address().as_array();
    let seed = [Seed::from(b"escrow"), Seed::from(&maker_address), Seed::from(&bump_bytes)];
    let seeds = Signer::from(&seed);

    // Vault pays the taker, then both accounts close with the rent going back to the maker.
    drain_and_close(vault, taker_ata_a, escrow_account, maker, vault_amount, seeds)
}
