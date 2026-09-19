// =================================================================================
// ACCOUNT ORDER (the "API" - client must match this exactly)
// 0 taker (writable, signer)     - the person buying
// 1 maker (writable)             - the person selling, gets rent back
// 2 mint_a                       - token being sold
// 3 mint_b                       - token being paid
// 4 escrow_account (writable)    - the deal record, will be closed
// 5 vault (writable)             - the "safe" holding token A
// 6 taker_ata_a (writable)       - where taker will receive token A
// 7 taker_ata_b (writable)       - taker's source of token B
// 8 maker_ata_b (writable)       - where maker will receive token B
// 9 system_program
// 10 token_program
// 11 associated_token_program
// ================================================================================

// ================================================================================
// WHAT ARE ALL THESE "use" LINES AT THE TOP?
//
// "use" just means: "I'm going to use this tool below, so let me
// give it a short name instead of writing its full address every
// single time."
//
// There are TWO different kinds of things being "used" here:
//
// 1) EXTERNAL LIBRARIES (code someone else wrote, downloaded from
//    the internet): pinocchio, pinocchio_pubkey, pinocchio_token.
//    These are NOT files in our project  they live in a hidden
//    cache folder on the computer (~/.cargo/registry/), the same
//    way node_modules works for JavaScript. We never open or edit
//    them, we just borrow tools from them.
//
// 2) OUR OWN CODE (files we wrote ourselves in this same project):
//    `crate::state::Escrow`
//    The word "crate" means "this whole project". So this line
//    means: "go find the Escrow struct, which lives in
//    src/state/escrow.rs, inside the `state` folder of THIS project."
//    (Look at the file tree on the left in VS Code  there's a
//    `state` folder with `escrow.rs` inside it. That's exactly
//    where this comes from.)
// ================================================================================


use pinocchio::{
    AccountView,   // the type of account passed into programm
    cpi::{Seed, Signer},  // tools for building a PDA signature
    error::ProgramError, // the standart list of error types we can get in return
    ProgramResult,               // Did it work? yes or no
};
use pinocchio_pubkey::derive_address;  // tool to recompute a PDA address from seeds
use pinocchio_token::state::Account as TokenAccount; // let us read the tokens account data

use crate::state::Escrow; // My own struct from src/state/escrow.rs

// This function runs whenever someone sends a "Take" instruction (discriminator = 1).
// Think of it as: "Bob wants to accept Alice's trade offer."

