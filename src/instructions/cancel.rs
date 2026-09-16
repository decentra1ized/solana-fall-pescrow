use pinocchio::{
    AccountView,
    cpi::{Seed, Signer},
    error::ProgramError,
    ProgramResult,
};

use pinocchio_pubkey::derive_address;

use pinocchio_token::{
    instructions::{CloseAccount, Transfer},
    state::Account as TokenAccount,
};

use crate::state::Escrow;

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
        _token_program,
    ] = accounts
    else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    // 1. Maker must sign
    if !maker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    // 2. Escrow must belong to our program
    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::InvalidAccountOwner);
    }

    // 3. Read and validate escrow state
    let bump = {
        let escrow = Escrow::load_mut(escrow_account)?;

        // Maker must be the maker stored in the escrow
        if escrow.maker() != *maker.address() {
            return Err(ProgramError::InvalidAccountData);
        }

        // Mint A must be the mint stored in the escrow
        if escrow.mint_a() != *mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }

        escrow.bump
    };

    // 4. Re-derive the escrow PDA using the stored bump
    let escrow_address = derive_address(
        &[
            b"escrow",
            maker.address().as_ref(),
            &[bump],
        ],
        None,
        &crate::ID.to_bytes(),
    );

    if escrow_address != escrow_account.address().to_bytes() {
        return Err(ProgramError::InvalidSeeds);
    }

    // 5. Validate the vault and read its balance
    let vault_amount = {
        let vault_state = TokenAccount::from_account_view(vault)?;

        // Vault must be owned by the escrow PDA
        if *vault_state.owner() != *escrow_account.address() {
            return Err(ProgramError::InvalidAccountData);
        }

        // Vault must contain Token A
        if *vault_state.mint() != *mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }

        vault_state.amount()
    };

    // 6. Validate maker's Token A ATA
    {
        let maker_ata_state = TokenAccount::from_account_view(maker_ata_a)?;

        if *maker_ata_state.owner() != *maker.address() {
            return Err(ProgramError::InvalidAccountData);
        }

        if *maker_ata_state.mint() != *mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    // 7. Build the escrow PDA signer
    let bump_bytes = [bump];

    let seed = [
        Seed::from(b"escrow"),
        Seed::from(maker.address().as_array()),
        Seed::from(&bump_bytes),
    ];

    let signer = Signer::from(&seed);

    // 8. Return all Token A from the vault to the maker
    Transfer {
        from: vault,
        to: maker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }
    .invoke_signed(&[signer.clone()])?;

    // 9. Close the vault and refund its rent to the maker
    CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }
    .invoke_signed(&[signer.clone()])?;

    // 10. Close the escrow account and refund its rent to the maker
    maker.set_lamports(
        maker
            .lamports()
            .checked_add(escrow_account.lamports())
            .ok_or(ProgramError::ArithmeticOverflow)?,
    );

    escrow_account.set_lamports(0);

    escrow_account.close()?;

    Ok(())
}
