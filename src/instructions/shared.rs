use pinocchio::{
    AccountView, Address, ProgramResult,
    cpi::{Seed, Signer},
};

/// Moves the vault's whole balance to `destination`, closes the vault, and
/// closes the escrow, refunding all rent to the maker.
///
/// Take and Cancel differ only in who receives the tokens, so the tail of both
/// instructions lives here — one place to fix rather than two.
///
/// `maker_key` is passed separately rather than read from `maker` because the
/// PDA signer borrows it, and `maker` itself has to stay mutable for the
/// lamport move at the end.
pub fn drain_vault_and_close(
    escrow_account: &mut AccountView,
    vault: &AccountView,
    destination: &AccountView,
    maker: &mut AccountView,
    maker_key: &Address,
    bump: u8,
    vault_amount: u64,
) -> ProgramResult {
    // The vault's authority is the escrow PDA, so both token CPIs are signed
    // with the same three seeds Make used. The bump comes from stored state, so
    // this never repeats Make's find_program_address search.
    let bump_bytes = [bump];
    let seed = [
        Seed::from(b"escrow"),
        Seed::from(maker_key.as_array()),
        Seed::from(&bump_bytes),
    ];
    let signer = Signer::from(&seed);

    pinocchio_token::instructions::Transfer {
        from: vault,
        to: destination,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }
    .invoke_signed(&[signer.clone()])?;

    // An emptied token account still holds rent, so hand it back to the maker
    // who paid for it.
    pinocchio_token::instructions::CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }
    .invoke_signed(&[signer])?;

    // The escrow is owned by this program, so there is no CPI for it. The
    // lamport move must come first: closing without it leaves the instruction
    // unbalanced and the runtime rejects the whole transaction.
    maker.set_lamports(maker.lamports() + escrow_account.lamports());
    escrow_account.set_lamports(0);
    escrow_account.close()
}
