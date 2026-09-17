use pinocchio::{
    AccountView, ProgramResult,
    cpi::{Seed, Signer},
    error::ProgramError,
};

use crate::state::Escrow;

// Accounts (position-based API, keep in sync with tests):
//   0. taker            [WRITE, SIGNER] pays fees; funds its own ATA for A if missing
//   1. maker            [WRITE]         receives rent refunds; must equal escrow.maker()
//   2. mint_a           []              must equal escrow.mint_a()
//   3. mint_b           []              must equal escrow.mint_b()
//   4. escrow_account   [WRITE]         PDA ["escrow", maker], owned by this program, will be closed
//   5. vault            [WRITE]         ATA(escrow PDA, mint A), will be closed
//   6. taker_ata_a      [WRITE]         ATA(taker, mint A), destination for A (created if missing)
//   7. taker_ata_b      [WRITE]         ATA(taker, mint B), source of B (must exist, funded)
//   8. maker_ata_b      [WRITE]         ATA(maker, mint B), destination for B (created if missing)
//   9. system_program   []
//   10. token_program   []
//   11. associated_token_program []
//
// Instruction data: just the discriminator byte `1` (handled in lib.rs dispatch).

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
        _associated_token_program,
    ] = accounts
    else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    if !taker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    // The escrow must be owned by this program before we trust anything inside it.
    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::IllegalOwner);
    }

    // Cross-check passed accounts against stored state, then drop the borrow
    // before any CPI touches `escrow_account`.
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

    // Re-derive the PDA from the stored (canonical) bump: single hash, no loop.
    // `create_program_address` additionally validates the address is off-curve.
    let expected_escrow = pinocchio::Address::create_program_address(
        &[b"escrow", maker.address().as_ref(), &[bump]],
        &crate::ID,
    )
    .map_err(|_| ProgramError::InvalidSeeds)?;
    if expected_escrow != *escrow_account.address() {
        return Err(ProgramError::InvalidSeeds);
    }

    // Validate the vault and read how much A the taker receives.
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

    // Destination ATAs may not exist yet; create idempotently, funded by taker.
    pinocchio_associated_token_account::instructions::CreateIdempotent {
        funding_account: taker,
        account: taker_ata_a,
        wallet: taker,
        mint: mint_a,
        system_program,
        token_program,
    }
    .invoke()?;

    pinocchio_associated_token_account::instructions::CreateIdempotent {
        funding_account: taker,
        account: maker_ata_b,
        wallet: maker,
        mint: mint_b,
        system_program,
        token_program,
    }
    .invoke()?;

    // Validate the taker's source ATA for mint B.
    {
        let taker_ata_b_state = pinocchio_token::state::Account::from_account_view(taker_ata_b)?;
        if taker_ata_b_state.owner() != taker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if taker_ata_b_state.mint() != mint_b.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    // CPI #1: taker pays maker in mint B (taker signed, plain invoke).
    pinocchio_token::instructions::Transfer {
        from: taker_ata_b,
        to: maker_ata_b,
        authority: taker,
        multisig_signers: &[] as &[&AccountView],
        amount: amount_to_receive,
    }
    .invoke()?;

    // PDA signer for the vault / escrow authority.
    let bump_bytes = [bump];
    let seed = [
        Seed::from(b"escrow"),
        Seed::from(maker.address().as_array()),
        Seed::from(&bump_bytes),
    ];
    let signer = Signer::from(&seed);

    // CPI #2: vault pays taker in mint A (PDA signs).
    pinocchio_token::instructions::Transfer {
        from: vault,
        to: taker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }
    .invoke_signed(&[signer.clone()])?;

    // CPI #3: close the vault, refund rent to maker (PDA signs).
    pinocchio_token::instructions::CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }
    .invoke_signed(&[signer.clone()])?;

    // Close the escrow account by hand: move lamports first, then close.
    maker.set_lamports(maker.lamports() + escrow_account.lamports());
    escrow_account.set_lamports(0);
    escrow_account.close()?;

    Ok(())
}
