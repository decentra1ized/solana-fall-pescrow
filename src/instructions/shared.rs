use pinocchio::{AccountView, ProgramResult, cpi::Signer};

/// Shared tail end of Take and Cancel: pay the vault's whole balance to
/// `destination_ata`, close the (now-empty) vault token account with its
/// rent going to `maker`, then close the escrow account by hand, refunding
/// its rent to `maker` too.
///
/// `escrow_account` and `maker` must not have any live borrow on their data
/// when this is called (see `Escrow::load_mut`'s docs): the CPIs below sign
/// with `escrow_account`, and `close()` refuses while a borrow is held.
pub fn drain_vault_and_close_escrow(
    vault: &AccountView,
    destination_ata: &AccountView,
    escrow_account: &mut AccountView,
    maker: &mut AccountView,
    vault_amount: u64,
    signer: &Signer,
) -> ProgramResult {
    pinocchio_token::instructions::Transfer {
        from: vault,
        to: destination_ata,
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
    .invoke_signed(&[signer.clone()])?;

    // The escrow is owned by this program, so there is no CPI to close it:
    // move its lamports to the maker, zero its own, then close() it.
    maker.set_lamports(maker.lamports() + escrow_account.lamports());
    escrow_account.set_lamports(0);
    escrow_account.close()?;

    Ok(())
}
