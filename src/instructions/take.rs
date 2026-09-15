use pinocchio::{
    AccountView, ProgramResult, cpi::{Seed, Signer}, error::ProgramError,
};
use pinocchio_pubkey::derive_address;

use crate::state::Escrow;

// Account order (the API — keep this comment and the test in sync):
//   0  taker                     (w, s) pays fees; funds its own ATA for A if missing
//   1  maker                     (w)    receives rent refunds; must equal escrow.maker()
//   2  mint_a                           must equal escrow.mint_a()
//   3  mint_b                           must equal escrow.mint_b()
//   4  escrow_account            (w)    PDA, owned by this program, will be closed
//   5  vault                     (w)    ATA(escrow PDA, mint A), will be closed
//   6  taker_ata_a               (w)    ATA(taker, mint A), destination for A, may need creating
//   7  taker_ata_b               (w)    ATA(taker, mint B), source of B
//   8  maker_ata_b               (w)    ATA(maker, mint B), destination for B, may need creating
//   9  system_program
//  10  token_program
//  11  associated_token_program
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

    // 1 · signer check
    if !taker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    // 2 · only trust escrow state after confirming this program owns the account
    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::InvalidAccountData);
    }

    // 3 · cross-check passed accounts against stored state, then drop the RefMut
    //     before any CPI touches escrow_account.
    let (amount_to_receive, bump) = {
        let escrow = Escrow::load_mut(escrow_account)?;
        if escrow.maker()  != *maker.address()  { return Err(ProgramError::InvalidAccountData); }
        if escrow.mint_a() != *mint_a.address() { return Err(ProgramError::InvalidAccountData); }
        if escrow.mint_b() != *mint_b.address() { return Err(ProgramError::InvalidAccountData); }
        (escrow.amount_to_receive(), escrow.bump)
    };

    // 4 · re-derive the PDA from the stored bump (single hash, no search)
    let derived = derive_address(
        &[b"escrow", maker.address().as_ref(), &[bump]],
        None,
        crate::ID.as_array(),
    );
    if derived != *escrow_account.address().as_array() {
        return Err(ProgramError::InvalidSeeds);
    }

    // 5 · validate the vault and read its live balance
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

    // 6 · make sure the destination ATAs exist (idempotent create)
    pinocchio_associated_token_account::instructions::CreateIdempotent {
        funding_account: taker,
        account: taker_ata_a,
        wallet: taker,
        mint: mint_a,
        system_program,
        token_program,
    }.invoke()?;

    pinocchio_associated_token_account::instructions::CreateIdempotent {
        funding_account: taker,
        account: maker_ata_b,
        wallet: maker,
        mint: mint_b,
        system_program,
        token_program,
    }.invoke()?;

    {
        let taker_ata_b_state = pinocchio_token::state::Account::from_account_view(taker_ata_b)?;
        if taker_ata_b_state.owner() != taker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if taker_ata_b_state.mint() != mint_b.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    // 7 · CPI #1 — taker pays maker (plain invoke, taker signed the tx)
    pinocchio_token::instructions::Transfer {
        from: taker_ata_b,
        to: maker_ata_b,
        authority: taker,
        multisig_signers: &[] as &[&AccountView],
        amount: amount_to_receive,
    }.invoke()?;

    // 8 · build the PDA signer, CPI #2 — vault pays taker
    let bump_bytes = [bump];
    let seed = [Seed::from(b"escrow"), Seed::from(maker.address().as_array()), Seed::from(&bump_bytes)];
    let signer = Signer::from(&seed);

    pinocchio_token::instructions::Transfer {
        from: vault,
        to: taker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }.invoke_signed(&[signer.clone()])?;

    // 9 · CPI #3 — close the vault, rent to maker
    pinocchio_token::instructions::CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }.invoke_signed(&[signer.clone()])?;

    // 10 · close the escrow account by hand (program-owned, no CPI for this)
    maker.set_lamports(maker.lamports() + escrow_account.lamports());
    escrow_account.set_lamports(0);
    escrow_account.close()?;

    Ok(())
}
