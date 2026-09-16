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

        #[allow(deprecated)]
        svm.set_sysvar(&solana_rent::Rent {
            lamports_per_byte_year: 6960,
            exemption_threshold: 1.0,
            burn_percent: 50,
        });

        svm
            .airdrop(&payer.pubkey(), 10 * LAMPORTS_PER_SOL)
            .expect("Airdrop failed");

        let so_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/deploy/escrow.so");

        let program_data = std::fs::read(&so_path)
            .unwrap_or_else(|e| panic!("Failed to read program SO file at {}: {e}. Run `cargo build-sbf` first.", so_path.display()));

        svm.add_program(program_id(), &program_data).expect("Failed to add program");

        (svm, payer)

    }

    /// Helper: sets up two mints, funds the maker, runs Make, and returns everything
    /// the next steps need.
    fn make_setup() -> (LiteSVM, Keypair, Pubkey, Pubkey, Pubkey, u8, Pubkey) {
        let (mut svm, payer) = setup();

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

        let maker_ata_a = CreateAssociatedTokenAccount::new(&mut svm, &payer, &mint_a)
            .owner(&payer.pubkey()).send().unwrap();
        println!("Maker ATA A: {}", maker_ata_a);

        let escrow = Pubkey::find_program_address(
            &[b"escrow".as_ref(), payer.pubkey().as_ref()],
            &PROGRAM_ID.parse().unwrap(),
        );
        println!("Escrow PDA: {}", escrow.0);

        let vault = spl_associated_token_account::get_associated_token_address(
            &escrow.0,
            &mint_a,
        );
        println!("Vault: {}", vault);

        let associated_token_program = ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();
        let token_program = TOKEN_PROGRAM_ID;
        let system_program = solana_sdk_ids::system_program::ID;

        MintTo::new(&mut svm, &payer, &mint_a, &maker_ata_a, 1000000000)
            .send()
            .unwrap();

        let amount_to_receive: u64 = 100000000; // 100 tokens
        let amount_to_give: u64 = 500000000;    // 500 tokens

        let make_data = [
            vec![0u8],
            amount_to_receive.to_le_bytes().to_vec(),
            amount_to_give.to_le_bytes().to_vec(),
        ].concat();
        let make_ix = Instruction {
            program_id: program_id(),
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

        let message = Message::new(&[make_ix], Some(&payer.pubkey()));
        let recent_blockhash = svm.latest_blockhash();
        let transaction = Transaction::new(&[&payer], message, recent_blockhash);

        let tx = svm.send_transaction(transaction).unwrap();
        println!("Make CUs: {}", tx.compute_units_consumed);

        (svm, payer, mint_a, mint_b, escrow.0, escrow.1, vault)
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

        let maker_ata_a = CreateAssociatedTokenAccount::new(&mut svm, &payer, &mint_a)
            .owner(&payer.pubkey()).send().unwrap();
        println!("Maker ATA A: {}\n", maker_ata_a);

        let escrow = Pubkey::find_program_address(
            &[b"escrow".as_ref(), payer.pubkey().as_ref()],
            &PROGRAM_ID.parse().unwrap(),
        );
        println!("Escrow PDA: {}\n", escrow.0);

        let vault = spl_associated_token_account::get_associated_token_address(
            &escrow.0,
            &mint_a
        );
        println!("Vault PDA: {}\n", vault);

        let associated_token_program = ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();
        let token_program = TOKEN_PROGRAM_ID;
        let system_program = solana_sdk_ids::system_program::ID;

        MintTo::new(&mut svm, &payer, &mint_a, &maker_ata_a, 1000000000)
            .send()
            .unwrap();

        let amount_to_receive: u64 = 100000000;
        let amount_to_give: u64 = 500000000;
        let bump: u8 = escrow.1;

        println!("Bump: {}", bump);

        let make_data = [
            vec![0u8],
            amount_to_receive.to_le_bytes().to_vec(),
            amount_to_give.to_le_bytes().to_vec(),
        ].concat();
        let make_ix = Instruction {
            program_id: program_id,
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

        let message = Message::new(&[make_ix], Some(&payer.pubkey()));
        let recent_blockhash = svm.latest_blockhash();
        let transaction = Transaction::new(&[&payer], message, recent_blockhash);

        let tx = svm.send_transaction(transaction).unwrap();

        println!("\n\nMake transaction successful");
        println!("CUs Consumed: {}", tx.compute_units_consumed);

        let vault_acc = svm.get_account(&vault).unwrap();
        let vault_state = spl_token_2022::state::Account::unpack(&vault_acc.data).unwrap();
        println!("Vault owner: {} (escrow PDA? {})", vault_state.owner, vault_state.owner == escrow.0);
        println!("Vault balance: {}", vault_state.amount);
        assert_eq!(vault_state.amount, amount_to_give);

        let maker_acc = svm.get_account(&maker_ata_a).unwrap();
        let maker_state = spl_token_2022::state::Account::unpack(&maker_acc.data).unwrap();
        println!("Maker ATA balance: {}", maker_state.amount);
        assert_eq!(maker_state.amount, 1000000000 - amount_to_give);

        let esc = svm.get_account(&escrow.0).unwrap();
        println!("Escrow account owner: {} (program? {})", esc.owner, esc.owner == program_id);
        println!("Escrow data len: {}", esc.data.len());
        let d = &esc.data;
        println!("  maker   = {}", Pubkey::new_from_array(d[0..32].try_into().unwrap()));
        println!("  mint_a  = {}", Pubkey::new_from_array(d[32..64].try_into().unwrap()));
        println!("  mint_b  = {}", Pubkey::new_from_array(d[64..96].try_into().unwrap()));
        println!("  receive = {}", u64::from_le_bytes(d[96..104].try_into().unwrap()));
        println!("  give    = {}", u64::from_le_bytes(d[104..112].try_into().unwrap()));
        println!("  bump    = {}", d[112]);
        assert_eq!(&d[0..32], payer.pubkey().as_ref());
        assert_eq!(u64::from_le_bytes(d[96..104].try_into().unwrap()), amount_to_receive);
        assert_eq!(u64::from_le_bytes(d[104..112].try_into().unwrap()), amount_to_give);
        assert_eq!(d[112], bump);
    }

    #[test]
    pub fn test_take_instruction() {
        let (mut svm, maker, mint_a, mint_b, escrow_pda, _bump, vault) = make_setup();

        // Create taker keypair and airdrop SOL
        let taker = Keypair::new();
        svm.airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

        // Create taker's ATA for mint B and fund it with 100 B
        // Note: MintTo requires the mint authority (maker) to sign
        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut svm, &taker, &mint_b)
            .owner(&taker.pubkey()).send().unwrap();
        MintTo::new(&mut svm, &maker, &mint_b, &taker_ata_b, 100000000)
            .send()
            .unwrap();

        // Derive the ATAs (will be created by the program via CreateIdempotent)
        let taker_ata_a = spl_associated_token_account::get_associated_token_address(
            &taker.pubkey(),
            &mint_a,
        );
        let maker_ata_b = spl_associated_token_account::get_associated_token_address(
            &maker.pubkey(),
            &mint_b,
        );

        let associated_token_program = ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();
        let token_program = TOKEN_PROGRAM_ID;
        let system_program = solana_sdk_ids::system_program::ID;

        // Build Take instruction: data = [1u8]
        let take_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(taker.pubkey(), true),          // 0 taker
                AccountMeta::new(maker.pubkey(), false),         // 1 maker
                AccountMeta::new(mint_a, false),                 // 2 mint_a
                AccountMeta::new(mint_b, false),                 // 3 mint_b
                AccountMeta::new(escrow_pda, false),             // 4 escrow_account
                AccountMeta::new(vault, false),                  // 5 vault
                AccountMeta::new(taker_ata_a, false),            // 6 taker_ata_a
                AccountMeta::new(taker_ata_b, false),            // 7 taker_ata_b
                AccountMeta::new(maker_ata_b, false),            // 8 maker_ata_b
                AccountMeta::new(system_program, false),         // 9 system_program
                AccountMeta::new(token_program, false),          // 10 token_program
                AccountMeta::new(associated_token_program, false), // 11 associated_token_program
            ],
            data: vec![1u8],  // Take discriminator
        };

        let message = Message::new(&[take_ix], Some(&taker.pubkey()));
        let recent_blockhash = svm.latest_blockhash();
        let transaction = Transaction::new(&[&taker], message, recent_blockhash);

        let tx = svm.send_transaction(transaction).unwrap();
        println!("\n\nTake transaction successful");
        println!("CUs Consumed: {}", tx.compute_units_consumed);

        // Assert: taker_ata_a has 500 A
        let taker_a_acc = svm.get_account(&taker_ata_a).unwrap();
        let taker_a_state = spl_token_2022::state::Account::unpack(&taker_a_acc.data).unwrap();
        println!("Taker ATA A balance: {}", taker_a_state.amount);
        assert_eq!(taker_a_state.amount, 500000000);

        // Assert: maker_ata_b has 100 B
        let maker_b_acc = svm.get_account(&maker_ata_b).unwrap();
        let maker_b_state = spl_token_2022::state::Account::unpack(&maker_b_acc.data).unwrap();
        println!("Maker ATA B balance: {}", maker_b_state.amount);
        assert_eq!(maker_b_state.amount, 100000000);

        // Assert: vault is closed (None or zero lamports)
        let vault_acc = svm.get_account(&vault);
        println!("Vault account exists: {}", vault_acc.is_some());
        assert!(vault_acc.is_none() || vault_acc.unwrap().lamports == 0);

        // Assert: escrow is closed
        let esc_acc = svm.get_account(&escrow_pda);
        println!("Escrow account exists: {}", esc_acc.is_some());
        assert!(esc_acc.is_none() || esc_acc.unwrap().lamports == 0);
    }

    #[test]
    pub fn test_cancel_instruction() {
        let (mut svm, maker, mint_a, _mint_b, escrow_pda, _bump, vault) = make_setup();

        // Maker's ATA for A should have 500 A after Make (1000 - 500)
        let maker_ata_a = spl_associated_token_account::get_associated_token_address(
            &maker.pubkey(),
            &mint_a,
        );

        let token_program = TOKEN_PROGRAM_ID;

        // Build Cancel instruction: data = [2u8]
        let cancel_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(maker.pubkey(), true),          // 0 maker
                AccountMeta::new(mint_a, false),                 // 1 mint_a
                AccountMeta::new(escrow_pda, false),             // 2 escrow_account
                AccountMeta::new(vault, false),                  // 3 vault
                AccountMeta::new(maker_ata_a, false),            // 4 maker_ata_a
                AccountMeta::new(token_program, false),          // 5 token_program
            ],
            data: vec![2u8],  // Cancel discriminator
        };

        let message = Message::new(&[cancel_ix], Some(&maker.pubkey()));
        let recent_blockhash = svm.latest_blockhash();
        let transaction = Transaction::new(&[&maker], message, recent_blockhash);

        let tx = svm.send_transaction(transaction).unwrap();
        println!("\n\nCancel transaction successful");
        println!("CUs Consumed: {}", tx.compute_units_consumed);

        // Assert: maker_ata_a is back to 1000 A
        let maker_a_acc = svm.get_account(&maker_ata_a).unwrap();
        let maker_a_state = spl_token_2022::state::Account::unpack(&maker_a_acc.data).unwrap();
        println!("Maker ATA A balance: {}", maker_a_state.amount);
        assert_eq!(maker_a_state.amount, 1000000000);

        // Assert: vault is closed
        let vault_acc = svm.get_account(&vault);
        println!("Vault account exists: {}", vault_acc.is_some());
        assert!(vault_acc.is_none() || vault_acc.unwrap().lamports == 0);

        // Assert: escrow is closed
        let esc_acc = svm.get_account(&escrow_pda);
        println!("Escrow account exists: {}", esc_acc.is_some());
        assert!(esc_acc.is_none() || esc_acc.unwrap().lamports == 0);
    }

    #[test]
    pub fn test_take_insufficient_funds() {
        let (mut svm, maker, mint_a, mint_b, escrow_pda, _bump, vault) = make_setup();

        // Create taker with only 50 B (needs 100)
        let taker = Keypair::new();
        svm.airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut svm, &taker, &mint_b)
            .owner(&taker.pubkey()).send().unwrap();
        MintTo::new(&mut svm, &maker, &mint_b, &taker_ata_b, 50000000)  // only 50 B
            .send()
            .unwrap();

        let taker_ata_a = spl_associated_token_account::get_associated_token_address(
            &taker.pubkey(),
            &mint_a,
        );
        let maker_ata_b = spl_associated_token_account::get_associated_token_address(
            &maker.pubkey(),
            &mint_b,
        );

        let associated_token_program = ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();
        let token_program = TOKEN_PROGRAM_ID;
        let system_program = solana_sdk_ids::system_program::ID;

        let take_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(taker.pubkey(), true),
                AccountMeta::new(maker.pubkey(), false),
                AccountMeta::new(mint_a, false),
                AccountMeta::new(mint_b, false),
                AccountMeta::new(escrow_pda, false),
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

        // Should fail: taker doesn't have enough B
        let result = svm.send_transaction(transaction);
        println!("\n\nTake with insufficient funds result: {:?}", result.is_err());
        assert!(result.is_err(), "Take should fail when taker has insufficient B");

        // Verify escrow and vault are still intact
        let esc_acc = svm.get_account(&escrow_pda);
        assert!(esc_acc.is_some(), "Escrow should still exist after failed Take");
        let vault_acc = svm.get_account(&vault);
        assert!(vault_acc.is_some(), "Vault should still exist after failed Take");
    }

    #[test]
    pub fn test_cancel_by_stranger() {
        let (mut svm, _maker, mint_a, _mint_b, escrow_pda, _bump, vault) = make_setup();

        // A stranger tries to cancel
        let stranger = Keypair::new();
        svm.airdrop(&stranger.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

        let stranger_ata_a = CreateAssociatedTokenAccount::new(&mut svm, &stranger, &mint_a)
            .owner(&stranger.pubkey()).send().unwrap();

        let token_program = TOKEN_PROGRAM_ID;

        let cancel_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(stranger.pubkey(), true),       // 0 signer = stranger
                AccountMeta::new(mint_a, false),                 // 1 mint_a
                AccountMeta::new(escrow_pda, false),             // 2 escrow_account
                AccountMeta::new(vault, false),                  // 3 vault
                AccountMeta::new(stranger_ata_a, false),         // 4 stranger_ata_a
                AccountMeta::new(token_program, false),          // 5 token_program
            ],
            data: vec![2u8],  // Cancel discriminator
        };

        let message = Message::new(&[cancel_ix], Some(&stranger.pubkey()));
        let recent_blockhash = svm.latest_blockhash();
        let transaction = Transaction::new(&[&stranger], message, recent_blockhash);

        // Should fail: stranger is not the maker
        let result = svm.send_transaction(transaction);
        println!("\n\nCancel by stranger result: {:?}", result.is_err());
        assert!(result.is_err(), "Cancel should fail when called by a stranger");

        // Verify vault still has 500 A
        let vault_acc = svm.get_account(&vault).unwrap();
        let vault_state = spl_token_2022::state::Account::unpack(&vault_acc.data).unwrap();
        println!("Vault balance after failed cancel: {}", vault_state.amount);
        assert_eq!(vault_state.amount, 500000000, "Vault should still hold 500 A");
    }
}
