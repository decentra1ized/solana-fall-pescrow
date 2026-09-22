use pinocchio::{AccountView, ProgramResult, cpi::Signer, error::ProgramError};
use pinocchio_token::instructions::{CloseAccount, Transfer};

/// Check a token account belongs to `owner` and holds `mint`, and return its balance.
pub fn check_token_account(
    account: &AccountView,
    owner: &pinocchio::Address,
    mint: &pinocchio::Address,
) -> Result<u64, ProgramError> {
    let state = pinocchio_token::state::Account::from_account_view(account)?;
    if state.owner() != owner {
        return Err(ProgramError::IllegalOwner);
    }
    if state.mint() != mint {
        return Err(ProgramError::InvalidAccountData);
    }
    Ok(state.amount())
}

/// Send everything left in the vault to `destination`, close the vault, then close the
/// escrow by hand. Both CPIs are signed by the escrow PDA, which owns the vault.
pub fn drain_and_close(
    vault: &mut AccountView,
    destination: &mut AccountView,
    escrow_account: &mut AccountView,
    rent_destination: &mut AccountView,
    amount: u64,
    signer: Signer,
) -> ProgramResult {
    Transfer {
        from: vault,
        to: destination,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount,
    }.invoke_signed(&[signer.clone()])?;

    CloseAccount {
        account: vault,
        destination: rent_destination,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }.invoke_signed(&[signer])?;

    // The escrow is owned by this program, so there is no CPI for it: move the lamports
    // out first, otherwise the runtime rejects the instruction as unbalanced.
    rent_destination.set_lamports(rent_destination.lamports() + escrow_account.lamports());
    escrow_account.set_lamports(0);
    escrow_account.close()
}
