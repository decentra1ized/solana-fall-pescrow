use pinocchio::{
    AccountView,
    ProgramResult,
    Address,
    cpi::{Seed, Signer},
    error::ProgramError,
};

use crate::state::Escrow;

pub fn process_take_instruction(
    accounts: &mut [AccountView],
    _data: &[u8],
) -> ProgramResult {
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
        _associated_token_program,
    ] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    // Check whether taker signed
    if !taker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    // Check whether escrow account is owned by our program
    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::IllegalOwner);
    }

    // Reading escrow state
    let (amount_to_receive, bump);

    {
        let escrow_state = Escrow::load_mut(escrow_account)?;

        if escrow_state.maker() != *maker.address() {
            return Err(ProgramError::InvalidAccountData);
        }

        if escrow_state.mint_a() != *mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }

        if escrow_state.mint_b() != *mint_b.address() {
            return Err(ProgramError::InvalidAccountData);
        }

        amount_to_receive = escrow_state.amount_to_receive();
        bump = escrow_state.bump;
    }

    // PDA verification
    let bump_bytes = [bump];

    let escrow_account_pda = pinocchio_pubkey::derive_address(
        &[b"escrow", maker.address().as_ref(), &bump_bytes],
        None,
        &crate::ID.to_bytes(),
    );

    if Address::from(escrow_account_pda) != *escrow_account.address() {
        return Err(ProgramError::InvalidSeeds);
    }

    // Checking the vault
    let vault_amount;

    {
        let vault_state =
            pinocchio_token::state::Account::from_account_view(vault)?;

        if vault_state.owner() != escrow_account.address() {
            return Err(ProgramError::IllegalOwner);
        }

        if vault_state.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }

        vault_amount = vault_state.amount();
    }

    // Check taker's Token B account
    {
        let taker_ata_b_state =
            pinocchio_token::state::Account::from_account_view(taker_ata_b)?;

        if taker_ata_b_state.owner() != taker.address() {
            return Err(ProgramError::IllegalOwner);
        }

        if taker_ata_b_state.mint() != mint_b.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    // Create taker's ATA for token A
    pinocchio_associated_token_account::instructions::CreateIdempotent {
        funding_account: taker,
        account: taker_ata_a,
        wallet: taker,
        mint: mint_a,
        token_program: token_program,
        system_program: system_program,
    }
    .invoke()?;

    // Create maker's ATA for token B
    pinocchio_associated_token_account::instructions::CreateIdempotent {
        funding_account: taker,
        account: maker_ata_b,
        wallet: maker,
        mint: mint_b,
        token_program: token_program,
        system_program: system_program,
    }
    .invoke()?;

    // Token B: taker -> maker
    pinocchio_token::instructions::Transfer {
        from: taker_ata_b,
        to: maker_ata_b,
        authority: taker,
        multisig_signers: &[] as &[&AccountView],
        amount: amount_to_receive,
    }
    .invoke()?;

    // Reconstruct PDA signer
    let seed = [
        Seed::from(b"escrow"),
        Seed::from(maker.address().as_array()),
        Seed::from(&bump_bytes),
    ];

    let seeds = Signer::from(&seed);

    // Token A: vault -> taker
    pinocchio_token::instructions::Transfer {
        from: vault,
        to: taker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }
    .invoke_signed(&[seeds.clone()])?;

    // Close vault
    pinocchio_token::instructions::CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }
    .invoke_signed(&[seeds])?;

    // Close escrow account
    maker.set_lamports(maker.lamports() + escrow_account.lamports());
    escrow_account.set_lamports(0);
    escrow_account.close()?;

    Ok(())
}