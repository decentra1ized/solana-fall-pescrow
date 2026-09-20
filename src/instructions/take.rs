use pinocchio::{
    cpi::{Seed, Signer},
    error::ProgramError,
    AccountView, ProgramResult,
};
use pinocchio_pubkey::derive_address;

use crate::state::Escrow;

// taker, maker, mint_a, mint_b, escrow_account, vault, taker_ata_a,
// taker_ata_b, maker_ata_b, system_program, token_program, associated_token_program
pub fn process_take_instruction(accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    if !data.is_empty() || accounts.len() != 12 {
        return Err(ProgramError::InvalidInstructionData);
    }
    let [taker, maker, mint_a, mint_b, escrow_account, vault, taker_ata_a, taker_ata_b, maker_ata_b, system_program, token_program, _ata] =
        accounts
    else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    if !taker.is_signer() || !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::MissingRequiredSignature);
    }

    let (amount_to_receive, bump) = {
        let escrow = Escrow::load_mut(escrow_account)?;
        if escrow.maker() != *maker.address()
            || escrow.mint_a() != *mint_a.address()
            || escrow.mint_b() != *mint_b.address()
        {
            return Err(ProgramError::InvalidAccountData);
        }
        (escrow.amount_to_receive(), escrow.bump)
    };
    let bump_bytes = [bump];
    let seeds = [b"escrow".as_ref(), maker.address().as_ref()];
    if derive_address(&seeds, Some(bump), &crate::ID.to_bytes())
        != *escrow_account.address().as_array()
    {
        return Err(ProgramError::InvalidSeeds);
    }

    let vault_amount = {
        let state = pinocchio_token::state::Account::from_account_view(vault)?;
        if *state.owner() != *escrow_account.address() || *state.mint() != *mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        state.amount()
    };
    pinocchio_associated_token_account::instructions::CreateIdempotent {
        funding_account: taker,
        account: taker_ata_a,
        wallet: taker,
        mint: mint_a,
        system_program,
        token_program,
    }
    .invoke()?;
    pinocchio_associated_token_account::instructions::CreateIdempotent {
        funding_account: taker,
        account: maker_ata_b,
        wallet: maker,
        mint: mint_b,
        system_program,
        token_program,
    }
    .invoke()?;
    {
        let state = pinocchio_token::state::Account::from_account_view(taker_ata_b)?;
        if *state.owner() != *taker.address() || *state.mint() != *mint_b.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }
    pinocchio_token::instructions::Transfer {
        from: taker_ata_b,
        to: maker_ata_b,
        authority: taker,
        multisig_signers: &[] as &[&AccountView],
        amount: amount_to_receive,
    }
    .invoke()?;

    let seed = [
        Seed::from(b"escrow"),
        Seed::from(maker.address().as_array()),
        Seed::from(&bump_bytes),
    ];
    let signer = Signer::from(&seed);
    pinocchio_token::instructions::Transfer {
        from: vault,
        to: taker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }
    .invoke_signed(&[signer.clone()])?;
    pinocchio_token::instructions::CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }
    .invoke_signed(&[signer])?;
    maker.set_lamports(maker.lamports() + escrow_account.lamports());
    escrow_account.set_lamports(0);
    escrow_account.close()
}
