#[cfg(test)]
mod tests {

    use std::path::PathBuf;

    use litesvm::LiteSVM;
    use litesvm_token::{
        spl_token::{self},
        CreateAssociatedTokenAccount,
        CreateMint,
        MintTo,
    };

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
    const ASSOCIATED_TOKEN_PROGRAM_ID: &str =
        "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL";

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

        svm.airdrop(&payer.pubkey(), 10 * LAMPORTS_PER_SOL)
            .expect("Airdrop failed");

        // Load program SO file (produced by `cargo build-sbf`)
        let so_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/deploy/escrow.so");

        let program_data = std::fs::read(&so_path).unwrap_or_else(|e| {
            panic!(
                "Failed to read program SO file at {}: {e}. Run `cargo build-sbf` first.",
                so_path.display()
            )
        });

        svm.add_program(program_id(), &program_data)
            .expect("Failed to add program");

        (svm, payer)
    }

    #[test]
    pub fn test_make_instruction() {
        let (mut svm, payer) = setup();

        let program_id = program_id();

        assert_eq!(program_id.to_string(), PROGRAM_ID);

        let mint_a = CreateMint::new(&mut svm, &payer)
            .decimals(6)
            .authority(&payer.pubkey())
            .send()
            .unwrap();
        println!("Mint A: {}", mint_a);

        let mint_b = CreateMint::new(&mut svm, &payer)
            .decimals(6)
            .authority(&payer.pubkey())
            .send()
            .unwrap();
        println!("Mint B: {}", mint_b);

        // Create the maker's associated token account for Mint A
        let maker_ata_a = CreateAssociatedTokenAccount::new(&mut svm, &payer, &mint_a)
            .owner(&payer.pubkey())
            .send()
            .unwrap();
        println!("Maker ATA A: {}\n", maker_ata_a);

        // Derive the PDA for the escrow account using the maker's public key and a seed value
        let escrow = Pubkey::find_program_address(
            &[b"escrow".as_ref(), payer.pubkey().as_ref()],
            &PROGRAM_ID.parse().unwrap(),
        );
        println!("Escrow PDA: {}\n", escrow.0);

        // Derive the PDA for the vault associated token account using the escrow PDA and Mint A
        let vault = spl_associated_token_account::get_associated_token_address(
            &escrow.0,
            &mint_a,
        );
        println!("Vault PDA: {}\n", vault);

        // Define program IDs for associated token program, token program, and system program
        let associated_token_program =
            ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();
        let token_program = TOKEN_PROGRAM_ID;
        let system_program = solana_sdk_ids::system_program::ID;

        // Mint 1,000 tokens (with 6 decimal places) of Mint A to the maker's associated token account
        MintTo::new(&mut svm, &payer, &mint_a, &maker_ata_a, 1000000000)
            .send()
            .unwrap();

        let amount_to_receive: u64 = 100000000;
        let amount_to_give: u64 = 500000000;
        let bump: u8 = escrow.1;

        println!("Bump: {}", bump);

        // Create the "Make" instruction to deposit tokens into the escrow
        let make_data = [
            vec![0u8],
            amount_to_receive.to_le_bytes().to_vec(),
            amount_to_give.to_le_bytes().to_vec(),
        ]
        .concat();

        let make_ix = Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new(payer.pubkey(), true),
                AccountMeta::new(mint_a, false),
                AccountMeta::new(mint_b, false),
                AccountMeta::new(escrow.0, false),
                AccountMeta::new(maker_ata_a, false),
                AccountMeta::new(vault, false),
                AccountMeta::new(system_program, false),
                AccountMeta::new(token_program, false),
                AccountMeta::new(associated_token_program, false),
            ],
            data: make_data,
        };

        // Create and send the transaction containing the "Make" instruction
        let message = Message::new(&[make_ix], Some(&payer.pubkey()));
        let recent_blockhash = svm.latest_blockhash();

        let transaction = Transaction::new(
            &[&payer],
            message,
            recent_blockhash,
        );

        let tx = svm.send_transaction(transaction).unwrap();

        println!("\n\nMake transaction successful");
        println!("CUs Consumed: {}", tx.compute_units_consumed);

        // --- extra verification ---
        let vault_acc = svm.get_account(&vault).unwrap();
        let vault_state =
            spl_token_2022::state::Account::unpack(&vault_acc.data).unwrap();

        println!(
            "Vault owner: {} (escrow PDA? {})",
            vault_state.owner,
            vault_state.owner == escrow.0
        );
        println!("Vault balance: {}", vault_state.amount);
        assert_eq!(vault_state.amount, amount_to_give);

        let maker_acc = svm.get_account(&maker_ata_a).unwrap();
        let maker_state =
            spl_token_2022::state::Account::unpack(&maker_acc.data).unwrap();

        println!("Maker ATA balance: {}", maker_state.amount);
        assert_eq!(maker_state.amount, 1000000000 - amount_to_give);

        let esc = svm.get_account(&escrow.0).unwrap();

        println!(
            "Escrow account owner: {} (program? {})",
            esc.owner,
            esc.owner == program_id
        );
        println!("Escrow data len: {}", esc.data.len());

        let d = &esc.data;

        println!(
            "  maker   = {}",
            Pubkey::new_from_array(d[0..32].try_into().unwrap())
        );
        println!(
            "  mint_a  = {}",
            Pubkey::new_from_array(d[32..64].try_into().unwrap())
        );
        println!(
            "  mint_b  = {}",
            Pubkey::new_from_array(d[64..96].try_into().unwrap())
        );
        println!(
            "  receive = {}",
            u64::from_le_bytes(d[96..104].try_into().unwrap())
        );
        println!(
            "  give    = {}",
            u64::from_le_bytes(d[104..112].try_into().unwrap())
        );
        println!("  bump    = {}", d[112]);

        assert_eq!(&d[0..32], payer.pubkey().as_ref());
        assert_eq!(
            u64::from_le_bytes(d[96..104].try_into().unwrap()),
            amount_to_receive
        );
        assert_eq!(
            u64::from_le_bytes(d[104..112].try_into().unwrap()),
            amount_to_give
        );
        assert_eq!(d[112], bump);
    }

    #[test]
    pub fn test_take_instruction() {
        let (mut svm, payer) = setup();

        let program_id = program_id();

        // Maker and taker
        let maker = Keypair::new();
        let taker = Keypair::new();

        svm.airdrop(&maker.pubkey(), 10 * LAMPORTS_PER_SOL)
            .unwrap();
        svm.airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL)
            .unwrap();

        // Create Mint A and Mint B
        let mint_a = CreateMint::new(&mut svm, &payer)
            .decimals(6)
            .authority(&payer.pubkey())
            .send()
            .unwrap();

        let mint_b = CreateMint::new(&mut svm, &payer)
            .decimals(6)
            .authority(&payer.pubkey())
            .send()
            .unwrap();

        // Create maker's ATA for Mint A
        let maker_ata_a =
            CreateAssociatedTokenAccount::new(&mut svm, &payer, &mint_a)
                .owner(&maker.pubkey())
                .send()
                .unwrap();

        // Derive escrow PDA
        let escrow = Pubkey::find_program_address(
            &[b"escrow", maker.pubkey().as_ref()],
            &program_id,
        );

        // Derive vault ATA
        let vault =
            spl_associated_token_account::get_associated_token_address(
                &escrow.0,
                &mint_a,
            );

        // Program IDs
        let associated_token_program =
            ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();
        let token_program = TOKEN_PROGRAM_ID;
        let system_program = solana_sdk_ids::system_program::ID;

        // Mint 1,000 A to maker
        MintTo::new(
            &mut svm,
            &payer,
            &mint_a,
            &maker_ata_a,
            1_000_000_000,
        )
        .send()
        .unwrap();

        let amount_to_receive: u64 = 100_000_000;
        let amount_to_give: u64 = 500_000_000;

        // Make
        let make_data = [
            vec![0u8],
            amount_to_receive.to_le_bytes().to_vec(),
            amount_to_give.to_le_bytes().to_vec(),
        ]
        .concat();

        let make_ix = Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new(maker.pubkey(), true),
                AccountMeta::new(mint_a, false),
                AccountMeta::new(mint_b, false),
                AccountMeta::new(escrow.0, false),
                AccountMeta::new(maker_ata_a, false),
                AccountMeta::new(vault, false),
                AccountMeta::new(system_program, false),
                AccountMeta::new(token_program, false),
                AccountMeta::new(associated_token_program, false),
            ],
            data: make_data,
        };

        let message = Message::new(&[make_ix], Some(&maker.pubkey()));

        let transaction = Transaction::new(
            &[&maker],
            message,
            svm.latest_blockhash(),
        );

        svm.send_transaction(transaction).unwrap();

        // Create taker's ATA for Mint A
        let taker_ata_a =
            CreateAssociatedTokenAccount::new(&mut svm, &payer, &mint_a)
                .owner(&taker.pubkey())
                .send()
                .unwrap();

        // Create taker's ATA for Mint B
        let taker_ata_b =
            CreateAssociatedTokenAccount::new(&mut svm, &payer, &mint_b)
                .owner(&taker.pubkey())
                .send()
                .unwrap();

        // Create maker's ATA for Mint B
        let maker_ata_b =
            CreateAssociatedTokenAccount::new(&mut svm, &payer, &mint_b)
                .owner(&maker.pubkey())
                .send()
                .unwrap();

        // Give the taker enough Mint B to complete the swap
        MintTo::new(
            &mut svm,
            &payer,
            &mint_b,
            &taker_ata_b,
            amount_to_receive,
        )
        .send()
        .unwrap();

        // Take
        let take_data = vec![1u8];

        let take_ix = Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new(taker.pubkey(), true),
                AccountMeta::new(maker.pubkey(), false),
                AccountMeta::new_readonly(mint_a, false),
                AccountMeta::new_readonly(mint_b, false),
                AccountMeta::new(escrow.0, false),
                AccountMeta::new(vault, false),
                AccountMeta::new(taker_ata_a, false),
                AccountMeta::new(taker_ata_b, false),
                AccountMeta::new(maker_ata_b, false),
                AccountMeta::new_readonly(system_program, false),
                AccountMeta::new_readonly(token_program, false),
                AccountMeta::new_readonly(associated_token_program, false),
            ],
            data: take_data,
        };

        let message = Message::new(&[take_ix], Some(&taker.pubkey()));

        let transaction = Transaction::new(
            &[&taker],
            message,
            svm.latest_blockhash(),
        );

        let tx = svm.send_transaction(transaction).unwrap();

        println!("Take transaction successful");
        println!("CUs Consumed: {}", tx.compute_units_consumed);

        // Verify taker received Mint A
        let taker_a_acc = svm.get_account(&taker_ata_a).unwrap();
        let taker_a_state =
            spl_token_2022::state::Account::unpack(&taker_a_acc.data).unwrap();

        assert_eq!(taker_a_state.amount, amount_to_give);

        // Verify maker received Mint B
        let maker_b_acc = svm.get_account(&maker_ata_b).unwrap();
        let maker_b_state =
            spl_token_2022::state::Account::unpack(&maker_b_acc.data).unwrap();

        assert_eq!(maker_b_state.amount, amount_to_receive);

        // Verify taker's Mint B was spent
        let taker_b_acc = svm.get_account(&taker_ata_b).unwrap();
        let taker_b_state =
            spl_token_2022::state::Account::unpack(&taker_b_acc.data).unwrap();

        assert_eq!(taker_b_state.amount, 0);

        // Vault should be closed
        assert!(svm.get_account(&vault).is_none());

        // Escrow should be closed
        assert!(svm.get_account(&escrow.0).is_none());
    }
    #[test]
    pub fn test_cancel_instruction() {
        let (mut svm, payer) = setup();

        let program_id = program_id();

        let maker = Keypair::new();

        svm.airdrop(&maker.pubkey(), 10 * LAMPORTS_PER_SOL)
            .unwrap();

        // Create Mint A and Mint B
        let mint_a = CreateMint::new(&mut svm, &payer)
            .decimals(6)
            .authority(&payer.pubkey())
            .send()
            .unwrap();

        let mint_b = CreateMint::new(&mut svm, &payer)
            .decimals(6)
            .authority(&payer.pubkey())
            .send()
            .unwrap();

        // Create maker's ATA for Mint A
        let maker_ata_a =
            CreateAssociatedTokenAccount::new(&mut svm, &payer, &mint_a)
                .owner(&maker.pubkey())
                .send()
                .unwrap();

        // Mint 1,000 A to maker
        MintTo::new(
            &mut svm,
            &payer,
            &mint_a,
            &maker_ata_a,
            1_000_000_000,
        )
        .send()
        .unwrap();

        // Derive escrow PDA
        let escrow = Pubkey::find_program_address(
            &[b"escrow", maker.pubkey().as_ref()],
            &program_id,
        );

        // Derive vault ATA
        let vault =
            spl_associated_token_account::get_associated_token_address(
                &escrow.0,
                &mint_a,
            );

        // Program IDs
        let associated_token_program =
            ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();
        let token_program = TOKEN_PROGRAM_ID;
        let system_program = solana_sdk_ids::system_program::ID;

        let amount_to_receive: u64 = 100_000_000;
        let amount_to_give: u64 = 500_000_000;

        // Make
        let make_data = [
            vec![0u8],
            amount_to_receive.to_le_bytes().to_vec(),
            amount_to_give.to_le_bytes().to_vec(),
        ]
        .concat();

        let make_ix = Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new(maker.pubkey(), true),
                AccountMeta::new(mint_a, false),
                AccountMeta::new(mint_b, false),
                AccountMeta::new(escrow.0, false),
                AccountMeta::new(maker_ata_a, false),
                AccountMeta::new(vault, false),
                AccountMeta::new(system_program, false),
                AccountMeta::new(token_program, false),
                AccountMeta::new(associated_token_program, false),
            ],
            data: make_data,
        };

        let message = Message::new(&[make_ix], Some(&maker.pubkey()));

        let transaction = Transaction::new(
            &[&maker],
            message,
            svm.latest_blockhash(),
        );

        svm.send_transaction(transaction).unwrap();

        // Cancel
        let cancel_ix = Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new(maker.pubkey(), true),
                AccountMeta::new_readonly(mint_a, false),
                AccountMeta::new(escrow.0, false),
                AccountMeta::new(vault, false),
                AccountMeta::new(maker_ata_a, false),
                AccountMeta::new_readonly(token_program, false),
            ],
            data: vec![2u8],
        };

        let message = Message::new(&[cancel_ix], Some(&maker.pubkey()));

        let transaction = Transaction::new(
            &[&maker],
            message,
            svm.latest_blockhash(),
        );

        let tx = svm.send_transaction(transaction).unwrap();

        println!("Cancel transaction successful");
        println!("CUs Consumed: {}", tx.compute_units_consumed);

        // Verify maker received all 1,000 A back
        let maker_a_acc = svm.get_account(&maker_ata_a).unwrap();
        let maker_a_state =
            spl_token_2022::state::Account::unpack(&maker_a_acc.data).unwrap();

        assert_eq!(maker_a_state.amount, 1_000_000_000);

        // Vault should be closed
        assert!(svm.get_account(&vault).is_none());

        // Escrow should be closed
        assert!(svm.get_account(&escrow.0).is_none());
    }
    #[test]
    pub fn test_take_insufficient_balance() {
        let (mut svm, payer) = setup();

        let program_id = program_id();

        let maker = Keypair::new();
        let taker = Keypair::new();

        svm.airdrop(&maker.pubkey(), 10 * LAMPORTS_PER_SOL)
            .unwrap();

        svm.airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL)
            .unwrap();

        let mint_a = CreateMint::new(&mut svm, &payer)
            .decimals(6)
            .authority(&payer.pubkey())
            .send()
            .unwrap();

        let mint_b = CreateMint::new(&mut svm, &payer)
            .decimals(6)
            .authority(&payer.pubkey())
            .send()
            .unwrap();

        let maker_ata_a =
            CreateAssociatedTokenAccount::new(&mut svm, &payer, &mint_a)
                .owner(&maker.pubkey())
                .send()
                .unwrap();

        MintTo::new(
            &mut svm,
            &payer,
            &mint_a,
            &maker_ata_a,
            1_000_000_000,
        )
        .send()
        .unwrap();

        let escrow = Pubkey::find_program_address(
            &[b"escrow", maker.pubkey().as_ref()],
            &program_id,
        );

        let vault =
            spl_associated_token_account::get_associated_token_address(
                &escrow.0,
                &mint_a,
            );

        let associated_token_program =
            ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();
        let token_program = TOKEN_PROGRAM_ID;
        let system_program = solana_sdk_ids::system_program::ID;

        let amount_to_receive: u64 = 100_000_000;
        let amount_to_give: u64 = 500_000_000;

        // Make
        let make_data = [
            vec![0u8],
            amount_to_receive.to_le_bytes().to_vec(),
            amount_to_give.to_le_bytes().to_vec(),
        ]
        .concat();

        let make_ix = Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new(maker.pubkey(), true),
                AccountMeta::new(mint_a, false),
                AccountMeta::new(mint_b, false),
                AccountMeta::new(escrow.0, false),
                AccountMeta::new(maker_ata_a, false),
                AccountMeta::new(vault, false),
                AccountMeta::new(system_program, false),
                AccountMeta::new(token_program, false),
                AccountMeta::new(associated_token_program, false),
            ],
            data: make_data,
        };

        let message = Message::new(&[make_ix], Some(&maker.pubkey()));

        let transaction = Transaction::new(
            &[&maker],
            message,
            svm.latest_blockhash(),
        );

        svm.send_transaction(transaction).unwrap();

        // Create taker's Mint A and Mint B ATAs
        let taker_ata_a =
            CreateAssociatedTokenAccount::new(&mut svm, &payer, &mint_a)
                .owner(&taker.pubkey())
                .send()
                .unwrap();

        let taker_ata_b =
            CreateAssociatedTokenAccount::new(&mut svm, &payer, &mint_b)
                .owner(&taker.pubkey())
                .send()
                .unwrap();

        let maker_ata_b =
            CreateAssociatedTokenAccount::new(&mut svm, &payer, &mint_b)
                .owner(&maker.pubkey())
                .send()
                .unwrap();

        // Give taker only 50 B.
        MintTo::new(
            &mut svm,
            &payer,
            &mint_b,
            &taker_ata_b,
            50_000_000,
        )
        .send()
        .unwrap();

        let take_ix = Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new(taker.pubkey(), true),
                AccountMeta::new(maker.pubkey(), false),
                AccountMeta::new_readonly(mint_a, false),
                AccountMeta::new_readonly(mint_b, false),
                AccountMeta::new(escrow.0, false),
                AccountMeta::new(vault, false),
                AccountMeta::new(taker_ata_a, false),
                AccountMeta::new(taker_ata_b, false),
                AccountMeta::new(maker_ata_b, false),
                AccountMeta::new_readonly(system_program, false),
                AccountMeta::new_readonly(token_program, false),
                AccountMeta::new_readonly(associated_token_program, false),
            ],
            data: vec![1u8],
        };

        let message = Message::new(&[take_ix], Some(&taker.pubkey()));

        let transaction = Transaction::new(
            &[&taker],
            message,
            svm.latest_blockhash(),
        );

        let result = svm.send_transaction(transaction);

        assert!(result.is_err());

        // Escrow should still exist because the transaction failed atomically.
        assert!(svm.get_account(&escrow.0).is_some());

        // Vault should still exist.
        assert!(svm.get_account(&vault).is_some());

        println!("Insufficient-balance Take correctly failed");
    }
}