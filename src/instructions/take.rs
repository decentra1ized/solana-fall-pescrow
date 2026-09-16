use pinocchio::{
    account::AccountView,cpi::{Seed, Signer}, error::ProgramError, 
};

use pinocchio_token::{
    state::{Account as TokenAccount},
    instructions::{CloseAccount, Transfer},
};

use pinocchio_associated_token_account::instructions::CreateIdempotent;
use pinocchio_pubkey::derive_address;



use crate::state::Escrow;

pub fn process_take_instructions (accounts: &mut [AccountView]) -> Result<(), ProgramError> {
    let [
        taker,
        maker,
        mint_a,
        mint_b,
        escrow_account,
        vault_account,
        taker_ata_a,
        taker_ata_b,
        maker_ata_b,
        system_program,
        token_program,
        _associated_token_program,
    ] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    // 1. Taker's signature
    if !taker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    // 2. Validate the escrow account
    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::InvalidAccountOwner);
    }

    //3. Validate the escrow account data
    let (amount_to_receive,  bump) = {
        let escrow = Escrow::load_mut(escrow_account)?;

        if escrow.maker() != *maker.address() {
            return Err(ProgramError::InvalidAccountData);
        }

        if escrow.mint_a() != *mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }   

        if escrow.mint_b() != *mint_b.address() {
            return Err(ProgramError::InvalidAccountData);
        } 

        (escrow.amount_to_receive(), escrow.bump)
    };

    // 4. Rederive the escrow account PDA and validate it
    let escrow_address = derive_address(
        &[b"escrow", maker.address().as_ref(), &[bump],],
        None,
        &crate::ID.to_bytes(),
    );
    if escrow_address != escrow_account.address().to_bytes() {
        return Err(ProgramError::InvalidAccountData);
    }

    //5. Validate vault account
    let vault_amount = {
        let vault_account= TokenAccount::from_account_view(vault_account)?;
        if vault_account.owner() != escrow_account.address() {
            return Err(ProgramError::InvalidAccountData);
        }

        if vault_account.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }

        vault_account.amount()
    };

    
    // 6. Create maker's B ATA if necessary.
    CreateIdempotent {
        funding_account: taker,
        account: maker_ata_b,
        wallet: maker,
        mint: mint_b,
        token_program,
        system_program,
    }
    .invoke()?;

    CreateIdempotent {
        funding_account: taker,
        account: taker_ata_a,
        wallet: taker,
        mint: mint_a,
        token_program,
        system_program,
    }
    .invoke()?;

    // Validate taker's B ATA.
    {
        let taker_b_account = TokenAccount::from_account_view(taker_ata_b)?;

        if taker_b_account.owner() != taker.address() {
            return Err(ProgramError::InvalidAccountData);
        }

        if taker_b_account.mint() != mint_b.address() {
            return Err(ProgramError::InvalidAccountData);
        }

        if taker_b_account.amount() < amount_to_receive {
            return Err(ProgramError::InsufficientFunds);
        }
    }

    // 7. Taker pays maker 100 B.
    Transfer {
        from: taker_ata_b,
        to: maker_ata_b,
        authority: taker,
        multisig_signers: &[] as &[&AccountView],
        amount: amount_to_receive,
    }
    .invoke()?;

    // 8. Construct the escrow PDA signer.
    let bump_bytes = [bump];

    let seed = [
        Seed::from(b"escrow"),
        Seed::from(maker.address().as_array()),
        Seed::from(&bump_bytes),
    ];

    let signer = Signer::from(&seed);

    // Vault pays taker all of its A.
    Transfer {
        from: vault_account,
        to: taker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }
    .invoke_signed(&[signer.clone()])?;

    // 9. Close the now-empty vault and refund its rent to maker.
    CloseAccount {
        account: vault_account,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }
    .invoke_signed(&[signer.clone()])?;

    // 10. Close the escrow account manually.
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