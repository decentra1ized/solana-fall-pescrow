#[cfg(test)]
mod tests {

    use std::path::PathBuf;

    use litesvm::LiteSVM;
    use litesvm_token::{spl_token::{self}, CreateAssociatedTokenAccount, CreateMint, MintTo};
    
    use solana_instruction::{AccountMeta, Instruction};
    use solana_keypair::Keypair;
    use solana_message::Message;
    use solana_native_token::LAMPORTS_PER_SOL;
    use solana_pubkey::Pubkey;
    use solana_signer::Signer;
    use solana_transaction::Transaction;
    use solana_program_pack::Pack;

    const PROGRAM_ID: &str = "4ibrEMW5F6hKnkW4jVedswYv6H6VtwPN6ar6dvXDN1nT";
    const TOKEN_PROGRAM_ID: Pubkey = spl_token::ID;
    const ASSOCIATED_TOKEN_PROGRAM_ID: &str = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL";
    
fn program_id() -> Pubkey {
    Pubkey::from(crate::ID)
}

fn setup() -> (LiteSVM, Keypair) {

    let mut svm = LiteSVM::new();
    let payer = Keypair::new();

    

    // LiteSVM 0.9 still ships the pre-SIMD-0194 Rent sysvar (3480 lamports/byte-year,
    // 2-year exemption threshold). Mainnet has activated SIMD-0194, which folds the
    // threshold into the rate (6960 lamports/byte, threshold 1.0), and pinocchio 0.11
    // computes rent exemption that way. Set the sysvar to match the live cluster.
    #[allow(deprecated)]
    svm.set_sysvar(&solana_rent::Rent {
        lamports_per_byte_year: 6960,
        exemption_threshold: 1.0,
        burn_percent: 50,
    });

    svm
        .airdrop(&payer.pubkey(), 10 * LAMPORTS_PER_SOL)
        .expect("Airdrop failed");

    // Load program SO file (produced by `cargo build-sbf`)
    let so_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/deploy/escrow.so");

    let program_data = std::fs::read(&so_path)
        .unwrap_or_else(|e| panic!("Failed to read program SO file at {}: {e}. Run `cargo build-sbf` first.", so_path.display()));

    svm.add_program(program_id(), &program_data).expect("Failed to add program");

    (svm, payer)
    
}

 struct MakeSetup {
        svm: LiteSVM,
        maker: Keypair,
        mint_a: Pubkey,
        mint_b: Pubkey,
        maker_ata_a: Pubkey,
        escrow: Pubkey,
        bump: u8,
        vault: Pubkey,
        amount_to_receive: u64,
        amount_to_give: u64,
    }

fn account_closed(svm: &LiteSVM, pubkey: &Pubkey) -> bool {
        match svm.get_account(pubkey) {
            None => true,
            Some(acc) => acc.lamports == 0 || acc.owner == solana_sdk_ids::system_program::ID,
        }
    }

fn make_helper() -> MakeSetup {
        let (mut svm, maker) = setup();
        let program_id = program_id();

        let mint_a = CreateMint::new(&mut svm, &maker)
            .decimals(6)
            .authority(&maker.pubkey())
            .send()
            .unwrap();

        let mint_b = CreateMint::new(&mut svm, &maker)
            .decimals(6)
            .authority(&maker.pubkey())
            .send()
            .unwrap();
    
       // Create the maker's associated token account for Mint A
        let maker_ata_a = CreateAssociatedTokenAccount::new(&mut svm, &maker, &mint_a)
            .owner(&maker.pubkey()).send().unwrap();

        // Derive the PDA for the escrow account using the maker's public key and a seed value
        let (escrow, bump) = Pubkey::find_program_address(
            &[b"escrow".as_ref(), maker.pubkey().as_ref()],
            &program_id,
        );

        // Derive the PDA for the vault associated token account using the escrow PDA and Mint A
        let vault = spl_associated_token_account::get_associated_token_address(
            &escrow,    // owner will be the escrow PDA
            &mint_a     // mint
        );

           // Define program IDs for associated token program, token program, and system program
        let associated_token_program = ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();
        let token_program = TOKEN_PROGRAM_ID;
        let system_program = solana_sdk_ids::system_program::ID;

        // Mint 1,000 tokens (with 6 decimal places) of Mint A to the maker's associated token account
        MintTo::new(&mut svm, &maker, &mint_a, &maker_ata_a, 1_000_000_000)
            .send()
            .unwrap();

        let amount_to_receive: u64 = 100_000_000; // 100 tokens with 6 decimal places
        let amount_to_give: u64 = 500_000_000;    // 500 tokens with 6 decimal places

        let make_data = [
            vec![0u8],              // Discriminator for "Make" instruction
            amount_to_receive.to_le_bytes().to_vec(),
            amount_to_give.to_le_bytes().to_vec(),
        ].concat();
        let make_ix = Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new(maker.pubkey(), true),
                AccountMeta::new(mint_a, false),
                AccountMeta::new(mint_b, false),
                AccountMeta::new(escrow, false),
                AccountMeta::new(maker_ata_a, false),
                AccountMeta::new(vault, false),
                AccountMeta::new(system_program, false),
                AccountMeta::new(token_program, false),
                AccountMeta::new(associated_token_program, false),
            ],
            data: make_data,
        };

           // Create and send the transaction containing the "Make" instruction
        let message = Message::new(&[make_ix], Some(&maker.pubkey()));
        let recent_blockhash = svm.latest_blockhash();
        let transaction = Transaction::new(&[&maker], message, recent_blockhash);

        // Send the transaction and capture the result
        svm.send_transaction(transaction).unwrap();

        MakeSetup {
            svm, maker, mint_a, mint_b, maker_ata_a,
            escrow, bump, vault, amount_to_receive, amount_to_give,
        }
    }




