// Take: the taker pays the maker in mint B and receives the vault's mint A.
// Instruction data is the discriminator `1` only; the terms come from the escrow account.
//
// Account order is the API here: there is no IDL, so the test must pass these in
// exactly this order.
//
//   #   Account                    W  S  Notes
//   0   taker                      x  x  Pays fees; its ATA for A is created if missing
//   1   maker                      x     Receives the rent refunds. Must equal escrow.maker()
//   2   mint_a                           Must equal escrow.mint_a()
//   3   mint_b                           Must equal escrow.mint_b()
//   4   escrow_account             x     PDA ["escrow", maker], owned by us, closed here
//   5   vault                      x     ATA(escrow PDA, mint A), closed here
//   6   taker_ata_a                x     ATA(taker, mint A), destination for A. May be created
//   7   taker_ata_b                x     ATA(taker, mint B), source of B. Must exist and be funded
//   8   maker_ata_b                x     ATA(maker, mint B), destination for B. May be created
//   9   system_program
//  10   token_program
//  11   associated_token_program
use pinocchio::{
    cpi::{Seed, Signer},
    error::ProgramError,
    AccountView, ProgramResult,
};

use crate::state::Escrow;

pub fn process_take_instruction(accounts: &mut [AccountView], _: &[u8]) -> ProgramResult {
    let [taker, maker, mint_a, mint_b, escrow_account, vault, taker_ata_a, taker_ata_b, maker_ata_b, system_program, token_program, _associated_token_program @ ..] =
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

    // Cross-check the passed accounts against the terms stored at Make.
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

    // Re-derive the PDA from the bump Make stored: a single hash, no search.
    let escrow_pda = pinocchio_pubkey::derive_address(
        &[b"escrow", maker.address().as_ref(), &[bump]],
        None,
        &crate::ID.to_bytes(),
    );
    if escrow_pda != *escrow_account.address().as_array() {
        return Err(ProgramError::InvalidSeeds);
    }

    // Validate the vault: owner = escrow PDA, mint = mint A. Its balance is what the taker gets.
    let vault_amount;
    {
        let vault = pinocchio_token::state::Account::from_account_view(vault)?;
        if vault.owner() != escrow_account.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if vault.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        vault_amount = vault.amount();
    }

    // Make sure both destination ATAs exist. CreateIdempotent is a no-op if they already do.
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
        // The taker funds this one too: the maker does not sign Take, so nothing can be
        // debited from them here. The maker keeps the account once it exists.
        funding_account: taker,
        account: maker_ata_b,
        wallet: maker,
        mint: mint_b,
        token_program,
        system_program,
    }
    .invoke()?;

    // Validate taker_ata_b: owner = taker, mint = mint B.
    {
        let taker_ata_b_state = pinocchio_token::state::Account::from_account_view(taker_ata_b)?;
        if taker_ata_b_state.owner() != taker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if taker_ata_b_state.mint() != mint_b.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    // The taker pays the maker in B. The taker signed the transaction, so a plain invoke.
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

    // The vault pays the taker in A. Its authority is the escrow PDA, so this must be signed.
    pinocchio_token::instructions::Transfer {
        from: vault,
        to: taker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }
    .invoke_signed(&[signer.clone()])?;

    // An emptied token account still holds rent, so the token program closes it and
    // refunds the maker, who paid for it at Make.
    pinocchio_token::instructions::CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }
    .invoke_signed(&[signer.clone()])?;

    // The escrow account is owned by this program, so no CPI: move its lamports out first,
    // or the runtime rejects the instruction as unbalanced.
    maker.set_lamports(maker.lamports() + escrow_account.lamports());
    escrow_account.set_lamports(0);
    escrow_account.close()?;

    Ok(())
}
