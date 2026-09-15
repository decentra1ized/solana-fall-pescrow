// Account order (6 accounts, no names/IDL - this order IS the API):
//   0  maker           (signer, writable) must sign; must equal escrow.maker()
//   1  mint_a                              must equal escrow.mint_a()
//   2  escrow_account  (writable)          PDA, will be closed
//   3  vault           (writable)          will be closed
//   4  maker_ata_a     (writable)          destination for the returned A
//   5  token_program

use pinocchio::{
    AccountView, cpi::{Seed, Signer}, error::ProgramError, ProgramResult,
};
use pinocchio_pubkey::derive_address;

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
        _token_program @ ..
    ] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    // The critical authorization check: forget this and anyone can drain any
    // escrow back to its own maker, which is annoying but not catastrophic
    // since the maker check below still holds.
    if !maker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::IllegalOwner);
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

    let bump_bytes = [bump];
    let derived = derive_address(
        &[b"escrow", maker.address().as_ref(), &bump_bytes],
        None,
        &crate::ID.to_bytes(),
    );
    if derived != *escrow_account.address().as_array() {
        return Err(ProgramError::InvalidSeeds);
    }

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
        let maker_ata_state = pinocchio_token::state::Account::from_account_view(maker_ata_a)?;
        if maker_ata_state.owner() != maker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if maker_ata_state.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    let seed = [Seed::from(b"escrow"), Seed::from(maker.address().as_array()), Seed::from(&bump_bytes)];
    let seeds = Signer::from(&seed);

    pinocchio_token::instructions::Transfer {
        from: vault,
        to: maker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }.invoke_signed(&[seeds.clone()])?;

    pinocchio_token::instructions::CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }.invoke_signed(&[seeds.clone()])?;

    {
        let escrow_lamports = escrow_account.lamports();
        let maker_lamports = maker.lamports();
        maker.set_lamports(maker_lamports + escrow_lamports);
        escrow_account.set_lamports(0);
        escrow_account.close()?;
    }

    Ok(())
}
