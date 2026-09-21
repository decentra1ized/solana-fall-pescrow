use pinocchio::{AccountView, ProgramResult, cpi::Signer};
use pinocchio_token::instructions::{CloseAccount, Transfer};

/// Drain the vault into `destination`, close the vault, then close the escrow PDA.
///
/// Take and Cancel end the same way. The only differences are the token
/// destination and who receives the rent, so both are parameters.
///
/// * `vault` - the escrow's token account for mint A. It is emptied and closed.
/// * `destination` - the token account that receives the drained amount.
/// * `escrow_account` - the escrow PDA. It is closed by hand at the end.
/// * `rent_destination` - the wallet that receives the rent of both accounts.
/// * `amount` - the vault balance read by the caller.
/// * `signers` - the PDA signer built from the escrow seeds and the stored bump.
///
/// The caller must drop every `Ref`/`RefMut` on these accounts before it calls
/// this function. A live borrow makes the CPIs and `close()` fail with
/// `AccountBorrowFailed`.
pub fn drain_and_close(
    vault: &AccountView,
    destination: &AccountView,
    escrow_account: &mut AccountView,
    rent_destination: &mut AccountView,
    amount: u64,
    signers: &[Signer],
) -> ProgramResult {
    // The vault's authority is the escrow PDA, so both token CPIs are signed
    // with the seeds instead of a wallet signature.
    Transfer {
        from: vault,
        to: destination,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount,
    }.invoke_signed(signers)?;

    CloseAccount {
        account: vault,
        destination: rent_destination,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }.invoke_signed(signers)?;

    // The escrow account is owned by this program, so there is no CPI for it.
    // Move the lamports out first; the runtime rejects an instruction that
    // makes lamports disappear.
    rent_destination.set_lamports(rent_destination.lamports() + escrow_account.lamports());
    escrow_account.set_lamports(0);
    escrow_account.close()
}
