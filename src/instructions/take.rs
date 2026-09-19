use pinocchio::{
    cpi::{Seed, Signer},
    error::ProgramError,
    AccountView, ProgramResult,
};
use pinocchio_pubkey::derive_address;

use crate::state::Escrow;

pub fn process_take_instruction(accounts: &mut [AccountView]) -> ProgramResult {
    // Destructure
    let [taker, maker, mint_a, mint_b, escrow_account, vault, taker_ata_a, taker_ata_b, maker_ata_b, system_program, token_program, _associated_token_program @ ..] =
        accounts
    else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    // check the signer
    if !taker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    // check escrow owner
    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::IllegalOwner);
    }

    // Cross-check the passed accounts against the state
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
    }; // ← RefMut guard dropped here

    // Re-derive the PDA
    // let derived_addr = pinocchio::Address::from(pinocchio_pubkey::derive_address(
    let derived_addr = derive_address(
        &[b"escrow", maker.address().as_ref(), &[bump]],
        None,
        &crate::ID.to_bytes(),
    );

    // validate escrow belongs to this maker with this bump
    // if derived_addr != *escrow_account.address() {
    if derived_addr != *escrow_account.address().as_array() {
        return Err(ProgramError::IllegalOwner);
    }

    // validate the valut and read into local state
    // how much A the taker will receive
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

    // Make sure the destination ATAs (taker_ata_a & maker_ata_b ) exist
    pinocchio_associated_token_account::instructions::CreateIdempotent {
        funding_account: taker,
        account: taker_ata_a,
        wallet: taker,
        mint: mint_a,
        token_program,
        system_program,
    }
    .invoke()?;

    pinocchio_associated_token_account::instructions::CreateIdempotent {
        funding_account: taker,
        account: maker_ata_b,
        wallet: maker,
        mint: mint_b,
        token_program,
        system_program,
    }
    .invoke()?;

    // validate taker_ata_b
    {
        let taker_ata_b_state = pinocchio_token::state::Account::from_account_view(taker_ata_b)?;
        if taker_ata_b_state.owner() != taker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if taker_ata_b_state.mint() != mint_b.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    // CPI #1, taker pays maker
    pinocchio_token::instructions::Transfer {
        from: taker_ata_b,
        to: maker_ata_b,
        amount: amount_to_receive,
        authority: taker,
        multisig_signers: &[] as &[&AccountView],
    }
    .invoke()?; // plain invoke: the taker signed the transaction

    // Build the PDA signer
    let bump_bytes = [bump];
    let seed = [
        Seed::from(b"escrow"),
        Seed::from(maker.address().as_array()),
        Seed::from(&bump_bytes),
    ];
    let signer = Signer::from(&seed);

    // CPI #2, vault pays taker
    pinocchio_token::instructions::Transfer {
        from: vault,
        to: taker_ata_a,
        amount,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }
    .invoke_signed(std::slice::from_ref(&signer))?;

    // CPI #3, close the vault
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
