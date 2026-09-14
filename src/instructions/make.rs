use pinocchio::{
    cpi::{Seed, Signer},
    error::ProgramError,
    sysvars::{rent::Rent, Sysvar},
    AccountView, Address, ProgramResult,
};
use pinocchio_system::instructions::CreateAccount;

use crate::state::Escrow;

/// 8 (amount_to_receive) + 8 (amount_to_give)
const MAKE_DATA_LEN: usize = 16;

pub fn process_make_instruction(accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    let [maker, mint_a, mint_b, escrow_account, maker_ata, escrow_ata, system_program, token_program, _associated_token_program @ ..] =
        accounts
    else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    // Scope the borrow so it is released before any CPI below borrows `maker_ata`.
    {
        let maker_ata_state = pinocchio_token::state::Account::from_account_view(maker_ata)?;
        if maker_ata_state.owner() != maker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if maker_ata_state.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    // Instruction data layout (after the discriminator byte):
    //   [0..8]   amount_to_receive: u64 (little-endian)
    //   [8..16]  amount_to_give: u64 (little-endian)
    if data.len() < MAKE_DATA_LEN {
        return Err(ProgramError::InvalidInstructionData);
    }
    let amount_to_receive = u64::from_le_bytes(data[0..8].try_into().unwrap());
    let amount_to_give = u64::from_le_bytes(data[8..16].try_into().unwrap());

    if !maker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    // Derive the canonical bump on-chain. Never trust a bump supplied by the client:
    // with seeds ["escrow", maker] a non-canonical bump would let one maker open several
    // "the" escrows, and any client using find_program_address would only see one of them.
    // This costs a few thousand CU once, at init. Take and Cancel will use the stored bump
    // with derive_address (a single hash) instead.
    let (escrow_account_pda, bump) =
        Address::find_program_address(&[b"escrow", maker.address().as_ref()], &crate::ID);
    if escrow_account_pda != *escrow_account.address() {
        return Err(ProgramError::InvalidSeeds);
    }

    let bump_bytes = [bump];
    let seed = [
        Seed::from(b"escrow"),
        Seed::from(maker.address().as_array()),
        Seed::from(&bump_bytes),
    ];
    let seeds = Signer::from(&seed);

    // Refuse to overwrite an escrow that already exists for this maker.
    if escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::AccountAlreadyInitialized);
    }

    CreateAccount {
        from: maker,
        to: escrow_account,
        lamports: Rent::get()?.try_minimum_balance(Escrow::LEN)?,
        space: Escrow::LEN as u64,
        owner: &crate::ID,
    }
    .invoke_signed(&[seeds.clone()])?;

    // Scoped so the mutable borrow on the escrow data is released before the CPIs below.
    {
        let mut escrow_state = Escrow::load_mut(escrow_account)?;
        escrow_state.set_maker(maker.address());
        escrow_state.set_mint_a(mint_a.address());
        escrow_state.set_mint_b(mint_b.address());
        escrow_state.set_amount_to_receive(amount_to_receive);
        escrow_state.set_amount_to_give(amount_to_give);
        escrow_state.bump = bump;
    }

    pinocchio_associated_token_account::instructions::Create {
        funding_account: maker,
        account: escrow_ata,
        wallet: escrow_account,
        mint: mint_a,
        token_program: token_program,
        system_program: system_program,
    }
    .invoke()?;

    pinocchio_token::instructions::Transfer {
        from: maker_ata,
        to: escrow_ata,
        authority: maker,
        multisig_signers: &[] as &[&AccountView],
        amount: amount_to_give,
    }
    .invoke()?;

    Ok(())
}
