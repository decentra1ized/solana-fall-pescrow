use pinocchio::{
    AccountView, ProgramResult,
    cpi::{Seed, Signer},
    error::ProgramError,
};
use pinocchio_associated_token_account::instructions::CreateIdempotent;
use pinocchio_pubkey::derive_address;
use pinocchio_token::{
    instructions::{CloseAccount, Transfer},
    state::Account,
};

use crate::state::Escrow;

/// Accounts for Take:
/// 0. `[signer, writable]` taker: Pays fees, signs transaction
/// 1. `[writable]` maker: Receives rent refunds
/// 2. `[]` mint_a: Mint of token deposited by maker
/// 3. `[]` mint_b: Mint of token required by maker
/// 4. `[writable]` escrow_account: PDA, to be closed
/// 5. `[writable]` vault: ATA(escrow PDA, mint A), to be closed
/// 6. `[writable]` taker_ata_a: ATA(taker, mint A), destination for token A
/// 7. `[writable]` taker_ata_b: ATA(taker, mint B), source of token B
/// 8. `[writable]` maker_ata_b: ATA(maker, mint B), destination for token B
/// 9. `[]` system_program
/// 10. `[]` token_program
/// 11. `[]` associated_token_program
pub fn process_take_instruction(accounts: &mut [AccountView]) -> ProgramResult {
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
        _associated_token_program @ ..,
    ] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    // 1. Check taker is signer
    if !taker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    // 2. Verify escrow account is owned by this program
    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::IllegalOwner);
    }

    // 3. Cross-check accounts against Escrow state and extract values
    // Scoped so the mutable borrow on the escrow account is released before CPIs
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

    // 4. Re-derive PDA with canonical bump
    let derived_pda = derive_address(
        &[b"escrow", maker.address().as_ref(), &[bump]],
        None,
        &crate::ID.to_bytes(),
    );

    if &derived_pda != escrow_account.address().as_ref() {
        return Err(ProgramError::InvalidSeeds);
    }

    // 5. Validate vault ATA
    let vault_amount = {
        let vault_state = Account::from_account_view(vault)?;
        if vault_state.owner() != escrow_account.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if vault_state.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        vault_state.amount()
    };

    // 6. Ensure destination ATAs exist (idempotent creation funded by taker)
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

    // Validate taker source ATA for Mint B
    {
        let taker_ata_b_state = Account::from_account_view(taker_ata_b)?;
        if taker_ata_b_state.owner() != taker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if taker_ata_b_state.mint() != mint_b.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    // 7. CPI #1: Taker pays Maker amount_to_receive of Mint B
    Transfer {
        from: taker_ata_b,
        to: maker_ata_b,
        authority: taker,
        multisig_signers: &[] as &[&AccountView],
        amount: amount_to_receive,
    }
    .invoke()?;

    // 8. Build PDA Signer and CPI #2: Vault transfers Mint A to Taker
    let bump_bytes = [bump];
    let seed = [
        Seed::from(b"escrow"),
        Seed::from(maker.address().as_array()),
        Seed::from(&bump_bytes),
    ];
    let signer = Signer::from(&seed);

    Transfer {
        from: vault,
        to: taker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }
    .invoke_signed(&[signer.clone()])?;

    // 9. CPI #3: Close Vault ATA, refunding rent to Maker
    CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }
    .invoke_signed(&[signer.clone()])?;

    // 10. Close Escrow account, transferring lamports to Maker
    maker.set_lamports(maker.lamports() + escrow_account.lamports());
    escrow_account.set_lamports(0);
    escrow_account.close()?;

    Ok(())
}
