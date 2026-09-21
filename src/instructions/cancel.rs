// needs 6 accounts in order
//
// 0	maker		✓	✓	Must sign. Must equal escrow.maker()
// 1	mint_a				Must equal escrow.mint_a()
// 2	escrow_account	✓		PDA, owned by this program, will be closed
// 3	vault		✓		ATA(escrow PDA, mint A), will be closed
// 4	maker_ata_a	✓		ATA(maker, mint A), destination for returned A
// 5	token_program

use pinocchio::{
    AccountView, ProgramResult, cpi::{Seed, Signer}, error::ProgramError,
};
use crate::state::Escrow;
use pinocchio_pubkey::derive_address;

pub fn process_cancel_instruction(
    accounts: &mut [AccountView],
    _data: &[u8],
) -> ProgramResult {
    let [
        maker,           // 0
        mint_a,          // 1
        escrow_account,  // 2
        vault,           // 3
        maker_ata_a,     // 4
        _token_program @ .., // 5
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

    let derived = derive_address(
        &[b"escrow", maker.address().as_ref(), &[bump]],
        None,
        &crate::ID.to_bytes(),
    );
    if pinocchio::Address::from(derived) != *escrow_account.address() {
        return Err(ProgramError::InvalidSeeds);
    }

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

    {
        let maker_ata_a_state = pinocchio_token::state::Account::from_account_view(maker_ata_a)?;
        if maker_ata_a_state.owner() != maker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if maker_ata_a_state.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    let bump_bytes = [bump];
    let seed = [
        Seed::from(b"escrow"),
        Seed::from(maker.address().as_array()),
        Seed::from(&bump_bytes),
    ];
    let signer = Signer::from(&seed);

    pinocchio_token::instructions::Transfer {
        from: vault,
        to: maker_ata_a,
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

    maker.set_lamports(maker.lamports() + escrow_account.lamports());
    escrow_account.set_lamports(0);
    escrow_account.close()?;

    Ok(())
}
