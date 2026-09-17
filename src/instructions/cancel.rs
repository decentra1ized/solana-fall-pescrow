use crate::state::Escrow;
use pinocchio::{
    cpi::{Seed, Signer},
    error::ProgramError,
    AccountView, ProgramResult,
};
use pinocchio_pubkey::derive_address;

// maker, mint_a, escrow_account, vault, maker_ata_a, token_program
pub fn process_cancel_instruction(accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    if !data.is_empty() || accounts.len() != 6 {
        return Err(ProgramError::InvalidInstructionData);
    }
    let [maker, mint_a, escrow_account, vault, maker_ata_a, _token_program] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    if !maker.is_signer() || !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::MissingRequiredSignature);
    }
    let bump = {
        let escrow = Escrow::load_mut(escrow_account)?;
        if escrow.maker() != *maker.address() || escrow.mint_a() != *mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        escrow.bump
    };
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
    {
        let state = pinocchio_token::state::Account::from_account_view(maker_ata_a)?;
        if *state.owner() != *maker.address() || *state.mint() != *mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }
    let bump_bytes = [bump];
    let seed = [
        Seed::from(b"escrow"),
        Seed::from(maker.address().as_array()),
        Seed::from(&bump_bytes),
    ];
    let signer = Signer::from(&seed);
    pinocchio_token::instructions::Transfer {
        from: vault,
        to: maker_ata_a,
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
