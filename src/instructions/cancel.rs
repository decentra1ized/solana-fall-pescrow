use pinocchio::{
    AccountView, ProgramResult, cpi::{Seed, Signer}, error::ProgramError,
};
use pinocchio_pubkey::derive_address;

use crate::state::Escrow;

// Account order (the API — keep this comment and the test in sync):
//   0  maker            (w, s) must sign; must equal escrow.maker()
//   1  mint_a                  must equal escrow.mint_a()
//   2  escrow_account   (w)    PDA, will be closed
//   3  vault            (w)    will be closed
//   4  maker_ata_a      (w)    destination for the returned A
//   5  token_program
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
        _token_program @ ..
    ] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    // 1 · the critical authorization check — forget this and anyone can cancel anyone's escrow
    if !maker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    // 2 · only trust escrow state after confirming this program owns the account
    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::InvalidAccountData);
    }

    // 3 · the second critical check — proves this is *this* maker's escrow, not
    //     just that *some* signer showed up. Copy bump out, then drop the guard
    //     before any CPI touches escrow_account.
    let bump = {
        let escrow = Escrow::load_mut(escrow_account)?;
        if escrow.maker()  != *maker.address()  { return Err(ProgramError::InvalidAccountData); }
        if escrow.mint_a() != *mint_a.address() { return Err(ProgramError::InvalidAccountData); }
        escrow.bump
    };

    // 4 · re-derive the PDA from the stored bump (single hash, no search)
    let derived = derive_address(
        &[b"escrow", maker.address().as_ref(), &[bump]],
        None,
        crate::ID.as_array(),
    );
    if derived != *escrow_account.address().as_array() {
        return Err(ProgramError::InvalidSeeds);
    }

    // 5 · validate the vault and read its live balance
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

    // 6 · build the PDA signer
    let bump_bytes = [bump];
    let seed = [Seed::from(b"escrow"), Seed::from(maker.address().as_array()), Seed::from(&bump_bytes)];
    let signer = Signer::from(&seed);

    // 7 · vault pays the maker back
    pinocchio_token::instructions::Transfer {
        from: vault,
        to: maker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }.invoke_signed(&[signer.clone()])?;

    // 8 · close the vault, rent to maker
    pinocchio_token::instructions::CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }.invoke_signed(&[signer.clone()])?;

    // 9 · close the escrow account by hand
    maker.set_lamports(maker.lamports() + escrow_account.lamports());
    escrow_account.set_lamports(0);
    escrow_account.close()?;

    Ok(())
}
