use pinocchio::{
    AccountView, ProgramResult, cpi::{Seed, Signer}, error::ProgramError
};
use pinocchio_associated_token_account::instructions::CreateIdempotent;
use pinocchio_token::instructions::{CloseAccount, Transfer};

use crate::state::Escrow;

/// Take carries no data beyond the discriminator: every term lives in the escrow account.
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

    // Never read state from an account this program does not own.
    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::InvalidAccountOwner);
    }

    // Copy what we need into locals and let the RefMut guard drop before any CPI.
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

    // Make already found the canonical bump and stored it, so one hash is enough here.
    let escrow_pda = pinocchio_pubkey::derive_address(
        &[b"escrow", maker.address().as_ref(), &[bump]],
        None,
        &crate::ID.to_bytes(),
    );
    if escrow_pda != escrow_account.address().to_bytes() {
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

    // The mint check matters: the token program only checks that source and destination
    // share a mint, so without it a taker could pay in any token.
    {
        let taker_ata_b_state = pinocchio_token::state::Account::from_account_view(taker_ata_b)?;
        if taker_ata_b_state.owner() != taker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if taker_ata_b_state.mint() != mint_b.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    // CreateIdempotent also checks that each address is the real ATA for (wallet, mint),
    // so a taker cannot redirect the maker's payment to an account of their own.
    CreateIdempotent {
        funding_account: taker,
        account: taker_ata_a,
        wallet: taker,
        mint: mint_a,
        system_program,
        token_program,
    }.invoke()?;

    CreateIdempotent {
        funding_account: taker,
        account: maker_ata_b,
        wallet: maker,
        mint: mint_b,
        system_program,
        token_program,
    }.invoke()?;

    Transfer {
        from: taker_ata_b,
        to: maker_ata_b,
        authority: taker,
        multisig_signers: &[] as &[&AccountView],
        amount: amount_to_receive,
    }.invoke()?;

    let bump_bytes = [bump];
    let seed = [Seed::from(b"escrow"), Seed::from(maker.address().as_array()), Seed::from(&bump_bytes)];
    let signer = Signer::from(&seed);

    Transfer {
        from: vault,
        to: taker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }.invoke_signed(&[signer.clone()])?;

    CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }.invoke_signed(&[signer])?;

    // Program-owned account: close it by hand. Move the lamports first or the
    // runtime rejects the instruction as unbalanced.
    maker.set_lamports(maker.lamports() + escrow_account.lamports());
    escrow_account.set_lamports(0);
    escrow_account.close()?;

    Ok(())
}
