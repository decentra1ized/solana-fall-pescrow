//! The tail both Take and Cancel end with: drain the vault, close it, close the
//! escrow. Only the destination of the tokens differs between the two.

use pinocchio::{AccountView, ProgramResult, cpi::Signer};

/// Moves every token out of `vault` into `destination`, then closes the vault
/// and the escrow account, refunding both rents to `maker`.
///
/// `signer` must be the escrow PDA's seeds: the vault's authority is the PDA,
/// so neither the transfer nor the close can be a plain invoke.
pub fn settle(
    vault: &mut AccountView,
    destination: &mut AccountView,
    escrow_account: &mut AccountView,
    maker: &mut AccountView,
    amount: u64,
    signer: &Signer,
) -> ProgramResult {
    pinocchio_token::instructions::Transfer {
        from: vault,
        to: destination,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount,
    }.invoke_signed(&[signer.clone()])?;

    // An emptied token account still holds its rent.
    pinocchio_token::instructions::CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }.invoke_signed(&[signer.clone()])?;

    // The escrow is owned by this program, so no CPI: move the lamports first,
    // then close. Skip the move and the runtime rejects the instruction as
    // unbalanced — lamports can never simply vanish.
    maker.set_lamports(maker.lamports() + escrow_account.lamports());
    escrow_account.set_lamports(0);
    escrow_account.close()
}
