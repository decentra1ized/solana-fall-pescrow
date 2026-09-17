// Accounts required:
// 0: taker (Signer, pays fees)
// 1: maker (Receives rent refunds)
// 2: mint_a
// 3: mint_b
// 4: escrow_account (PDA)
// 5: vault (ATA)
// 6: taker_ata_a (Destination for A)
// 7: taker_ata_b (Source of B)
// 8: maker_ata_b (Destination for B)
// 9: system_program
// 10: token_program
// 11: associated_token_program

use pinocchio::{
    AccountView, 
    ProgramResult, 
    error::ProgramError
};
use pinocchio_pubkey::derive_address;

pub fn process_take_instruction(
    accounts: &mut [AccountView],
    _data: &[u8],
) -> ProgramResult {
    // 1. Destructure the accounts by their exact position
    let [
        taker,                  // 0: Signer, pays fees
        maker,                  // 1: Receives rent refunds
        mint_a,                 // 2: The token being bought
        mint_b,                 // 3: The token being paid
        escrow_account,         // 4: PDA to be closed
        vault,                  // 5: ATA vault to be closed
        taker_ata_a,            // 6: Destination for A
        taker_ata_b,            // 7: Source of B
        maker_ata_b,            // 8: Destination for B
        _system_program,        // 9
        _token_program,         // 10
        _associated_token,      // 11
    ] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    // 2. Manual Security Check: Did the taker actually sign?
    if !taker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    // 3. Load the Escrow State and verify cross-checks
    // We scope this so the RefMut guard is released before we do any CPIs.
    let (amount_to_receive, bump) = {
        // SECURITY: Is this account actually owned by our program?
        if !escrow_account.owned_by(&crate::ID) {
            return Err(ProgramError::InvalidAccountData);
        }

        let escrow_state = crate::state::escrow::Escrow::load_mut(escrow_account)?;
        
        // SECURITY: Do the provided accounts match the written state?
        if escrow_state.maker() != *maker.address() { return Err(ProgramError::InvalidAccountData); }
        if escrow_state.mint_a() != *mint_a.address() { return Err(ProgramError::InvalidAccountData); }
        if escrow_state.mint_b() != *mint_b.address() { return Err(ProgramError::InvalidAccountData); }
        
        (escrow_state.amount_to_receive(), escrow_state.bump)
    };

    // 4. Re-derive the PDA fast using the stored bump
    let derived_address = derive_address(
        &[b"escrow", maker.address().as_ref(), &[bump]], 
        None, 
        &crate::ID.to_bytes()
    ); // <-- Removed the .map_err() and the ?

    if derived_address != *escrow_account.address().as_ref() {
        return Err(ProgramError::InvalidSeeds);
    }

    // 5. Validate the vault and capture its TRUE balance
    let vault_amount = {
        let vault_state = pinocchio_token::state::Account::from_account_view(vault)?;
        if vault_state.owner() != escrow_account.address() || vault_state.mint() != mint_a.address() {
            return Err(ProgramError::IllegalOwner);
        }
        vault_state.amount() // Extract the exact balance to prevent stranded tokens
    };

    // Validate taker's source ATA (taker_ata_b)
    {
        let taker_ata_b_state = pinocchio_token::state::Account::from_account_view(taker_ata_b)?;
        if taker_ata_b_state.owner() != taker.address() || taker_ata_b_state.mint() != mint_b.address() {
            return Err(ProgramError::IllegalOwner);
        }
    }

    // 6. Ensure destination ATAs exist (Idempotent)
    pinocchio_associated_token_account::instructions::CreateIdempotent {
        funding_account: taker,
        account: taker_ata_a,
        wallet: taker,
        mint: mint_a,
        token_program: _token_program,
        system_program: _system_program,
    }.invoke()?;

    pinocchio_associated_token_account::instructions::CreateIdempotent {
        funding_account: taker,
        account: maker_ata_b,
        wallet: maker,
        mint: mint_b,
        token_program: _token_program,
        system_program: _system_program,
    }.invoke()?;

    // Re-build the PDA signer using the stored bump
    let bump_bytes = [bump];
    let seed = [
        pinocchio::cpi::Seed::from(b"escrow"),
        pinocchio::cpi::Seed::from(maker.address().as_array()),
        pinocchio::cpi::Seed::from(&bump_bytes)
    ];
    let seeds = pinocchio::cpi::Signer::from(&seed);

    // 7. Taker pays Token B to Maker (Signed by Taker)
    pinocchio_token::instructions::Transfer {
        from: taker_ata_b,
        to: maker_ata_b,
        authority: taker,
        multisig_signers: &[] as &[&AccountView],
        amount: amount_to_receive,
    }.invoke()?;

    // 8. Escrow releases Token A to Taker (Signed by Escrow PDA)
    pinocchio_token::instructions::Transfer {
        from: vault,
        to: taker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount, // <-- CRITICAL FIX: Using true vault balance
    }.invoke_signed(&[seeds.clone()])?;

    // 9. Close the Vault ATA (Signed by Escrow PDA)
    pinocchio_token::instructions::CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }.invoke_signed(&[seeds.clone()])?;

    // 10. Close the Escrow state account itself (Rent reclaimed by Maker)
    
    // Step A: Find out how much money (lamports) everyone has right now
    let escrow_lamports = escrow_account.lamports();
    let maker_lamports = maker.lamports();
    
    // Step B: Move the money by doing basic math
    maker.set_lamports(maker_lamports + escrow_lamports);
    escrow_account.set_lamports(0);
    
    // Step C: Wipe the account data clean so Solana knows it's closed
    escrow_account.close()?;

    Ok(())
}