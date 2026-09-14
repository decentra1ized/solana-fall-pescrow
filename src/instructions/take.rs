use pinocchio::{
    cpi::{Seed, Signer},
    error::ProgramError,
    AccountView, ProgramResult,
};
use pinocchio_pubkey::derive_address;

use crate::state::Escrow;

pub fn process_take_instruction(accounts: &mut [AccountView], _data: &[u8]) -> ProgramResult {
    let [taker, maker, mint_a, mint_b, escrow_account, vault, taker_ata_a, taker_ata_b, maker_ata_b, system_program, token_program, _associated_token_program] =
        accounts
    else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    // ------------------------------------------------------------
    // 1. The taker must sign.
    // ------------------------------------------------------------
    if !taker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    // ------------------------------------------------------------
    // 2. The escrow must actually belong to our program.
    // ------------------------------------------------------------
    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::IllegalOwner);
    }

    // ------------------------------------------------------------
    // 3. Read + validate escrow state.
    //
    // IMPORTANT:
    // Keep this inside its own scope so RefMut is dropped before CPI.
    // ------------------------------------------------------------
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

    // ------------------------------------------------------------
    // 4. Re-derive and verify escrow PDA using stored bump.
    // ------------------------------------------------------------
    let bump_bytes = [bump];

    let expected_escrow = derive_address(
        &[b"escrow", maker.address().as_ref(), &bump_bytes],
        None,
        &crate::ID.to_bytes(),
    );

    if expected_escrow != escrow_account.address().to_bytes() {
        return Err(ProgramError::InvalidSeeds);
    }

    // ------------------------------------------------------------
    // 5. Validate vault.
    //
    // Vault must:
    // - belong to escrow PDA
    // - hold Mint A
    //
    // We transfer the actual vault balance.
    // ------------------------------------------------------------
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

    // ------------------------------------------------------------
    // 6. Validate taker's Mint B source account.
    // ------------------------------------------------------------
    {
        let taker_ata_b_state = pinocchio_token::state::Account::from_account_view(taker_ata_b)?;

        if taker_ata_b_state.owner() != taker.address() {
            return Err(ProgramError::IllegalOwner);
        }

        if taker_ata_b_state.mint() != mint_b.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    // ------------------------------------------------------------
    // 7. Create taker's Mint A ATA if it doesn't exist.
    //
    // Taker pays for its creation.
    // ------------------------------------------------------------
    pinocchio_associated_token_account::instructions::CreateIdempotent {
        funding_account: taker,
        account: taker_ata_a,
        wallet: taker,
        mint: mint_a,
        system_program,
        token_program,
    }
    .invoke()?;

    // ------------------------------------------------------------
    // 8. Create maker's Mint B ATA if it doesn't exist.
    //
    // The taker is funding this ATA creation as part of Take.
    // ------------------------------------------------------------
    pinocchio_associated_token_account::instructions::CreateIdempotent {
        funding_account: taker,
        account: maker_ata_b,
        wallet: maker,
        mint: mint_b,
        system_program,
        token_program,
    }
    .invoke()?;

    // ------------------------------------------------------------
    // 9. Taker pays Mint B to maker.
    //
    // Taker signed the transaction, so invoke(), not invoke_signed().
    // ------------------------------------------------------------
    pinocchio_token::instructions::Transfer {
        from: taker_ata_b,
        to: maker_ata_b,
        authority: taker,
        multisig_signers: &[] as &[&AccountView],
        amount: amount_to_receive,
    }
    .invoke()?;

    // ------------------------------------------------------------
    // 10. Build escrow PDA signer.
    // ------------------------------------------------------------
    let seeds = [
        Seed::from(b"escrow"),
        Seed::from(maker.address().as_array()),
        Seed::from(&bump_bytes),
    ];

    let signer = Signer::from(&seeds);

    // ------------------------------------------------------------
    // 11. Transfer all Mint A from vault -> taker.
    //
    // Vault authority = escrow PDA.
    // Therefore invoke_signed().
    // ------------------------------------------------------------
    pinocchio_token::instructions::Transfer {
        from: vault,
        to: taker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }
    .invoke_signed(&[signer.clone()])?;

    // ------------------------------------------------------------
    // 12. Close vault.
    //
    // Vault rent goes back to maker.
    // ------------------------------------------------------------
    pinocchio_token::instructions::CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }
    .invoke_signed(&[signer])?;

    // ------------------------------------------------------------
    // 13. Close escrow account manually.
    //
    // This account belongs to our program, so no CPI is required.
    // Move lamports first or the runtime reports an unbalanced
    // instruction.
    // ------------------------------------------------------------
    maker.set_lamports(maker.lamports() + escrow_account.lamports());

    escrow_account.set_lamports(0);

    escrow_account.close()?;

    Ok(())
}
