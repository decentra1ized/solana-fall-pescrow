// Take instruction — account order:
//
// #  Account                  W  S  Notes
// 0  taker                    ✓  ✓  Pays fees; funds ATAs if missing
// 1  maker                    ✓     Receives rent refunds. Must equal escrow.maker()
// 2  mint_a                          Must equal escrow.mint_a()
// 3  mint_b                          Must equal escrow.mint_b()
// 4  escrow_account           ✓     PDA ["escrow", maker], owned by this program, will be closed
// 5  vault                    ✓     ATA(escrow PDA, mint_a), will be closed
// 6  taker_ata_a              ✓     ATA(taker, mint_a), destination for A — may need creating
// 7  taker_ata_b              ✓     ATA(taker, mint_b), source of B — must exist with enough balance
// 8  maker_ata_b              ✓     ATA(maker, mint_b), destination for B — may need creating
// 9  system_program
// 10 token_program
// 11 associated_token_program

use pinocchio::{AccountView, ProgramResult, cpi::{Seed, Signer}, error::ProgramError};

use crate::state::Escrow;

pub fn process_take_instruction(
    accounts: &mut [AccountView],
    _data: &[u8],
) -> ProgramResult {
    // 1 · Destructure accounts by position
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

    // Check the signer
    if !taker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    // 2 · Load the escrow state and verify ownership
    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::IllegalOwner);
    }

    // 3 · Cross-check passed accounts against escrow state, then drop the borrow
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
    }; // ← RefMut guard dropped here

    // 4 · Re-derive the PDA from the stored bump (single hash, no loop)
    let derived = pinocchio_pubkey::derive_address(
        &[b"escrow", maker.address().as_ref()],
        Some(bump),
        crate::ID.as_array(),
    );
    if derived != escrow_account.address().to_bytes() {
        return Err(ProgramError::InvalidSeeds);
    }

    // 5 · Validate the vault: must be owned by escrow PDA and hold mint A
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

    // 6 · Ensure destination ATAs exist (idempotent create)
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

    // Validate taker_ata_b: owner = taker, mint = mint_b
    {
        let taker_ata_b_state = pinocchio_token::state::Account::from_account_view(taker_ata_b)?;
        if taker_ata_b_state.owner() != taker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if taker_ata_b_state.mint() != mint_b.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    // 7 · CPI #1: taker pays maker (transfer amount_to_receive of B)
    pinocchio_token::instructions::Transfer {
        from: taker_ata_b,
        to: maker_ata_b,
        authority: taker,
        multisig_signers: &[] as &[&AccountView],
        amount: amount_to_receive,
    }.invoke()?;

    // 8 · Build PDA signer, then CPI #2: vault pays taker (transfer all A)
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
    }.invoke_signed(&[signer.clone()])?;

    // 9 · CPI #3: close the vault (SPL token account), refund rent to maker
    pinocchio_token::instructions::CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }.invoke_signed(&[signer.clone()])?;

    // 10 · Close the escrow account (owned by our program, no CPI needed)
    maker.set_lamports(maker.lamports() + escrow_account.lamports());
    escrow_account.set_lamports(0);
    escrow_account.close()?;

    Ok(())
}
