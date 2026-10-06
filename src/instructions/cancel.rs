use pinocchio::{AccountView, Address, ProgramResult, cpi::{Seed, Signer}, error::ProgramError};
use crate::instructions::take::close_escrow;
use crate::state::Escrow;

pub fn process_cancel_instruction(accounts: &mut [AccountView], _data: &[u8]) -> ProgramResult {
    let [
        maker,
        mint_a,
        escrow_account,
        maker_ata_a,
        vault,
        token_program,
        _rest @ ..
    ] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    if !maker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }
    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::IllegalOwner);
    }

    let (stored_maker, stored_mint_a, bump) = {
        let escrow = Escrow::load_mut(escrow_account)?;
        (escrow.maker(), escrow.mint_a(), escrow.bump)
    };
    if stored_maker != *maker.address() || stored_mint_a != *mint_a.address() {
        return Err(ProgramError::InvalidAccountData);
    }

    let bump_bytes = [bump];
    let pda = Address::derive_address(
        &[b"escrow", maker.address().as_ref()],
        Some(bump),
        &crate::ID,
    );
    if pda != *escrow_account.address() {
        return Err(ProgramError::InvalidSeeds);
    }

    let seed = [
        Seed::from(b"escrow"),
        Seed::from(maker.address().as_array()),
        Seed::from(&bump_bytes),
    ];
    let signer = Signer::from(&seed);
    let amount = pinocchio_token::state::Account::from_account_view(vault)?.amount();

    pinocchio_token::instructions::Transfer {
        from: vault,
        to: maker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount,
    }
    .invoke_signed(&[signer.clone()])?;

    pinocchio_token::instructions::CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }
    .invoke_signed(&[signer])?;

    let _ = token_program;
    close_escrow(escrow_account, maker)
}
