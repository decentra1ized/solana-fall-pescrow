/* Accounts in particular order
* 0 - maker
* 1 - mint_a
* 2 - escrow_account
* 3 - vault
* 4 - maker_ata_a
* 5 - token_program
*/

 use pinocchio::{
    AccountView,ProgramResult, cpi::{Seed, Signer}, error::ProgramError};
use pinocchio_pubkey::derive_address;

use crate::state::Escrow;


pub fn process_cancel_instruction(accounts: &mut [AccountView]) -> ProgramResult{

        /* Task 1(cancel): Destructure. Check maker.is_signer() */

        let [maker, mint_a, escrow_account, vault, maker_ata_a, _token_program] = accounts else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };

        if !maker.is_signer(){
            return Err(ProgramError::MissingRequiredSignature); 
        }

        /* Task 2(cancel): Load Escrow State with `Escrow::load_mut`, verify program ownership, verify `escrow.maker() == maker.address()` and `escrow.mint_a == mint_a.address()`. Copy out bump and drop the guard. */

        if !escrow_account.owned_by(&crate::ID){
                return Err(ProgramError::AccountAlreadyInitialized);
            }

        let bump = {
            let escrow = Escrow::load_mut(escrow_account)?;
            if escrow.maker()  != *maker.address()  { return Err(ProgramError::InvalidAccountData); }
            if escrow.mint_a() != *mint_a.address() {return Err(ProgramError::InvalidAccountData); }
            escrow.bump
        };

        /* Task 3(cancel): Re-derive and check the PDA. */

        let derived = derive_address(&[b"escrow", maker.address().as_ref(), &[bump]], None, &crate::ID.to_bytes());

            if derived != *escrow_account.address().as_array() {
                return Err(ProgramError::InvalidSeeds);
            }

        /* Task 4(cancel): Validate the vault: owner = escrow PDA, mint = mint A. Read its balance, drop the borrow. Validate maker_ata_a: owner = maker, mint = mint A. */
        let vault_amount = {
        let vault_state = pinocchio_token::state::Account::from_account_view(vault)?;
        if vault_state.owner() != escrow_account.address() {
                    return Err(ProgramError::IllegalOwner);
                }
        if vault_state.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
                    
        }
        vault_state.amount()
        }; // <- borrow dropped here


        {
            let maker_ata_a_state = pinocchio_token::state::Account::from_account_view(maker_ata_a)?;
            if maker_ata_a_state.owner() != maker.address() {
                return Err(ProgramError::IllegalOwner);
            }
            if maker_ata_a_state.mint() != mint_a.address() {
                return Err(ProgramError::InvalidAccountData);
            }
        }

        /* Task 5(cancel): Build the PDA signer. */
         let bump_bytes = [bump];
        let seed   = [Seed::from(b"escrow"), Seed::from(maker.address().as_array()), Seed::from(&bump_bytes)];
        let signer = Signer::from(&seed);


        /* Task 6(cancel): Invoke the transfer instruction. */
        pinocchio_token::instructions::Transfer {
        from: vault,
        to: maker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
        }.invoke_signed(&[signer.clone()])?;


        /* Task 7(cancel): Close account */
        pinocchio_token::instructions::CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        }.invoke_signed(&[signer.clone()])?;


        /* Task 8(cancel): Close the account by hand */
        maker.set_lamports(maker.lamports() + escrow_account.lamports());
        escrow_account.set_lamports(0);
        escrow_account.close()?;


    Ok(())
}