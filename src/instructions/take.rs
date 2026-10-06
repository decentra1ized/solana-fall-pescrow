use pinocchio::{
    AccountView, Address, ProgramResult,
    cpi::{Seed, Signer},
    error::ProgramError,
};
use crate::state::Escrow;

pub fn process_take_instruction(accounts: &mut [AccountView], _data: &[u8]) -> ProgramResult {
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
        _associated_token_program @ ..
    ] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    if !taker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }
    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::IllegalOwner);
    }

    let (maker_key, mint_a_key, mint_b_key, amount_to_receive, bump) = {
        let escrow = Escrow::load_mut(escrow_account)?;
        (
            escrow.maker(),
            escrow.mint_a(),
            escrow.mint_b(),
            escrow.amount_to_receive(),
            escrow.bump,
        )
    };
    if maker_key != *maker.address() || mint_a_key != *mint_a.address() || mint_b_key != *mint_b.address() {
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

    pinocchio_token::instructions::Transfer {
        from: taker_ata_b,
        to: maker_ata_b,
        authority: taker,
        multisig_signers: &[] as &[&AccountView],
        amount: amount_to_receive,
    }
    .invoke()?;

    let vault_amount = pinocchio_token::state::Account::from_account_view(vault)?.amount();
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

    close_escrow(escrow_account, maker)
}

pub fn close_escrow(escrow: &mut AccountView, maker: &mut AccountView) -> ProgramResult {
    let rent = escrow.lamports();
    maker.set_lamports(maker.lamports().checked_add(rent).ok_or(ProgramError::ArithmeticOverflow)?);
    escrow.set_lamports(0);
    escrow.try_borrow_mut()?.fill(0);
    Ok(())
}
