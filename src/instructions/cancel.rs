use pinocchio::{
    AccountView, ProgramResult,
    cpi::{Seed, Signer},
    error::ProgramError,
};

use crate::instructions::shared::drain_and_close;
use crate::state::Escrow;

/// Account order (the account order is the API; there is no IDL):
///
/// |  # | Account         | Writable | Signer |
/// | -- | --------------- | -------- | ------ |
/// |  0 | maker           | yes      | yes    |
/// |  1 | mint_a          |          |        |
/// |  2 | escrow_account  | yes      |        |
/// |  3 | vault           | yes      |        |
/// |  4 | maker_ata_a     | yes      |        |
/// |  5 | token_program   |          |        |
///
/// Instruction data: the discriminator byte only.
pub fn process_cancel_instruction(accounts: &mut [AccountView]) -> ProgramResult {

    let [
        maker,
        mint_a,
        escrow_account,
        vault,
        maker_ata_a,
        _token_program,
        ..
    ] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    // Three checks carry the whole authorization. `is_signer` proves who sent
    // the transaction. The stored maker proves it is that signer's escrow. The
    // PDA derivation proves the escrow is this program's, for this maker.
    if !maker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::IllegalOwner);
    }

    let bump = {
        let escrow_state = Escrow::load_mut(escrow_account)?;
        if escrow_state.maker() != *maker.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        if escrow_state.mint_a() != *mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        escrow_state.bump
    };

    let maker_key = maker.address().to_bytes();
    let bump_bytes = [bump];

    let seeds: [&[u8]; 3] = [b"escrow", &maker_key, &bump_bytes];
    if pinocchio_pubkey::derive_address(&seeds, None, &crate::ID.to_bytes())
        != escrow_account.address().to_bytes()
    {
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
        let maker_ata_a_state = pinocchio_token::state::Account::from_account_view(maker_ata_a)?;
        if maker_ata_a_state.owner() != maker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if maker_ata_a_state.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    let seed = [
        Seed::from(b"escrow"),
        Seed::from(&maker_key),
        Seed::from(&bump_bytes),
    ];
    let signer = Signer::from(&seed);

    drain_and_close(vault, maker_ata_a, escrow_account, maker, vault_amount, &[signer])
}
