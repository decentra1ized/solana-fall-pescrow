use pinocchio::{
    AccountView, ProgramResult,
    cpi::{Seed, Signer},
    error::ProgramError,
};
use pinocchio_associated_token_account::instructions::CreateIdempotent;
use pinocchio_pubkey::derive_address;

use crate::state::Escrow;

/// Accounts (exact order):
/// 0. taker (W, S)
/// 1. maker (W)
/// 2. mint_a
/// 3. mint_b
/// 4. escrow_account (W)
/// 5. vault (W)
/// 6. taker_ata_a (W)
/// 7. taker_ata_b (W)
/// 8. maker_ata_b (W)
/// 9. system_program
/// 10. token_program
/// 11. associated_token_program
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
    ] = accounts
    else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    if !taker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::IllegalOwner);
    }

    let (amount_to_receive, amount_to_give, bump) = {
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
        (escrow.amount_to_receive(), escrow.amount_to_give(), escrow.bump)
    };

    let derived = derive_address(
        &[b"escrow", maker.address().as_ref()],
        Some(bump),
        crate::ID.as_array(),
    );
    if derived != *escrow_account.address().as_array() {
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
    if vault_amount != amount_to_give {
        return Err(ProgramError::InvalidAccountData);
    }

    {
        let taker_b = pinocchio_token::state::Account::from_account_view(taker_ata_b)?;
        if taker_b.owner() != taker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if taker_b.mint() != mint_b.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        if taker_b.amount() < amount_to_receive {
            return Err(ProgramError::InsufficientFunds);
        }
    }

    CreateIdempotent {
        funding_account: taker,
        account: taker_ata_a,
        wallet: taker,
        mint: mint_a,
        system_program,
        token_program,
    }
    .invoke()?;

    CreateIdempotent {
        funding_account: taker,
        account: maker_ata_b,
        wallet: maker,
        mint: mint_b,
        system_program,
        token_program,
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

    let bump_bytes = [bump];
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
    escrow_account.close()?;

    Ok(())
}
