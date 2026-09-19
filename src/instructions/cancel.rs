use pinocchio::{
    cpi::{Seed, Signer},
    error::ProgramError,
    AccountView, ProgramResult,
};
use pinocchio_pubkey::derive_address;

use crate::state::Escrow;

pub fn process_cancel_instruction(accounts: &mut [AccountView]) -> ProgramResult {
    // Destructure
    let [maker, mint_a, escrow_account, vault, maker_ata_a, _token_program] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    // check the signer
    if !maker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    // Load escrow state, verify program ownership and Copy out bump
    let bump = {
        let escrow_state = Escrow::load_mut(escrow_account)?;
        if escrow_state.maker() != *maker.address() {
            return Err(ProgramError::IllegalOwner);
        }

        if escrow_state.mint_a() != *mint_a.address() {
            return Err(ProgramError::IllegalOwner);
        }

        escrow_state.bump
    };

    // Re-derive the PDA
    let derived_addr = derive_address(
        &[b"escrow", maker.address().as_ref(), &[bump]],
        None,
        &crate::ID.to_bytes(),
    );

    // validate escrow belongs to this maker with this bump
    if derived_addr != *escrow_account.address().as_array() {
        return Err(ProgramError::IllegalOwner);
    }

    // validate the valut and read into local state
    // how much A maker transferred to vault
    let amount = {
        let vault_state = pinocchio_token::state::Account::from_account_view(vault)?;

        if vault_state.owner() != escrow_account.address() {
            return Err(ProgramError::IllegalOwner);
        }

        if vault_state.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }

        vault_state.amount()
    };

    // validate maker_ata_a
    {
        let maker_ata_a_state = pinocchio_token::state::Account::from_account_view(maker_ata_a)?;
        if maker_ata_a_state.owner() != maker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if maker_ata_a_state.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    // Build the PDA signer
    let bump_bytes = [bump];
    let seed = [
        Seed::from(b"escrow"),
        Seed::from(maker.address().as_array()),
        Seed::from(&bump_bytes),
    ];
    let signer = Signer::from(&seed);

    // transfer from valut to maker_ata_a
    pinocchio_token::instructions::Transfer {
        from: vault,
        to: maker_ata_a,
        amount,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }
    .invoke_signed(std::slice::from_ref(&signer))?;

    // close the vault
    pinocchio_token::instructions::CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }
    .invoke_signed(std::slice::from_ref(&signer))?;

    // Close the escrow account, by hand
    // (The escrow is owned by this program, so there is no CPI for it)
    maker.set_lamports(maker.lamports() + escrow_account.lamports());
    escrow_account.set_lamports(0);
    escrow_account.close()?;

    Ok(())
}
