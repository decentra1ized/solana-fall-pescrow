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
            .owner(&payer.pubkey()).send().unwrap();
        println!("Maker ATA A: {}\n", maker_ata_a);

        // Derive the PDA for the escrow account using the maker's public key and a seed value
        let escrow = Pubkey::find_program_address(
            &[b"escrow".as_ref(), payer.pubkey().as_ref()],
            &PROGRAM_ID.parse().unwrap(),
        );
        println!("Escrow PDA: {}\n", escrow.0);

        // Derive the PDA for the vault associated token account using the escrow PDA and Mint A
        let vault = spl_associated_token_account::get_associated_token_address(
            &escrow.0,  // owner will be the escrow PDA
            &mint_a     // mint
        );
        println!("Vault PDA: {}\n", vault);

        // Define program IDs for associated token program, token program, and system program
        let associated_token_program = ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();
        let token_program = TOKEN_PROGRAM_ID;
        let system_program = solana_sdk_ids::system_program::ID;

        // Mint 1,000 tokens (with 6 decimal places) of Mint A to the maker's associated token account
        MintTo::new(&mut svm, &payer, &mint_a, &maker_ata_a, 1000000000)
            .send()
            .unwrap();

        let amount_to_receive: u64 = 100000000; // 100 tokens with 6 decimal places
        let amount_to_give: u64 = 500000000;    // 500 tokens with 6 decimal places
        let bump: u8 = escrow.1;   // canonical bump; the program derives the same one on-chain

        println!("Bump: {}", bump);

        // Create the "Make" instruction to deposit tokens into the escrow
        let make_data = [
            vec![0u8],              // Discriminator for "Make" instruction
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

        // Create and send the transaction containing the "Make" instruction
        let message = Message::new(&[make_ix], Some(&payer.pubkey()));
        let recent_blockhash = svm.latest_blockhash();

        let transaction = Transaction::new(&[&payer], message, recent_blockhash);

        // Send the transaction and capture the result
        let tx = svm.send_transaction(transaction).unwrap();

        // Log transaction details
        println!("\n\nMake transaction successful");
        println!("CUs Consumed: {}", tx.compute_units_consumed);

        // --- extra verification ---
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
    pub fn test_make_then_take() {
        let (mut svm, maker) = setup();

        let program_id = program_id();

        assert_eq!(program_id.to_string(), PROGRAM_ID);
        // PRECONDITION. THIS IS THE MAKE INSTRUCTION.
        let mint_a = CreateMint::new(&mut svm, &maker)
            .decimals(6)
            .authority(&maker.pubkey())
            .send()
            .unwrap();
        println!("Mint A: {}", mint_a);

        let mint_b = CreateMint::new(&mut svm, &maker)
            .decimals(6)
            .authority(&maker.pubkey())
            .send()
            .unwrap();
        println!("Mint B: {}", mint_b);

        // Create the maker's associated token account for Mint A
        let maker_ata_a = CreateAssociatedTokenAccount::new(&mut svm, &maker, &mint_a)
            .owner(&maker.pubkey()).send().unwrap();
        println!("Maker ATA A: {}\n", maker_ata_a);

        // Derive the PDA for the escrow account using the maker's public key and a seed value
        let escrow = Pubkey::find_program_address(
            &[b"escrow".as_ref(), maker.pubkey().as_ref()],
            &PROGRAM_ID.parse().unwrap(),
        );
        println!("Escrow PDA: {}\n", escrow.0);

        // Derive the PDA for the vault associated token account using the escrow PDA and Mint A
        let vault = spl_associated_token_account::get_associated_token_address(
            &escrow.0,  // owner will be the escrow PDA
            &mint_a     // mint
        );
        println!("Vault PDA: {}\n", vault);

        // Define program IDs for associated token program, token program, and system program
        let associated_token_program = ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();
        let token_program = TOKEN_PROGRAM_ID;
        let system_program = solana_sdk_ids::system_program::ID;

        // Mint 1,000 tokens (with 6 decimal places) of Mint A to the maker's associated token account
        MintTo::new(&mut svm, &maker, &mint_a, &maker_ata_a, 1000000000)
            .send()
            .unwrap();

        let amount_to_receive: u64 = 100000000; // 100 tokens with 6 decimal places
        let amount_to_give: u64 = 500000000;    // 500 tokens with 6 decimal places
        let bump: u8 = escrow.1;   // canonical bump; the program derives the same one on-chain

        println!("Bump: {}", bump);

        // Create the "Make" instruction to deposit tokens into the escrow
        let make_data = [
            vec![0u8],              // Discriminator for "Make" instruction
            amount_to_receive.to_le_bytes().to_vec(),
            amount_to_give.to_le_bytes().to_vec(),
        ].concat();
        let make_ix = Instruction {
            program_id: program_id,
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

        // Create and send the transaction containing the "Make" instruction
        let message = Message::new(&[make_ix], Some(&maker.pubkey()));
        let recent_blockhash = svm.latest_blockhash();

        let transaction = Transaction::new(&[&maker], message, recent_blockhash);

        // Send the transaction and capture the result
        let tx = svm.send_transaction(transaction).unwrap();
        println!("Escrow Taker instruction");

        // ============== TAKE INSTRUCTION ===================
        // GETTING THE ESCROW ACCOUNT
        let taker = Keypair::new();
        svm.airdrop(&taker.pubkey(), 1_000_000_000);
        let initial_taker_b_balance = 600000000;
        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut svm, &taker, &mint_b)
            .owner(&taker.pubkey()).send().unwrap();
        // Preload taker_ata_b with tokens to send back to maker
        MintTo::new(&mut svm, &maker, &mint_b, &taker_ata_b, initial_taker_b_balance).send().unwrap();
        let taker_ata_a = CreateAssociatedTokenAccount::new(&mut svm, &taker, &mint_a)
            .owner(&taker.pubkey()).send().unwrap();
        let maker_ata_b = CreateAssociatedTokenAccount::new(&mut svm, &maker, &mint_b)
            .owner(&maker.pubkey()).send().unwrap();
        // TAKE INSTRUCTION DATA(IS THE INVERSE OF MAKE INSTRUCTION DATA)
        let take_data = [
            vec![1u8], // Discriminator for the take instruction.
        ].concat();
        let take_ix = Instruction {
            program_id: program_id,
            accounts: vec![
                AccountMeta::new(taker.pubkey(), true),
                AccountMeta::new(maker.pubkey(), false),
                AccountMeta::new(mint_a, false),
                AccountMeta::new(mint_b, false),
                AccountMeta::new(escrow.0, false),
                AccountMeta::new(maker_ata_a, false),
                AccountMeta::new(maker_ata_b, false),
                AccountMeta::new(taker_ata_a, false),
                AccountMeta::new(taker_ata_b, false),
                AccountMeta::new(vault, false),
                AccountMeta::new(system_program, false),
                AccountMeta::new(token_program, false),
                AccountMeta::new(associated_token_program, false)
            ],
            data: take_data,
        };

        // Checking amount in vault before the take transaction
        let escrow_vault = svm.get_account(&vault).unwrap();
        let escrow_vault_state = spl_token_2022::state::Account::unpack(&escrow_vault.data).unwrap();
        println!("Amount of token A in vault that maker give taker is: {}", escrow_vault_state.amount);

        // Create and send the transaction containting the take instructions.
        let take_message = Message::new(&[take_ix], Some(&taker.pubkey()));
        let take_recent_blockhash = svm.latest_blockhash();

        let take_transaction = Transaction::new(&[&taker], take_message, take_recent_blockhash);

        // Send the transaction and capture the result.
        let take_tx = svm.send_transaction(take_transaction).unwrap();
        //println!("The take instruction log: {:#?}", take_tx);

        // --- extra verification of the state of the vault after take -----
        let taker_b_acc = svm.get_account(&taker_ata_b).unwrap(); // Gives token
        let taker_b_state = spl_token_2022::state::Account::unpack(&taker_b_acc.data).unwrap();
        println!("Taker ATA balance token B: {}", taker_b_state.amount);
        let maker_b_acc = svm.get_account(&maker_ata_b).unwrap(); // Takes token
        let maker_b_state = spl_token_2022::state::Account::unpack(&maker_b_acc.data).unwrap();
        // Amount released from the taker_b must be equal to final amount settled in maker's
        // account. (Change)
        assert_eq!(maker_b_state.amount, initial_taker_b_balance - taker_b_state.amount);
        println!("Maker ATA balance token B(Recieved from the taker): {}", maker_b_state.amount);
        let taker_a_acc = svm.get_account(&taker_ata_a).unwrap();
        let taker_a_state = spl_token_2022::state::Account::unpack(&taker_a_acc.data).unwrap();
        // Amount released by the vault should be equal to final amount settled in the taker's
        // account.
        assert_eq!(escrow_vault_state.amount, taker_a_state.amount);
        println!("Taker ATA balance token A(Received from the maker via escrow): {}", taker_a_state.amount);

        // Log transaction details.
        println!("\n\nTake transaction successful");
        println!("CUs Consumed: {}", take_tx.compute_units_consumed);
    }

    #[test]
    pub fn test_make_then_cancel() {
        let (mut svm, maker) = setup();

        let program_id = program_id();

        assert_eq!(program_id.to_string(), PROGRAM_ID);
        // PRECONDITION. THIS IS THE MAKE INSTRUCTION.
        let mint_a = CreateMint::new(&mut svm, &maker)
            .decimals(6)
            .authority(&maker.pubkey())
            .send()
            .unwrap();
        println!("Mint A: {}", mint_a);

        let mint_b = CreateMint::new(&mut svm, &maker)
            .decimals(6)
            .authority(&maker.pubkey())
            .send()
            .unwrap();
        println!("Mint B: {}", mint_b);

        // Create the maker's associated token account for Mint A
        let maker_ata_a = CreateAssociatedTokenAccount::new(&mut svm, &maker, &mint_a)
            .owner(&maker.pubkey()).send().unwrap();
        println!("Maker ATA A: {}\n", maker_ata_a);

        // Derive the PDA for the escrow account using the maker's public key and a seed value
        let escrow = Pubkey::find_program_address(
            &[b"escrow".as_ref(), maker.pubkey().as_ref()],
            &PROGRAM_ID.parse().unwrap(),
        );
        println!("Escrow PDA: {}\n", escrow.0);

        // Derive the PDA for the vault associated token account using the escrow PDA and Mint A
        let vault = spl_associated_token_account::get_associated_token_address(
            &escrow.0,  // owner will be the escrow PDA
            &mint_a     // mint
        );
        println!("Vault PDA: {}\n", vault);

        // Define program IDs for associated token program, token program, and system program
        let associated_token_program = ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();
        let token_program = TOKEN_PROGRAM_ID;
        let system_program = solana_sdk_ids::system_program::ID;

        let maker_ata_init_bal = 1000000000;
        // Mint 1,000 tokens (with 6 decimal places) of Mint A to the maker's associated token account
        MintTo::new(&mut svm, &maker, &mint_a, &maker_ata_a, maker_ata_init_bal)
            .send()
            .unwrap();

        let amount_to_receive: u64 = 100000000; // 100 tokens with 6 decimal places
        let amount_to_give: u64 = 500000000;    // 500 tokens with 6 decimal places
        let bump: u8 = escrow.1;   // canonical bump; the program derives the same one on-chain

        println!("Bump: {}", bump);

        // Create the "Make" instruction to deposit tokens into the escrow
        let make_data = [
            vec![0u8],              // Discriminator for "Make" instruction
            amount_to_receive.to_le_bytes().to_vec(),
            amount_to_give.to_le_bytes().to_vec(),
        ].concat();
        let make_ix = Instruction {
            program_id: program_id,
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

        // Create and send the transaction containing the "Make" instruction
        let message = Message::new(&[make_ix], Some(&maker.pubkey()));
        let recent_blockhash = svm.latest_blockhash();

        let transaction = Transaction::new(&[&maker], message, recent_blockhash);

        // Send the transaction and capture the result
        let tx = svm.send_transaction(transaction).unwrap();

        // ============== CANCEL INSTRUCTION ===================
        // GETTING THE ESCROW ACCOUNT
        // TAKE INSTRUCTION DATA(IS THE INVERSE OF MAKE INSTRUCTION DATA)
        let cancel_data = [
            vec![2u8], // Discriminator for the take instruction.
        ].concat();
        let cancel_ix = Instruction {
            program_id: program_id,
            accounts: vec![
                AccountMeta::new(maker.pubkey(), true),
                AccountMeta::new(mint_a, false),
                AccountMeta::new(mint_b, false),
                AccountMeta::new(escrow.0, false),
                AccountMeta::new(maker_ata_a, false),
                AccountMeta::new(vault, false),
                AccountMeta::new(system_program, false),
                AccountMeta::new(token_program, false),
                AccountMeta::new(associated_token_program, false)
            ],
            data: cancel_data,
        };

        // Checking amount in vault before the take transaction
        let escrow_vault = svm.get_account(&vault).unwrap();
        let escrow_vault_state = spl_token_2022::state::Account::unpack(&escrow_vault.data).unwrap();
        println!("Amount of token A in vault that maker give taker is: {}", escrow_vault_state.amount);

        // Checking balance before refund
        let maker_a_cancel_b= svm.get_account(&maker_ata_a).unwrap(); // Takes token
        let maker_a_cancel_b_state = spl_token_2022::state::Account::unpack(&maker_a_cancel_b.data).unwrap();
        // Amount released from the taker_b must be equal to final amount settled in maker's
        // account. (Change)
        println!("Maker refunded before cancelling escrow: {}", maker_a_cancel_b_state.amount);
        // Amount in the escrow vault should be equal to amount remaining for maker ata in this
        // test context.
        assert_eq!(escrow_vault_state.amount, maker_a_cancel_b_state.amount);

        // Create and send the transaction containting the take instructions.
        let cancel_message = Message::new(&[cancel_ix], Some(&maker.pubkey()));
        let cancel_recent_blockhash = svm.latest_blockhash();

        let cancel_transaction = Transaction::new(&[&maker], cancel_message, cancel_recent_blockhash);

        // Send the transaction and capture the result.
        println!("======= TEST MAKE THEN CANCEL =============");
        let cancel_tx = svm.send_transaction(cancel_transaction).unwrap();
        //println!("The take instruction log: {:#?}", cancel_tx);

        // --- extra verification of the state of the vault after take -----
        let maker_a_cancel_acc= svm.get_account(&maker_ata_a).unwrap(); // Takes token
        let maker_a_cancel_state = spl_token_2022::state::Account::unpack(&maker_a_cancel_acc.data).unwrap();
        // Amount released from the taker_b must be equal to final amount settled in maker's
        // account. (Change)
        println!("Maker refunded after cancelling escrow: {}", maker_a_cancel_state.amount);
        assert_eq!(maker_ata_init_bal, maker_a_cancel_state.amount);

        // Log transaction details.
        println!("\n\nCancel transaction successful");
        println!("CUs Consumed: {}", cancel_tx.compute_units_consumed);
    }

    #[test]
    pub fn test_make_take_fail() {
        // THIS TEST ONLY MODIFIES AMOUNTS, TO CHECK IF THE TAKER CAN AFFORD TO PAY THE PRICE.
        let (mut svm, maker) = setup();

        let program_id = program_id();

        assert_eq!(program_id.to_string(), PROGRAM_ID);
        // PRECONDITION. THIS IS THE MAKE INSTRUCTION.
        let mint_a = CreateMint::new(&mut svm, &maker)
            .decimals(6)
            .authority(&maker.pubkey())
            .send()
            .unwrap();
        println!("Mint A: {}", mint_a);

        let mint_b = CreateMint::new(&mut svm, &maker)
            .decimals(6)
            .authority(&maker.pubkey())
            .send()
            .unwrap();
        println!("Mint B: {}", mint_b);

        // Create the maker's associated token account for Mint A
        let maker_ata_a = CreateAssociatedTokenAccount::new(&mut svm, &maker, &mint_a)
            .owner(&maker.pubkey()).send().unwrap();
        println!("Maker ATA A: {}\n", maker_ata_a);

        // Derive the PDA for the escrow account using the maker's public key and a seed value
        let escrow = Pubkey::find_program_address(
            &[b"escrow".as_ref(), maker.pubkey().as_ref()],
            &PROGRAM_ID.parse().unwrap(),
        );
        println!("Escrow PDA: {}\n", escrow.0);

        // Derive the PDA for the vault associated token account using the escrow PDA and Mint A
        let vault = spl_associated_token_account::get_associated_token_address(
            &escrow.0,  // owner will be the escrow PDA
            &mint_a     // mint
        );
        println!("Vault PDA: {}\n", vault);

        // Define program IDs for associated token program, token program, and system program
        let associated_token_program = ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();
        let token_program = TOKEN_PROGRAM_ID;
        let system_program = solana_sdk_ids::system_program::ID;

        // Mint 1,000 tokens (with 6 decimal places) of Mint A to the maker's associated token account
        MintTo::new(&mut svm, &maker, &mint_a, &maker_ata_a, 1000000000)
            .send()
            .unwrap();

        let amount_to_receive: u64 = 100000000; // 100 tokens with 6 decimal places
        let amount_to_give: u64 = 500000000;    // 500 tokens with 6 decimal places
        let bump: u8 = escrow.1;   // canonical bump; the program derives the same one on-chain

        println!("Bump: {}", bump);

        // Create the "Make" instruction to deposit tokens into the escrow
        let make_data = [
            vec![0u8],              // Discriminator for "Make" instruction
            amount_to_receive.to_le_bytes().to_vec(),
            amount_to_give.to_le_bytes().to_vec(),
        ].concat();
        let make_ix = Instruction {
            program_id: program_id,
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

        // Create and send the transaction containing the "Make" instruction
        let message = Message::new(&[make_ix], Some(&maker.pubkey()));
        let recent_blockhash = svm.latest_blockhash();

        let transaction = Transaction::new(&[&maker], message, recent_blockhash);

        // Send the transaction and capture the result
        let tx = svm.send_transaction(transaction).unwrap();
        println!("Escrow Taker instruction");

        // ============== TAKE INSTRUCTION ===================
        // GETTING THE ESCROW ACCOUNT
        let taker = Keypair::new();
        svm.airdrop(&taker.pubkey(), 1_000_000_000);
        let initial_taker_b_balance = 10000000;
        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut svm, &taker, &mint_b)
            .owner(&taker.pubkey()).send().unwrap();
        // Preload taker_ata_b with tokens to send back to maker
        MintTo::new(&mut svm, &maker, &mint_b, &taker_ata_b, initial_taker_b_balance).send().unwrap();
        let taker_ata_a = CreateAssociatedTokenAccount::new(&mut svm, &taker, &mint_a)
            .owner(&taker.pubkey()).send().unwrap();
        let maker_ata_b = CreateAssociatedTokenAccount::new(&mut svm, &maker, &mint_b)
            .owner(&maker.pubkey()).send().unwrap();
        // TAKE INSTRUCTION DATA(IS THE INVERSE OF MAKE INSTRUCTION DATA)
        let take_data = [
            vec![1u8], // Discriminator for the take instruction.
        ].concat();
        let take_ix = Instruction {
            program_id: program_id,
            accounts: vec![
                AccountMeta::new(taker.pubkey(), true),
                AccountMeta::new(maker.pubkey(), false),
                AccountMeta::new(mint_a, false),
                AccountMeta::new(mint_b, false),
                AccountMeta::new(escrow.0, false),
                AccountMeta::new(maker_ata_a, false),
                AccountMeta::new(maker_ata_b, false),
                AccountMeta::new(taker_ata_a, false),
                AccountMeta::new(taker_ata_b, false),
                AccountMeta::new(vault, false),
                AccountMeta::new(system_program, false),
                AccountMeta::new(token_program, false),
                AccountMeta::new(associated_token_program, false)
            ],
            data: take_data,
        };

        // Checking amount in vault before the take transaction
        let escrow_vault = svm.get_account(&vault).unwrap();
        let escrow_vault_state = spl_token_2022::state::Account::unpack(&escrow_vault.data).unwrap();
        println!("Amount of token A in vault that maker give taker is: {}", escrow_vault_state.amount);

        // Create and send the transaction containting the take instructions.
        let take_message = Message::new(&[take_ix], Some(&taker.pubkey()));
        let take_recent_blockhash = svm.latest_blockhash();

        let take_transaction = Transaction::new(&[&taker], take_message, take_recent_blockhash);

        println!("About to perform Take transaction expected to fail");
        // Send the transaction and capture the result.
        let take_tx = svm.send_transaction(take_transaction);
        //println!("The take instruction log: {:#?}", take_tx);
        assert!(take_tx.is_err());

        // --- extra verification of the state of the vault after take -----
        let taker_b_acc = svm.get_account(&taker_ata_b).unwrap(); // Gives token
        let taker_b_state = spl_token_2022::state::Account::unpack(&taker_b_acc.data).unwrap();
        println!("Taker ATA balance token B(Low balance to give no funds moved): {}", taker_b_state.amount);
        let maker_b_acc = svm.get_account(&maker_ata_b).unwrap(); // Takes token
        let maker_b_state = spl_token_2022::state::Account::unpack(&maker_b_acc.data).unwrap();
        // Maker balance for token b should be 0 since no tokens were moved due to low bal
        assert_eq!(maker_b_state.amount, 0);
        println!("Maker ATA balance token B(Recieved from the taker): {}", maker_b_state.amount);
        let taker_a_acc = svm.get_account(&taker_ata_a).unwrap();
        let taker_a_state = spl_token_2022::state::Account::unpack(&taker_a_acc.data).unwrap();
        // Taker didn't recieve a since these was low b balance to give.
        assert_eq!(taker_a_state.amount, 0);
        // Escrow amount does not change since no funds moved.
        assert_eq!(escrow_vault_state.amount, amount_to_give);
        println!("Taker ATA balance token A(Received from the maker via escrow): {}", taker_a_state.amount);
        let escrow_vault_after = svm.get_account(&vault).unwrap();
        let escrow_vault_state_after = spl_token_2022::state::Account::unpack(&escrow_vault_after.data).unwrap();
        assert_eq!(escrow_vault_state.amount, escrow_vault_state_after.amount);
        println!("Amount of token A in vault after failed transaction(No change): {}", escrow_vault_state.amount);

        // Log transaction details.
        println!("\n\nTake transaction failed successfully");
    }

}
