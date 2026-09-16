use pinocchio::{
    AccountView,
    cpi::{Seed, Signer},
    error::ProgramError,
    ProgramResult,
};

use pinocchio_associated_token_account::instructions::CreateIdempotent;
use pinocchio_pubkey::derive_address;
use pinocchio_token::{
    instructions::{CloseAccount, Transfer},
    state::Account as TokenAccount,
};

use crate::state::Escrow;

pub fn process_take_instruction(
    accounts: &mut [AccountView],
    _data: &[u8],
) -> ProgramResult {
    let [
        taker,
        maker,
        mint_a,
        mint_b,
        escrow_account,
        vault,
        taker_ata_a,
        taker_ata_b,
        maker_ata_b,
        system_program,
        token_program,
        _associated_token_program,
    ] = accounts
    else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    // 1. Taker must sign
    if !taker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    // 2. Escrow must belong to our program
    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::InvalidAccountOwner);
    }

    // 3. Read and validate escrow state
    let (amount_to_receive, bump) = {
        let escrow = Escrow::load_mut(escrow_account)?;

        // 4. Make sure the maker must match the escrow's maker
        if escrow.maker() != *maker.address() {
            return Err(ProgramError::InvalidAccountData);
        }

        // 5. Make sure the mint_a matches the escrow's deposited mint
        if escrow.mint_a() != *mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }

        // 6. Make sure the mint_b matches the escrow's desired received mint
        if escrow.mint_b() != *mint_b.address() {
            return Err(ProgramError::InvalidAccountData);
        }

        (
            escrow.amount_to_receive(),
            escrow.bump,
        )
    };

    // 7. Re-derive the escrow PDA using the stored bump
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

    // 8. Validate the vault
    let vault_amount = {
        let vault_state = TokenAccount::from_account_view(vault)?;

        // Make sure the vault is owned by the escrow account
        if *vault_state.owner() != *escrow_account.address() {
            return Err(ProgramError::InvalidAccountData);
        }

        if *vault_state.mint() != *mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }

        vault_state.amount()
    };

    // 9. Create taker's Token A ATA if necessary
    CreateIdempotent {
        funding_account: taker,
        account: taker_ata_a,
        wallet: taker,
        mint: mint_a,
        token_program,
        system_program,
    }
    .invoke()?;

    // 10. Create maker's Token B ATA if necessary
    CreateIdempotent {
        funding_account: taker,
        account: maker_ata_b,
        wallet: maker,
        mint: mint_b,
        token_program,
        system_program,
    }
    .invoke()?;

    // 11. Validate taker's Token B ATA
    {
        let taker_ata_b_state = TokenAccount::from_account_view(taker_ata_b)?;

        if *taker_ata_b_state.owner() != *taker.address() {
            return Err(ProgramError::InvalidAccountData);
        }

        if *taker_ata_b_state.mint() != *mint_b.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    // 12. Bob/Taker sends Token B to Alice/Maker
    Transfer {
        from: taker_ata_b,
        to: maker_ata_b,
        authority: taker,
        multisig_signers: &[] as &[&AccountView],
        amount: amount_to_receive,
    }
    .invoke()?;

    // 13. Build PDA signer
    let bump_bytes = [bump];

    let seed = [
        Seed::from(b"escrow"),
        Seed::from(maker.address().as_array()),
        Seed::from(&bump_bytes),
    ];

    let signer = Signer::from(&seed);

    // 14. Vault sends Token A to taker
    Transfer {
        from: vault,
        to: taker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }
    .invoke_signed(&[signer.clone()])?;

    // 15. Close the vault and send rent to maker
    CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }
    .invoke_signed(&[signer.clone()])?;

    // 16. Close the escrow account and refund rent to maker
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
