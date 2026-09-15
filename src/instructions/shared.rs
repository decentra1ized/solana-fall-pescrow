use pinocchio::{
    AccountView, Address, ProgramResult, cpi::{Seed, Signer}, error::ProgramError,
};

/// Re-derive the escrow PDA from the stored canonical bump (single hash, no
/// bump search loop). The bump was computed and stored by `Make`.
pub fn derive_escrow_pda(maker: &AccountView, bump: u8) -> Address {
    let derived = pinocchio_pubkey::derive_address(
        &[b"escrow", maker.address().as_ref(), &[bump]],
        None,
        &crate::ID.to_bytes(),
    );
    Address::from(derived)
}

/// Drain the vault's full balance to `destination`, then close the vault
/// (rent refunded to `maker`) and close the escrow account by hand.
///
/// Shared tail of `Take` and `Cancel`. Callers are responsible for the
/// authorization checks (maker matches stored state, PDA re-derivation,
/// signer checks) before calling this.
pub fn dismiss_escrow(
    escrow_account: &mut AccountView,
    vault: &mut AccountView,
    destination: &mut AccountView,
    maker: &mut AccountView,
    mint_a: &AccountView,
    bump: u8,
) -> ProgramResult {
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

    let bump_bytes = [bump];
    let seed = [Seed::from(b"escrow"), Seed::from(maker.address().as_array()), Seed::from(&bump_bytes)];
    let signer = Signer::from(&seed);

    pinocchio_token::instructions::Transfer {
        from: vault,
        to: destination,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }.invoke_signed(&[signer.clone()])?;

    pinocchio_token::instructions::CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }.invoke_signed(&[signer.clone()])?;

    maker.set_lamports(maker.lamports() + escrow_account.lamports());
    escrow_account.set_lamports(0);
    escrow_account.close()?;

    Ok(())
}