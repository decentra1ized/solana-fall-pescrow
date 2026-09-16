use crate::state::Escrow;
use pinocchio::{
    cpi::{Seed, Signer},
    error::ProgramError,
    AccountView, ProgramResult,
};
use pinocchio_associated_token_account::instructions::CreateIdempotent;
use pinocchio_token::instructions::{CloseAccount, Transfer};

/// Accounts expected by `Take`, in order:
/// 0. `[writable, signer]` taker
/// 1. `[writable]` maker
/// 2. `[]` mint A
/// 3. `[]` mint B
/// 4. `[writable]` escrow account PDA
/// 5. `[writable]` vault (escrow PDA's ATA for mint A)
/// 6. `[writable]` taker's ATA for mint A
/// 7. `[writable]` taker's ATA for mint B
/// 8. `[writable]` maker's ATA for mint B
/// 9. `[]` system program
/// 10. `[]` token program
/// 11. `[]` associated token program
pub fn process_take_instruction(accounts: &mut [AccountView], _data: &[u8]) -> ProgramResult {
    let [taker, maker, mint_a, mint_b, escrow_account, vault, taker_ata_a, taker_ata_b, maker_ata_b, system_program, token_program, _associated_token_program] =
        accounts
    else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    if !taker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

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

        (escrow.amount_to_receive(), escrow.bump)
    };

    let bump_bytes = [bump];
    let expected_escrow = pinocchio_pubkey::derive_address(
        &[b"escrow", maker.address().as_ref(), &bump_bytes],
        None,
        &crate::ID.to_bytes(),
    );

    if pinocchio::Address::from(expected_escrow) != *escrow_account.address() {
        return Err(ProgramError::InvalidSeeds);
    }

    let seed = [
        Seed::from(b"escrow"),
        Seed::from(maker.address().as_array()),
        Seed::from(&bump_bytes),
    ];

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

    CreateIdempotent {
        funding_account: taker,
        account: taker_ata_a,
        wallet: taker,
        mint: mint_a,
        token_program,
        system_program,
    }
    .invoke()?;

    CreateIdempotent {
        funding_account: taker,
        account: maker_ata_b,
        wallet: maker,
        mint: mint_b,
        token_program,
        system_program,
    }
    .invoke()?;

    {
        let taker_ata_b_state = pinocchio_token::state::Account::from_account_view(taker_ata_b)?;

        if taker_ata_b_state.owner() != taker.address() {
            return Err(ProgramError::IllegalOwner);
        }

        if taker_ata_b_state.mint() != mint_b.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    Transfer {
        from: taker_ata_b,
        to: maker_ata_b,
        authority: taker,
        multisig_signers: &[] as &[&AccountView],
        amount: amount_to_receive,
    }
    .invoke()?;

    let signer = Signer::from(&seed);
    Transfer {
        from: vault,
        to: taker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }
    .invoke_signed(&[signer.clone()])?;

    CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }
    .invoke_signed(&[signer.clone()])?;

    maker.set_lamports(maker.lamports() + escrow_account.lamports());
    escrow_account.set_lamports(0);
    escrow_account.close()?;

    Ok(())
}
