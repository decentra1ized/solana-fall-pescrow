// Account order (12 accounts, no names/IDL - this order IS the API):
//   0  taker                    (signer, writable) pays fees; funds its own ATA for A if missing
//   1  maker                    (writable)         receives rent refunds; must equal escrow.maker()
//   2  mint_a                                       must equal escrow.mint_a()
//   3  mint_b                                       must equal escrow.mint_b()
//   4  escrow_account           (writable)         PDA, owned by this program, will be closed
//   5  vault                    (writable)         ATA(escrow PDA, mint A), will be closed
//   6  taker_ata_a              (writable)         ATA(taker, mint A), destination for A. May need creating
//   7  taker_ata_b              (writable)         ATA(taker, mint B), source of B. Must exist with balance
//   8  maker_ata_b              (writable)         ATA(maker, mint B), destination for B. May need creating
//   9  system_program
//   10 token_program
//   11 associated_token_program

use pinocchio::{
    AccountView, cpi::{Seed, Signer}, error::ProgramError, ProgramResult,
};
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

    // Cross-check the passed accounts against the stored state, then drop the
    // borrow before any CPI touches escrow_account.
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

    // Re-derive from the stored bump - a single hash, no search.
    let bump_bytes = [bump];
    let derived = derive_address(
        &[b"escrow", maker.address().as_ref(), &bump_bytes],
        None,
        &crate::ID.to_bytes(),
    );
    if derived != *escrow_account.address().as_array() {
        return Err(ProgramError::InvalidSeeds);
    }

    // Validate the vault and read its balance, then drop the borrow.
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

    // Make sure the taker's ATA for A and the maker's ATA for B exist.
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

    // Validate taker_ata_b: owner = taker, mint = mint_b.
    {
        let taker_ata_b_state = pinocchio_token::state::Account::from_account_view(taker_ata_b)?;
        if taker_ata_b_state.owner() != taker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if taker_ata_b_state.mint() != mint_b.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    // CPI #1: taker pays maker, amount_to_receive of B. Authority is the taker,
    // who signed the transaction, so this is a plain invoke().
    pinocchio_token::instructions::Transfer {
        from: taker_ata_b,
        to: maker_ata_b,
        authority: taker,
        multisig_signers: &[] as &[&AccountView],
        amount: amount_to_receive,
    }.invoke()?;

    // Build the PDA signer for the vault's two CPIs below.
    let seed = [Seed::from(b"escrow"), Seed::from(maker.address().as_array()), Seed::from(&bump_bytes)];
    let seeds = Signer::from(&seed);

    // CPI #2: vault pays taker, the vault's real balance (not amount_to_give from state).
    pinocchio_token::instructions::Transfer {
        from: vault,
        to: taker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }.invoke_signed(&[seeds.clone()])?;

    // CPI #3: close the now-empty vault, rent goes to the maker who paid for it.
    pinocchio_token::instructions::CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }.invoke_signed(&[seeds.clone()])?;

    // Close the escrow account by hand: move its lamports to the maker, zero
    // its own, then close(). This must run after the guard from the earlier
    // block has already been dropped.
    {
        let escrow_lamports = escrow_account.lamports();
        let maker_lamports = maker.lamports();
        maker.set_lamports(maker_lamports + escrow_lamports);
        escrow_account.set_lamports(0);
        escrow_account.close()?;
    }

    Ok(())
}
