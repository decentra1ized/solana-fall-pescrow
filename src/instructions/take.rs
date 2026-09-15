use pinocchio::{
    AccountView,
    ProgramResult,
    cpi::{Seed, Signer},
    error::ProgramError,
};
use crate::state::Escrow;
use pinocchio_pubkey::derive_address;

// Account order:
// 0  taker
// 1  maker
// 2  mint_a
// 3  mint_b
// 4  escrow_account
// 5  vault
// 6  taker_ata_a
// 7  taker_ata_b
// 8  maker_ata_b
// 9  system_program
// 10 token_program
// 11 associated_token_program
pub fn process_take_instruction(
    accounts: &mut [AccountView],
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
        ..
    ] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    // 1. The taker must authorize the trade.
    if !taker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    // 2. Never trust escrow data before checking who owns the account.
    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::InvalidAccountOwner);
    }

    // 3. Read and validate the stored deal terms.
    // Keep this borrow scoped: later CPIs need escrow_account again.
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

    // 4. Re-derive the escrow PDA using the stored bump.
    let escrow_pda = derive_address(
        &[b"escrow", maker.address().as_ref(), &[bump]],
        None,
        &crate::ID.to_bytes(),
    );

    if escrow_pda != escrow_account.address().to_bytes() {
        return Err(ProgramError::InvalidSeeds);
    }

    // 5. Validate the vault and remember how many tokens it holds.
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

    // 6. Make sure both destination ATAs exist.
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

    // The taker's B account is the source of payment.
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

    // 7. The taker pays the maker in token B.
    pinocchio_token::instructions::Transfer {
        from: taker_ata_b,
        to: maker_ata_b,
        authority: taker,
        multisig_signers: &[] as &[&AccountView],
        amount: amount_to_receive,
    }
    .invoke()?;

    // 8. The escrow PDA signs to release all token A from the vault.
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

    // 9. Close the now-empty token vault and refund its rent to the maker.
    pinocchio_token::instructions::CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }
    .invoke_signed(&[signer.clone()])?;

    // 10. Close the escrow account itself and refund its rent to the maker.
    maker.set_lamports(maker.lamports() + escrow_account.lamports());
    escrow_account.set_lamports(0);
    escrow_account.close()?;

    Ok(())
}