pub fn process_take_instruction(
    accounts: &mut [AccountView],
    _data: &[u8],   // no extra data needed  everything required is already saved in escrow
) -> ProgramResult {
    
    
    // ------------------------------------------------------------
    // STEP 1 Unpack all 12 accounts, in order, no names attached.
    // If the caller sent fewer than 12 accounts, stop right away.
    // ------------------------------------------------------------
    
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
        _associated_token_program,
    ] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    
    // Make sure Bob (taker) actually signed this transaction with his real key.
    // Without this check, anyone could pretend to be "taker" without permission.
    
    if !taker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    // ------------------------------------------------------------
    // STEP 2 Before trusting anything stored INSIDE escrow_account,
    // make sure this account actually belongs to OUR program.
    // Otherwise someone could hand us a fake account with fake data.
    // ------------------------------------------------------------
    
    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::InvalidAccountData);
    }

    // ------------------------------------------------------------
    // STEP 3 Open escrow's data, check it matches what was passed in
    // (same maker, same two tokens), then copy the numbers we need
    // into plain variables.
    //
    // WHY THE { } BRACES MATTER:
    // load_mut() temporarily "locks" the escrow account's data so we
    // can read/write it safely (this is called a "borrow"). While that
    // lock is held, nothing else can touch this account  including
    // the token transfers we do later in this same function.
    // The { } block makes that lock automatically release the moment
    // we leave it, BEFORE we try any transfers. Skip this and the
    // program crashes with "AccountBorrowFailed".
    // ------------------------------------------------------------

    
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
        // Read what we'll need later, right now, while we still have access.
        (escrow.amount_to_receive(), escrow.bump)
    }; // <- the lock on escrow's data ends exactly here

    
    // ------------------------------------------------------------
    // STEP 4  Recompute the escrow's address (PDA) using the bump
    // that was ALREADY calculated and saved back in Make.
    //
    // We don't search for the bump again (that would cost extra
    // compute)  we just plug in the saved bump and check the math
    // still lines up. This proves escrow_account really is the one
    // that belongs to THIS maker.
    // ------------------------------------------------------------
    
    let expected = derive_address(
        &[b"escrow", maker.address().as_ref(), &[bump]],
        None,
        &crate::ID.to_bytes(),
    );
    if expected != *escrow_account.address().as_array() {

        return Err(ProgramError::InvalidSeeds);
    }

    // ------------------------------------------------------------
    // STEP 5  Look inside the vault (the token "safe") and read how
    // much token A is actually sitting there right now. Same borrow
    // pattern as Step 3: open, read, close the { } block.
    // ------------------------------------------------------------
    let vault_amount = {
        let vault_state = TokenAccount::from_account_view(vault)?;
        if vault_state.owner() != escrow_account.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if vault_state.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        vault_state.amount()
    }; // <- borrow released here too

    // ------------------------------------------------------------
    // STEP 6  Make sure Bob's and Alice's receiving accounts (ATAs)
    // exist. "CreateIdempotent" is safe to call even if the account
    // already exists  it just does nothing in that case, instead of
    // erroring.
    // ------------------------------------------------------------
    pinocchio_associated_token_account::instructions::CreateIdempotent {
        funding_account: taker,
        account: taker_ata_a,
        wallet: taker,
        mint: mint_a,
        token_program,
        system_program,
    }.invoke()?;

    pinocchio_associated_token_account::instructions::CreateIdempotent {
        funding_account: taker,  // Bob pays if a new account needs creating
        account: maker_ata_b,
        wallet: maker,
        mint: mint_b,
        token_program,
        system_program,
    }.invoke()?;

    // Double-check taker's token-B account really belongs to taker and
    // holds the right kind of token, the same way Make checked maker's account.

   
    {
        let s = TokenAccount::from_account_view(taker_ata_b)?;
        if s.owner() != taker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if s.mint() != mint_b.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    // ------------------------------------------------------------
    // STEP 7  CPI #1: Bob pays Alice.
    // This is a plain transfer, because Bob himself signed this whole
    // transaction  so this money movement is authorized by a real
    // human wallet, not a PDA. That's why it's `invoke()`, not `invoke_signed()`.
    // ------------------------------------------------------------
    
    pinocchio_token::instructions::Transfer {
        from: taker_ata_b,
        to: maker_ata_b,
        authority: taker,
        multisig_signers: &[] as &[&AccountView],
        amount: amount_to_receive,
    }.invoke()?;

    // ------------------------------------------------------------
    // STEP 8  Build "proof" that our program can act as the escrow
    // PDA, then use it: CPI #2, the vault pays Bob.
    //
    // A PDA has no private key, so it can't "sign" the normal way.
    // Instead, the program proves it knows the exact seeds that
    // produce this PDA's address  that IS the signature.
    // ------------------------------------------------------------
    
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
        authority: escrow_account,  // the PDA is the authority here
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,   // the REAL balance we read in Step 5, not amount_to_give
    }.invoke_signed(&[signer.clone()])?;

    // ------------------------------------------------------------
    // STEP 9 CPI #3: close the now-empty vault. Closing a token
    // account refunds its rent (the small deposit that paid for its
    // storage)  that refund goes to the maker, who originally paid
    // for it back in Make.
    // ------------------------------------------------------------
    pinocchio_token::instructions::CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }.invoke_signed(&[signer])?;

    // ------------------------------------------------------------
    // STEP 10  Close the escrow account itself. This one is owned
    // directly by OUR program (not the token program), so there's no
    // CPI for it  we do it by hand:
    //   1. Move its lamports (its rent deposit) to maker's balance
    //   2. Zero out the escrow account's own lamports
    //   3. Call close() to mark the account as done/empty
    //
    // Order matters: the runtime checks that total lamports balance
    // out, so if you skip moving the lamports first, the transaction
    // gets rejected as "unbalanced."
    // ------------------------------------------------------------
    let maker_lamports = maker.lamports();
    let escrow_lamports = escrow_account.lamports();
    maker.set_lamports(maker_lamports + escrow_lamports);

    escrow_account.set_lamports(0);
    escrow_account.close()?;
    Ok(())

}