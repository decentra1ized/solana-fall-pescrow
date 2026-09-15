use pinocchio::{
    AccountView, Address, ProgramResult,
    cpi::{Seed, Signer},
    error::ProgramError,
};
use pinocchio_pubkey::derive_address;

use crate::state::Escrow;

/// Take instruction:
///
/// Accounts:
/// 0: `[signer, writable]`  taker
/// 1: `[writable]`          maker (receives rent refunds; must equal escrow.maker())
/// 2: `[]`                  mint_a (must equal escrow.mint_a())
/// 3: `[]`                  mint_b (must equal escrow.mint_b())
/// 4: `[writable]`          escrow_account (PDA, owned by program, closed)
/// 5: `[writable]`          vault (ATA(escrow PDA, mint_a), closed)
/// 6: `[writable]`          taker_ata_a (destination for token A, created idempotently)
/// 7: `[writable]`          taker_ata_b (source for token B, must hold balance)
/// 8: `[writable]`          maker_ata_b (destination for token B, created idempotently)
/// 9: `[]`                  system_program
/// 10: `[]`                 token_program
/// 11: `[]`                 associated_token_program
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
        _associated_token_program @ ..
    ] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    if !taker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::InvalidAccountOwner);
    }

    // Cross-check passed accounts against state and extract needed values.
    // Scoped so the RefMut guard is released before any CPI.
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
    };

    // Re-derive the PDA using the stored bump (single hash, no find_program_address loop).
    let bump_bytes = [bump];
    let expected_escrow = derive_address(
        &[b"escrow", maker.address().as_ref(), &bump_bytes],
        None,
        &crate::ID.to_bytes(),
    );
    if Address::from(expected_escrow) != *escrow_account.address() {
        return Err(ProgramError::InvalidSeeds);
    }

    // Validate the vault and read token balance.
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

    // Idempotently create destination ATAs funded by taker.
    pinocchio_associated_token_account::instructions::CreateIdempotent {
        funding_account: taker,
        account: taker_ata_a,
        wallet: taker,
        mint: mint_a,
        token_program,
        system_program,
    }.invoke()?;

    pinocchio_associated_token_account::instructions::CreateIdempotent {
        funding_account: taker,
        account: maker_ata_b,
        wallet: maker,
        mint: mint_b,
        token_program,
        system_program,
    }.invoke()?;

    // Validate taker's source ATA for token B.
    {
        let taker_ata_b_state = pinocchio_token::state::Account::from_account_view(taker_ata_b)?;
        if taker_ata_b_state.owner() != taker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if taker_ata_b_state.mint() != mint_b.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    // CPI 1: Taker pays maker (amount_to_receive of token B).
    pinocchio_token::instructions::Transfer {
        from: taker_ata_b,
        to: maker_ata_b,
        authority: taker,
        multisig_signers: &[] as &[&AccountView],
        amount: amount_to_receive,
    }.invoke()?;

    // Build the PDA signer for the escrow account.
    let seed = [
        Seed::from(b"escrow"),
        Seed::from(maker.address().as_array()),
        Seed::from(&bump_bytes),
    ];
    let signer = Signer::from(&seed);

    // CPI 2: Vault pays taker (vault_amount of token A).
    pinocchio_token::instructions::Transfer {
        from: vault,
        to: taker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }.invoke_signed(&[signer.clone()])?;

    // CPI 3: Close the vault and return rent to maker.
    pinocchio_token::instructions::CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }.invoke_signed(&[signer.clone()])?;

    // Close the escrow account: transfer lamports to maker, zero balance, close.
    maker.set_lamports(maker.lamports() + escrow_account.lamports());
    escrow_account.set_lamports(0);
    escrow_account.close()?;

    Ok(())
}
