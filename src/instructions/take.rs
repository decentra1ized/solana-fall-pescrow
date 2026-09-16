 /* Accounts in Order
 * 0 - taker
 * 1 - maker
 * 2 - mint_a
 * 3 - mint_b
 * 4 - escrow_account
 * 5 - vault
 * 6 - taker_ata_a
 * 7 - taker_ata_b
 * 8 - maker_ata_b
 * 9 - system_program
 * 10 - token_program
 * 11 - associated_token_program
 */

 use pinocchio::{
    AccountView,ProgramResult, cpi::{Seed, Signer}, error::ProgramError};
use pinocchio_pubkey::derive_address;

use crate::state::Escrow;


pub fn process_take_instruction(accounts: &mut [AccountView]) -> ProgramResult {

    /* Task 1(take): Destructure and check the signer */
    
    let [taker,
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
     _associated_token_program
    ] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    if !taker.is_signer(){
        return Err(ProgramError::MissingRequiredSignature);
    }


    /* Task 2(take): Load the escrow state, and trust it only after checking the owner */

    if !escrow_account.owned_by(&crate::ID){
        return Err(ProgramError::AccountAlreadyInitialized);
    }
    
    /* Task 3(take): Cross-check the passed accounts against the state */

        let (amount_to_receive, bump) = {
        let escrow = Escrow::load_mut(escrow_account)?;
        if escrow.maker()  != *maker.address()  { return Err(ProgramError::InvalidAccountData); }
        if escrow.mint_a() != *mint_a.address() { return Err(ProgramError::InvalidAccountData); }
        if escrow.mint_b() != *mint_b.address() { return Err(ProgramError::InvalidAccountData); }
        (escrow.amount_to_receive(), escrow.bump)
    };   // ← RefMut guard dropped here


    /* Task 4(take): Re-derive the PDA */

    let derived = derive_address(&[b"escrow", maker.address().as_ref(), &[bump]], None, &crate::ID.to_bytes());

    if derived != *escrow_account.address().as_array() {
        return Err(ProgramError::InvalidSeeds);
    }

    /* Task 5(take): Validate the vault */

    let vault_amount = {
        let vault_state =  pinocchio_token::state::Account::from_account_view(vault)?;

        if vault_state.owner() != escrow_account.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if vault_state.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        vault_state.amount()
    }; // <- Borrow dropped here.


    /* Task 6(take): Make sure the destination ATA exists */

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


    // Scoped so the borrow is released before any CPI below borrows taker_ata_b.
    {
        let taker_ata_b_state = pinocchio_token::state::Account::from_account_view(taker_ata_b)?;
        if taker_ata_b_state.owner() != taker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if taker_ata_b_state.mint() != mint_b.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    /* Task 7(take): CPI #1, taker pays maker */

    pinocchio_token::instructions::Transfer {
        from: taker_ata_b,
        to: maker_ata_b,
        authority: taker,
        multisig_signers: &[] as &[&AccountView],
        amount: amount_to_receive,
    }.invoke()?;

    /* Task 8(take): Build the PDA signer, then CPI #2, vault pays taker */
    let bump_bytes = [bump];
    let seed   = [Seed::from(b"escrow"), Seed::from(maker.address().as_array()), Seed::from(&bump_bytes)];
    let signer = Signer::from(&seed);

    pinocchio_token::instructions::Transfer {
        from: vault,
        to: taker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }.invoke_signed(&[signer.clone()])?;


    /* Task 9(take):  CPI #3, close the vault */
    pinocchio_token::instructions::CloseAccount {
    account: vault,
    destination: maker,
    authority: escrow_account,
    multisig_signers: &[] as &[&AccountView],
    }.invoke_signed(&[signer.clone()])?;


    /* Task 10(take): Close the escrow account, by hand*/

    maker.set_lamports(maker.lamports() + escrow_account.lamports());
    escrow_account.set_lamports(0);
    escrow_account.close()?;



Ok(())


    
    



}