// needs 12 accounts in order

// 0	taker	✓	✓	Pays fees; funds its own ATA for A if missing
// 1	maker	✓		Receives rent refunds. Must equal escrow.maker()
// 2	mint_a			Must equal escrow.mint_a()
// 3	mint_b			Must equal escrow.mint_b()
// 4	escrow_account	✓		PDA, owned by this program, will be closed
// 5	vault	✓		ATA(escrow PDA, mint A), will be closed
// 6	taker_ata_a	✓		ATA(taker, mint A), destination for A. May need creating
// 7	taker_ata_b	✓		ATA(taker, mint B), source of B. Must exist with enough balance
// 8	maker_ata_b	✓		ATA(maker, mint B), destination for B. May need creating
// 9	system_program			
// 10	token_program			
// 11	associated_token_program			


use pinocchio::{
    AccountView, ProgramResult, cpi::{Seed, Signer}, error::ProgramError,
};
use crate::state::Escrow;
use pinocchio_pubkey::derive_address;

pub fn process_take_instruction(
    accounts: &mut [AccountView],
    _data: &[u8],
) -> ProgramResult {
    let [
        taker,           // 0
        maker,           // 1
        mint_a,          // 2
        mint_b,          // 3
        escrow_account,  // 4
        vault,           // 5
        taker_ata_a,     // 6
        taker_ata_b,     // 7
        maker_ata_b,     // 8
        system_program,  // 9
        token_program,   // 10
        _associated_token_program @ .., // 11
    ] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    if !taker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }
    
    // stage 2: fake escrow accounts are not owned by this program
    if !escrow_account.owned_by(&crate::ID) {
        return Err(ProgramError::IllegalOwner);
    }
    
    // stage 3: read the deal, then drop the borrow
    let (amount_to_receive, bump) = {
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
    
    // stage 4: this PDA must be ["escrow", maker, stored bump]
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

    pinocchio_associated_token_account::instructions::CreateIdempotent {
        funding_account: taker,
        account: taker_ata_a,
        wallet: taker,
        mint: mint_a,
        token_program,
        system_program,
    }
    .invoke()?;

    pinocchio_associated_token_account::instructions::CreateIdempotent {
        funding_account: taker,
        account: maker_ata_b,
        wallet: maker,
        mint: mint_b,
        token_program,
        system_program,
    }
    .invoke()?;

    {
        let taker_ata_b_state = pinocchio_token::state::Account::from_account_view(taker_ata_b)?;
        if taker_ata_b_state.owner() != taker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if taker_ata_b_state.mint() != mint_b.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

   // 7 · taker pays maker (taker signed the tx → invoke, not invoke_signed)
    pinocchio_token::instructions::Transfer {
        from: taker_ata_b,
        to: maker_ata_b,
        authority: taker,
        multisig_signers: &[] as &[&AccountView],
        amount: amount_to_receive,
    }
    .invoke()?;

    // 8 · vault pays taker (vault authority is the PDA → invoke_signed)
    let bump_bytes = [bump];
    let seed = [
        Seed::from(b"escrow"),
        Seed::from(maker.address().as_array()),
        Seed::from(&bump_bytes),
    ];
    let signer = Signer::from(&seed);

    pinocchio_token::instructions::Transfer {
        from: vault,
        to: taker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }
    .invoke_signed(&[signer.clone()])?;

    // 9 · the vault is an SPL token account, so the token program closes it
    pinocchio_token::instructions::CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }.invoke_signed(&[signer.clone()])?;

    // 10 · the escrow is ours, so we close it ourselves
    maker.set_lamports(maker.lamports() + escrow_account.lamports());
    escrow_account.set_lamports(0);
    escrow_account.close()?;
    
    Ok(())
}

