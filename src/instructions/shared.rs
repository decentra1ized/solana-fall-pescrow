use pinocchio::{AccountView, ProgramResult, cpi::Signer};

/// Helper to drain the vault, close the vault, and close the escrow account.
pub fn drain_and_close_vault_and_escrow<'a>(
    maker: &mut AccountView,
    escrow_account: &mut AccountView,
    vault: &mut AccountView,
    destination_ata: &mut AccountView,
    vault_amount: u64,
    signer: Signer<'a, 'a>,
) -> ProgramResult {
    // Drain the vault
    pinocchio_token::instructions::Transfer {
        from: vault,
        to: destination_ata,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }.invoke_signed(&[signer.clone()])?;

    // Close the vault
    pinocchio_token::instructions::CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }.invoke_signed(&[signer.clone()])?;

    // Close the escrow account
    maker.set_lamports(maker.lamports() + escrow_account.lamports());
    escrow_account.set_lamports(0);
    escrow_account.close()?;

    Ok(())
}
