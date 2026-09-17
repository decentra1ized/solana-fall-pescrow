// Accounts required:
// 0: maker (Signer)
// 1: mint_a
// 2: escrow_account (PDA)
// 3: vault (ATA)
// 4: maker_ata_a (Destination for A)
// 5: token_program

use pinocchio::{
    AccountView, 
    ProgramResult, 
    error::ProgramError
};
use pinocchio_pubkey::derive_address;

pub fn process_cancel_instruction(
    accounts: &mut [AccountView],
    _data: &[u8],
) -> ProgramResult {
    // 1. Destructure accounts
    let [
        maker,
        mint_a,
        escrow_account,
        vault,
        maker_ata_a,
        _token_program,
    ] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    // 2. CRITICAL SECURITY: Ensure the maker signed this transaction
    if !maker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    // 3. Load Escrow State and verify cross-checks
    let bump = {
        if !escrow_account.owned_by(&crate::ID) {
            return Err(ProgramError::InvalidAccountData);
        }
        let escrow_state = crate::state::escrow::Escrow::load_mut(escrow_account)?;
        
        if escrow_state.maker() != *maker.address() { return Err(ProgramError::InvalidAccountData); }
        if escrow_state.mint_a() != *mint_a.address() { return Err(ProgramError::InvalidAccountData); }
        
        escrow_state.bump
    }; // RefMut guard drops here

    // 4. Re-derive the PDA fast using the stored bump
    let derived_address = derive_address(
        &[b"escrow", maker.address().as_ref(), &[bump]], 
        None, 
        &crate::ID.to_bytes()
    );

    if derived_address != *escrow_account.address().as_ref() {
        return Err(ProgramError::InvalidSeeds);
    }

    // 5. Validate the vault and maker's ATA, capture vault balance
    let vault_amount = {
        let vault_state = pinocchio_token::state::Account::from_account_view(vault)?;
        if vault_state.owner() != escrow_account.address() || vault_state.mint() != mint_a.address() {
            return Err(ProgramError::IllegalOwner);
        }
        vault_state.amount()
    };

    {
        let maker_ata_a_state = pinocchio_token::state::Account::from_account_view(maker_ata_a)?;
        if maker_ata_a_state.owner() != maker.address() || maker_ata_a_state.mint() != mint_a.address() {
            return Err(ProgramError::IllegalOwner);
        }
    }

    // 6. Build the PDA signer
    let bump_bytes = [bump];
    let seed = [
        pinocchio::cpi::Seed::from(b"escrow"),
        pinocchio::cpi::Seed::from(maker.address().as_array()),
        pinocchio::cpi::Seed::from(&bump_bytes)
    ];
    let seeds = pinocchio::cpi::Signer::from(&seed);

    // 7. Transfer Token A back from Vault to Maker (Signed by Escrow PDA)
    pinocchio_token::instructions::Transfer {
        from: vault,
        to: maker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }.invoke_signed(&[seeds.clone()])?;

    // 8. Close the Vault ATA (Signed by Escrow PDA)
    pinocchio_token::instructions::CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }.invoke_signed(&[seeds.clone()])?;

    // 9. Close the Escrow state account itself (Rent reclaimed by Maker)
    let escrow_lamports = escrow_account.lamports();
    let maker_lamports = maker.lamports();
    maker.set_lamports(maker_lamports + escrow_lamports);
    escrow_account.set_lamports(0);
    escrow_account.close()?;

    Ok(())
}