#[test]
pub fn test_make_instruction() {
    let MakeSetup { mut svm, maker,mint_a, mint_b, maker_ata_a, escrow, bump, vault, amount_to_receive, amount_to_give} = make_helper();

    let program_id = program_id();

    assert_eq!(program_id.to_string(), PROGRAM_ID);


    let token_program = TOKEN_PROGRAM_ID;
    let system_program = solana_sdk_ids::system_program::ID;
    let associated_token_program = ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();

    let taker = Keypair::new();
    svm.airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL).expect("Airdrop failed");

    let taker_ata_b = CreateAssociatedTokenAccount::new(&mut svm, &taker, &mint_b)
        .owner(&taker.pubkey())
        .send()
        .unwrap();

    // Only 50 tokens minted — the escrow requires 100 to Take
    MintTo::new(&mut svm, &maker, &mint_b, &taker_ata_b, 50_000_000)
        .send()
        .unwrap();

    let taker_ata_a = spl_associated_token_account::get_associated_token_address(&taker.pubkey(), &mint_a);
    let maker_ata_b = spl_associated_token_account::get_associated_token_address(&maker.pubkey(), &mint_b);

    let take_ix = Instruction {
        program_id,
        accounts: vec![
            AccountMeta::new(taker.pubkey(), true),
            AccountMeta::new(maker.pubkey(), false),
            AccountMeta::new(mint_a, false),
            AccountMeta::new(mint_b, false),
            AccountMeta::new(escrow, false),
            AccountMeta::new(vault, false),
            AccountMeta::new(taker_ata_a, false),
            AccountMeta::new(taker_ata_b, false),
            AccountMeta::new(maker_ata_b, false),
            AccountMeta::new(system_program, false),
            AccountMeta::new(token_program, false),
            AccountMeta::new(associated_token_program, false),
        ],
        data: vec![1u8],
    };

    let message = Message::new(&[take_ix], Some(&taker.pubkey()));
    let recent_blockhash = svm.latest_blockhash();
    let transaction = Transaction::new(&[&taker], message, recent_blockhash);

    // The transfer of 100 B should be rejected by the token program since
    // the taker only holds 50 B — the whole transaction should fail
    let result = svm.send_transaction(transaction);
    assert!(result.is_err());

    // --- extra verification: nothing should have moved ---
    let vault_acc = svm.get_account(&vault).unwrap();
    let vault_state = spl_token_2022::state::Account::unpack(&vault_acc.data).unwrap();
    assert_eq!(vault_state.amount, 500_000_000);
}

#[test]
pub fn test_cancel_fails_for_stranger() {
    let MakeSetup {
        mut svm, mint_a, maker_ata_a, escrow, vault, ..
    } = make_helper();

    let program_id = program_id();
    let token_program = TOKEN_PROGRAM_ID;

    let stranger = Keypair::new();
    svm.airdrop(&stranger.pubkey(), 10 * LAMPORTS_PER_SOL).expect("Airdrop failed");

    // Stranger signs and sits in the maker slot — the escrow's stored maker
    // is still the real maker, so this must be rejected on-chain
    let cancel_ix = Instruction {
        program_id,
        accounts: vec![
            AccountMeta::new(stranger.pubkey(), true),
            AccountMeta::new(mint_a, false),
            AccountMeta::new(escrow, false),
            AccountMeta::new(vault, false),
            AccountMeta::new(maker_ata_a, false),
            AccountMeta::new(token_program, false),
        ],
        data: vec![2u8],
    };

    let message = Message::new(&[cancel_ix], Some(&stranger.pubkey()));
    let recent_blockhash = svm.latest_blockhash();
    let transaction = Transaction::new(&[&stranger], message, recent_blockhash);

    // Cancel by a non-maker must fail
    let result = svm.send_transaction(transaction);
    assert!(result.is_err());

    // --- extra verification: the 500 A should still be in the vault ---
    let vault_acc = svm.get_account(&vault).unwrap();
    let vault_state = spl_token_2022::state::Account::unpack(&vault_acc.data).unwrap();
    assert_eq!(vault_state.amount, 500_000_000);
}
}

