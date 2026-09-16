// Cancel instruction — account order:
//
// #  Account                  W  S  Notes
// 0  maker                    ✓  ✓  Must sign. Must equal escrow.maker()
// 1  mint_a                          Must equal escrow.mint_a()
// 2  escrow_account           ✓     PDA, will be closed
// 3  vault                    ✓     Will be closed
// 4  maker_ata_a              ✓     Destination for the returned A
// 5  token_program

use pinocchio::{AccountView, ProgramResult, cpi::{Seed, Signer}, error::ProgramError};

use crate::state::Escrow;

pub fn process_cancel_instruction(
    accounts: &mut [AccountView],
    _data: &[u8],
) -> ProgramResult {
    // 1 · Destructure accounts by position
    let [
        maker,
        mint_a,
        escrow_account,
        vault,
        maker_ata_a,
        _token_program @ ..
    ] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    // Check the signer — only the maker can cancel
    if !maker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    // 2 · Verify escrow ownership
    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::IllegalOwner);
    }

    // 3 · Load escrow state, cross-check, and extract fields before dropping the borrow
    let bump = {
        let escrow = Escrow::load_mut(escrow_account)?;
        if escrow.maker() != *maker.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        if escrow.mint_a() != *mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        escrow.bump
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

    // 5 · Validate the vault: owner = escrow PDA, mint = mint A
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

    // Validate maker_ata_a: owner = maker, mint = mint A
    {
        let maker_ata_state = pinocchio_token::state::Account::from_account_view(maker_ata_a)?;
        if maker_ata_state.owner() != maker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if maker_ata_state.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    // 6 · Build PDA signer
    let bump_bytes = [bump];
    let seed = [
        Seed::from(b"escrow"),
        Seed::from(maker.address().as_array()),
        Seed::from(&bump_bytes),
    ];
    let signer = Signer::from(&seed);

    // 7 · CPI #1: transfer all A from vault back to maker
    pinocchio_token::instructions::Transfer {
        from: vault,
        to: maker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }.invoke_signed(&[signer.clone()])?;

    // 8 · CPI #2: close the vault, refund rent to maker
    pinocchio_token::instructions::CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }.invoke_signed(&[signer.clone()])?;

    // 9 · Close the escrow account (owned by our program, no CPI needed)
    let escrow_lamports = escrow_account.lamports();
    maker.set_lamports(maker.lamports() + escrow_lamports);
    escrow_account.set_lamports(0);
    escrow_account.close()?;

    Ok(())
}
