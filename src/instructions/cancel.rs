use pinocchio::{
    AccountView,
    ProgramResult,
    error::ProgramError,
};
use pinocchio_pubkey::derive_address;
use crate::state::Escrow;
use pinocchio_token::state::Account;
use pinocchio::cpi::{Seed, Signer};
use pinocchio_token::instructions::Transfer;
use pinocchio_token::instructions::CloseAccount;
pub fn process_cancel_instruction(
    accounts: &mut [AccountView],
    _data: &[u8],
) -> ProgramResult {
    let [
    maker,
    mint_a,
    escrow_account,
    vault,
    maker_ata_a,
    token_program,
] = accounts else {
    return Err(ProgramError::NotEnoughAccountKeys);
};
if !maker.is_signer() {
    return Err(ProgramError::MissingRequiredSignature);
}
if !escrow_account.owned_by(&crate::ID) {
    return Err(ProgramError::IllegalOwner);
}
let bump = {
    let escrow = Escrow::load_mut(escrow_account)?;

    if escrow.maker() != *maker.address() {
        return Err(ProgramError::InvalidAccountData);
    }

    if escrow.mint_a() != *mint_a.address() {
        return Err(ProgramError::InvalidAccountData);
    }

    escrow.bump
};
let escrow_pda = derive_address(
    &[
        b"escrow",
        maker.address().as_ref(),
        &[bump],
    ],
    None,
    &crate::ID.to_bytes(),
);

if escrow_pda != *escrow_account.address().as_array() {
    return Err(ProgramError::InvalidSeeds);
}
let vault_amount = {
    let vault_state = Account::from_account_view(vault)?;

    if vault_state.owner() != escrow_account.address() {
        return Err(ProgramError::IllegalOwner);
    }

    if vault_state.mint() != mint_a.address() {
        return Err(ProgramError::InvalidAccountData);
    }

    vault_state.amount()
};
let bump_bytes = [bump];

let seed = [
    Seed::from(b"escrow"),
    Seed::from(maker.address().as_array()),
    Seed::from(&bump_bytes),
];

let signer = Signer::from(&seed);
Transfer {
    from: vault,
    to: maker_ata_a,
    authority: escrow_account,
    multisig_signers: &[] as &[&AccountView],
    amount: vault_amount,
}
.invoke_signed(&[signer.clone()])?;
CloseAccount {
    account: vault,
    destination: maker,
    authority: escrow_account,
    multisig_signers: &[] as &[&AccountView],
}
.invoke_signed(&[signer.clone()])?;
let maker_lamports = maker.lamports();
let escrow_lamports = escrow_account.lamports();

maker.set_lamports(maker_lamports + escrow_lamports);
escrow_account.set_lamports(0);
escrow_account.close()?;

Ok(())
}