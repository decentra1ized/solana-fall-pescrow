/// Accounts, in order:
/// 0  taker           (writable, signer)     pays fees; funds its own ATA for A if missing
/// 1  maker           (writable)             receives rent refunds; must equal escrow.maker()
/// 2  mint_a                                   must equal escrow.mint_a()
/// 3  mint_b                                   must equal escrow.mint_b()
/// 4  escrow_account  (writable)              PDA, owned by this program, will be closed
/// 5  vault           (writable)              ATA(escrow PDA, mint A), will be closed
/// 6  taker_ata_a     (writable)              ATA(taker, mint A), destination for A; may need creating
/// 7  taker_ata_b     (writable)              ATA(taker, mint B), source of B; must exist
/// 8  maker_ata_b     (writable)              ATA(maker, mint B), destination for B; may need creating
/// 9  system_program
/// 10 token_program
/// 11 associated_token_program

use pinocchio::{
    AccountView, ProgramResult,
    cpi::{Seed, Signer},
    error::ProgramError,
};
use pinocchio_token::state::Account as TokenAccount;

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
        _associated_token_program @ ..,
    ] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };

    // 1. Check signer
    if !taker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }

    // 2. Load escrow state, verify program ownership, cross-check accounts
    let (amount_to_receive, bump) = {
        if !escrow_account.owned_by(&crate::ID) {
            return Err(ProgramError::IncorrectProgramId);
        }
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
        let amount_to_receive = escrow.amount_to_receive();
        let bump = escrow.bump;
        (amount_to_receive, bump)
    };

    // 3. Validate vault
    let vault_amount = {
        let vault_state = TokenAccount::from_account_view(vault)?;
        if vault_state.owner() != escrow_account.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if vault_state.mint() != mint_a.address() {
            return Err(ProgramError::InvalidAccountData);
        }
        vault_state.amount()
    };

    // 4. Validate taker_ata_b
    {
        let taker_ata_b_state = TokenAccount::from_account_view(taker_ata_b)?;
        if taker_ata_b_state.owner() != taker.address() {
            return Err(ProgramError::IllegalOwner);
        }
        if taker_ata_b_state.mint() != mint_b.address() {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    // 5. Create destination ATAs if needed (idempotent)
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

    // 6. CPI #1: taker pays maker (transfer B)
    pinocchio_token::instructions::Transfer {
        from: taker_ata_b,
        to: maker_ata_b,
        authority: taker,
        multisig_signers: &[] as &[&AccountView],
        amount: amount_to_receive,
    }
    .invoke()?;

    // 7. Build PDA signer and CPI #2: vault pays taker (transfer A)
    let bump_bytes = [bump];
    let seed = [
        Seed::from(b"escrow"),
        Seed::from(maker.address().as_array()),
        Seed::from(&bump_bytes),
    ];
    let seeds = Signer::from(&seed);

    pinocchio_token::instructions::Transfer {
        from: vault,
        to: taker_ata_a,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
        amount: vault_amount,
    }
    .invoke_signed(&[seeds.clone()])?;

    // 8. CPI #3: close the vault (rent refunded to maker)
    pinocchio_token::instructions::CloseAccount {
        account: vault,
        destination: maker,
        authority: escrow_account,
        multisig_signers: &[] as &[&AccountView],
    }
    .invoke_signed(&[seeds.clone()])?;

    // 9. Close the escrow account by hand (program-owned, no CPI needed)
    //    Move lamports to maker, then zero and close.
    let escrow_lamports = escrow_account.lamports();
    maker.set_lamports(maker.lamports() + escrow_lamports);
    escrow_account.set_lamports(0);
    escrow_account.close()?;

    Ok(())
}
