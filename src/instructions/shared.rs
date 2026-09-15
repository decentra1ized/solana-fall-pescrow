use pinocchio::{
    AccountView, ProgramResult, cpi::Signer, error::ProgramError,
};

/// Re-derive the escrow PDA from the stored bump (a single hash, no loop) and check it
/// matches the account that was passed in.
pub fn verify_escrow_pda(escrow_account: &AccountView, maker: &AccountView, bump: u8) -> ProgramResult {
    let derived = pinocchio_pubkey::derive_address(
        &[b"escrow".as_ref(), maker.address().as_ref(), &[bump]],
        None,
        &crate::ID.to_bytes(),
    );
    if &derived != escrow_account.address().as_array() {
        return Err(ProgramError::InvalidSeeds);
    }
    Ok(())
}

/// Validate an SPL token account's owner and mint, returning its balance.
/// The borrow is released before this returns, so the account is free for CPIs.
pub fn check_token_account(
    account: &AccountView,
    owner: &AccountView,
    mint: &AccountView,
) -> Result<u64, ProgramError> {
    let state = pinocchio_token::state::Account::from_account_view(account)?;
    if state.owner() != owner.address() {
        return Err(ProgramError::IllegalOwner);
    }
    if state.mint() != mint.address() {
        return Err(ProgramError::InvalidAccountData);
    }
    Ok(state.amount())
}

/// Tail shared by Take and Cancel: drain the vault into `destination`, close the vault
/// (rent to the maker), then close the program-owned escrow account by hand.
pub fn drain_vault_and_close(
    vault: &AccountView,
    destination: &AccountView,
    escrow_account: &mut AccountView,
    maker: &mut AccountView,
    vault_amount: u64,
    signer: Signer,
) -> ProgramResult {
    pinocchio_token::instructions::Transfer {
        from: vault,
        to: destination,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }
    .invoke_signed(&[signer.clone()])?;

    pinocchio_token::instructions::CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }
    .invoke_signed(&[signer])?;

    // Move the lamports first, or the runtime rejects the instruction as unbalanced.
    let refunded = maker
        .lamports()
        .checked_add(escrow_account.lamports())
        .ok_or(ProgramError::ArithmeticOverflow)?;
    maker.set_lamports(refunded);
    escrow_account.set_lamports(0);
    escrow_account.close()?;

    Ok(())
}
