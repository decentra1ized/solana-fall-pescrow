use pinocchio::{
    AccountView,
    ProgramResult,
    cpi::{Seed, Signer},
    error::ProgramError,
    Address
};

use pinocchio_associated_token_account::instructions::CreateIdempotent;
use pinocchio_pubkey::derive_address;

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
        associated_token_program,
    ] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    // --------------------------------------------------
    // 1. Taker must sign
    // --------------------------------------------------

    if !taker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    // --------------------------------------------------
    // 2. Validate escrow and read its state
    // --------------------------------------------------

    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::IllegalOwner);
    }

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

        (
            escrow.amount_to_receive(),
            escrow.bump,
        )
    };

    // --------------------------------------------------
    // 3. Re-derive the escrow PDA
    // --------------------------------------------------

    let escrow_pda = derive_address(
        &[
            b"escrow",
            maker.address().as_ref(),
            &[bump],
        ],
        None,
        &crate::ID.to_bytes(),
    );

    let escrow_pda = derive_address(
        &[
            b"escrow",
            maker.address().as_ref(),
            &[bump],
        ],
        None,
        &crate::ID.to_bytes(),
    );
    
    if Address::from(escrow_pda) != *escrow_account.address() {
        return Err(ProgramError::InvalidSeeds);
    }

    // --------------------------------------------------
    // 4. Validate vault
    // --------------------------------------------------

    let vault_amount = {
        let vault_state =
            pinocchio_token::state::Account::from_account_view(vault)?;

        if vault_state.owner() != escrow_account.address() {
            return Err(ProgramError::IllegalOwner);
        }

        if vault_state.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }

        vault_state.amount()
    };

    // --------------------------------------------------
    // Create taker's ATA for Token A
    // --------------------------------------------------

    CreateIdempotent {
        funding_account: taker,
        account: taker_ata_a,
        wallet: taker,
        mint: mint_a,
        system_program,
        token_program,
    }
    .invoke()?;

    // --------------------------------------------------
    // Create maker's ATA for Token B
    // --------------------------------------------------

    CreateIdempotent {
        funding_account: taker,
        account: maker_ata_b,
        wallet: maker,
        mint: mint_b,
        system_program,
        token_program,
    }
    .invoke()?;

    // --------------------------------------------------
    // Validate taker's Token B account
    // --------------------------------------------------

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

    // --------------------------------------------------
    // 5. Taker pays maker Token B
    // --------------------------------------------------

    pinocchio_token::instructions::Transfer {
        from: taker_ata_b,
        to: maker_ata_b,
        authority: taker,
        multisig_signers: &[] as &[&AccountView],
        amount: amount_to_receive,
    }
    .invoke()?;

    // --------------------------------------------------
    // 6. Build PDA signer
    // --------------------------------------------------

    let bump_bytes = [bump];

    let seed = [
        Seed::from(b"escrow"),
        Seed::from(maker.address().as_array()),
        Seed::from(&bump_bytes),
    ];

    let signer = Signer::from(&seed);

    // --------------------------------------------------
    // 7. Vault sends Token A to taker
    // --------------------------------------------------

    pinocchio_token::instructions::Transfer {
        from: vault,
        to: taker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }
    .invoke_signed(&[signer.clone()])?;

    // --------------------------------------------------
    // 8. Close vault
    // --------------------------------------------------

    pinocchio_token::instructions::CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }
    .invoke_signed(&[signer.clone()])?;

    // --------------------------------------------------
    // 9. Close escrow account
    // --------------------------------------------------

    maker.set_lamports(
        maker.lamports() + escrow_account.lamports()
    );

    escrow_account.set_lamports(0);

    escrow_account.close()?;

    Ok(())
}