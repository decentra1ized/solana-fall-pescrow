#[cfg(test)]
mod tests {

    use std::path::PathBuf;

    use litesvm::LiteSVM;
    use litesvm_token::{
        spl_token::{self},
        CreateAssociatedTokenAccount, CreateMint, MintTo,
    };

    // use crate::state::{escrow, Escrow};
    use solana_address::Address;
    use solana_instruction::{AccountMeta, Instruction};
    use solana_keypair::Keypair;
    use solana_message::Message;
    use solana_native_token::LAMPORTS_PER_SOL;
    use solana_program_pack::Pack;
    use solana_pubkey::Pubkey;
    use solana_signer::Signer;
    use solana_transaction::Transaction;

    const PROGRAM_ID: &str = "4ibrEMW5F6hKnkW4jVedswYv6H6VtwPN6ar6dvXDN1nT";
    const TOKEN_PROGRAM_ID: Pubkey = spl_token::ID;
    const ASSOCIATED_TOKEN_PROGRAM_ID: &str = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL";
    const AMOUNT_TO_RECEIVE: u64 = 100000000; // 100 token B with 6 decimal places
    const AMOUNT_TO_GIVE: u64 = 500000000; // 500 token A with 6 decimal places
    const AMOUNT_TO_MINT_MAKER: u64 = 1000000000; // 1000 token A with 6 decimal places
    const AMOUNT_TO_MINT_TAKER: u64 = 1000000000; // 1000 token A with 6 decimal places

    fn program_id() -> Pubkey {
        Pubkey::from(crate::ID)
    }

    fn setup() -> (LiteSVM, Keypair, Keypair) {
        let mut svm = LiteSVM::new();
        let maker = Keypair::new();
        let taker = Keypair::new();

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

        svm.airdrop(&maker.pubkey(), 10 * LAMPORTS_PER_SOL)
            .expect("Airdrop failed");
        svm.airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL)
            .expect("Airdrop failed");

        // Load program SO file (produced by `cargo build-sbf`)
        let so_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/deploy/escrow.so");

        let program_data = std::fs::read(&so_path).unwrap_or_else(|e| {
            panic!(
                "Failed to read program SO file at {}: {e}. Run `cargo build-sbf` first.",
                so_path.display()
            )
        });

        svm.add_program(program_id(), &program_data)
            .expect("Failed to add program");

        (svm, maker, taker)
    }

    fn setup_mint(svm: &mut LiteSVM, payer: &Keypair, decimals: u8) -> Address {
        CreateMint::new(svm, payer)
            .decimals(decimals)
            .authority(&payer.pubkey())
            .send()
            .unwrap()
    }

    fn get_ata(owner: &Address, mint: &Address) -> Address {
        spl_associated_token_account::get_associated_token_address(owner, mint)
    }

    fn mint_b_to_taker_ata(svm: &mut LiteSVM, mint_b: Address, taker: &Keypair, maker: &Keypair) {
        // Create the maker's associated token account for Mint A
        let taker_ata_b = CreateAssociatedTokenAccount::new(svm, taker, &mint_b)
            .owner(&taker.pubkey())
            .send()
            .unwrap();
        println!("Taker ATA A: {}\n", taker_ata_b);

        MintTo::new(svm, maker, &mint_b, &taker_ata_b, AMOUNT_TO_MINT_TAKER)
            .send()
            .unwrap();
    }

    fn send_take_ix(
        svm: &mut LiteSVM,
        maker: &Keypair,
        taker: &Keypair,
        mint_a: Address,
        mint_b: Address,
        escrow: Address,
    ) {
        // Create the "Take" instruction to swap tokens from escrow
        let take_data = [
            vec![1u8], // Discriminator for "Make" instruction
        ]
        .concat();

        let vault = get_ata(&escrow, &mint_a);
        let taker_ata_a = get_ata(&taker.pubkey(), &mint_a);
        let taker_ata_b = get_ata(&taker.pubkey(), &mint_b);
        let maker_ata_b = get_ata(&maker.pubkey(), &mint_b);

        let associated_token_program = ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();
        let token_program = TOKEN_PROGRAM_ID;
        let system_program = solana_sdk_ids::system_program::ID;
        // [taker, maker, mint_a, mint_b, escrow_account, vault, taker_ata_a, taker_ata_b, maker_ata_b, system_program, token_program, _associated_token_program @ ..]
        let take_ix = Instruction {
            program_id: program_id(),
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
            data: take_data,
        };

        // Create and send the transaction containing the "Make" instruction
        let message = Message::new(&[take_ix], Some(&taker.pubkey()));
        let recent_blockhash = svm.latest_blockhash();

        let transaction = Transaction::new(&[taker], message, recent_blockhash);

        let tx_result = svm.send_transaction(transaction);
        println!("{:?}", tx_result);

        // Send the transaction and capture the result
        let tx = tx_result.unwrap();

        // Log transaction details
        println!("\n\nTake transaction successful");
        println!("CUs Consumed: {}", tx.compute_units_consumed);
    }

    fn send_cancel_ix(
        svm: &mut LiteSVM,
        maker: &Keypair,
        mint_a: Address,
        escrow: Address,
    ) {
        // Create the "Cancel" instruction to refund tokens from escrow
        let cancel_data = [
            vec![2u8], // Discriminator for "Cancel" instruction
        ]
        .concat();

        let vault = get_ata(&escrow, &mint_a);
        let maker_ata_a = get_ata(&maker.pubkey(), &mint_a);
        let token_program = TOKEN_PROGRAM_ID;

        // [maker, mint_a, escrow_account, vault, maker_ata_a, token_program, ..]
        let cancel_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(maker.pubkey(), true),
                AccountMeta::new(mint_a, false),
                AccountMeta::new(escrow, false),
                AccountMeta::new(vault, false),
                AccountMeta::new(maker_ata_a, false),
                AccountMeta::new(token_program, false),
            ],
            data: cancel_data,
        };

        // Create and send the transaction containing the "Cancel" instruction
        let message = Message::new(&[cancel_ix], Some(&maker.pubkey()));
        let recent_blockhash = svm.latest_blockhash();

        let transaction = Transaction::new(&[maker], message, recent_blockhash);

        let tx_result = svm.send_transaction(transaction);
        println!("{:?}", tx_result);

        // Send the transaction and capture the result
        let tx = tx_result.unwrap();

        // Log transaction details
        println!("\n\nCancel transaction successful");
        println!("CUs Consumed: {}", tx.compute_units_consumed);
    }

    // Returns escrow address
    fn setup_make(
        svm: &mut LiteSVM,
        mint_a: Address,
        mint_b: Address,
        payer: &Keypair,
    ) -> (Address, u8) {
        // Create the maker's associated token account for Mint A
        let maker_ata_a = CreateAssociatedTokenAccount::new(svm, payer, &mint_a)
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
        let vault = get_ata(
            &escrow.0, // owner will be the escrow PDA
            &mint_a,   // mint
        );
        println!("Vault PDA: {}\n", vault);

        // Define program IDs for associated token program, token program, and system program
        let associated_token_program = ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();
        let token_program = TOKEN_PROGRAM_ID;
        let system_program = solana_sdk_ids::system_program::ID;

        // Mint 1,000 tokens (with 6 decimal places) of Mint A to the maker's associated token account
        MintTo::new(svm, payer, &mint_a, &maker_ata_a, AMOUNT_TO_MINT_MAKER)
            .send()
            .unwrap();

        let bump: u8 = escrow.1; // canonical bump; the program derives the same one on-chain

        println!("Bump: {}", bump);

        // Create the "Make" instruction to deposit tokens into the escrow
        let make_data = [
            vec![0u8], // Discriminator for "Make" instruction
            AMOUNT_TO_RECEIVE.to_le_bytes().to_vec(),
            AMOUNT_TO_GIVE.to_le_bytes().to_vec(),
        ]
        .concat();
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

        // Create and send the transaction containing the "Make" instruction
        let message = Message::new(&[make_ix], Some(&payer.pubkey()));
        let recent_blockhash = svm.latest_blockhash();

        let transaction = Transaction::new(&[payer], message, recent_blockhash);

        // Send the transaction and capture the result
        let tx = svm.send_transaction(transaction).unwrap();

        // Log transaction details
        println!("\n\nMake transaction successful");
        println!("CUs Consumed: {}", tx.compute_units_consumed);

        escrow
    }

    #[test]
    pub fn test_make_instruction() {
        let (mut svm, maker, ..) = setup();

        let program_id = program_id();

        assert_eq!(program_id.to_string(), PROGRAM_ID);

        let mint_a = setup_mint(&mut svm, &maker, 6);
        println!("Mint A: {}", mint_a);

        let mint_b = setup_mint(&mut svm, &maker, 6);
        println!("Mint B: {}", mint_b);

        let (escrow, bump) = setup_make(&mut svm, mint_a, mint_b, &maker);
        let vault = get_ata(&escrow, &mint_a);
        let maker_ata_a = get_ata(&maker.pubkey(), &mint_a);

        // --- extra verification ---
        let vault_acc = svm.get_account(&vault).unwrap();
        let vault_state = spl_token_2022::state::Account::unpack(&vault_acc.data).unwrap();
        println!(
            "Vault owner: {} (escrow PDA? {})",
            vault_state.owner,
            vault_state.owner == escrow
        );
        println!("Vault balance: {}", vault_state.amount);
        assert_eq!(vault_state.amount, AMOUNT_TO_GIVE);

        let maker_acc = svm.get_account(&maker_ata_a).unwrap();
        let maker_state = spl_token_2022::state::Account::unpack(&maker_acc.data).unwrap();
        println!("Maker ATA balance: {}", maker_state.amount);
        assert_eq!(maker_state.amount, 1000000000 - AMOUNT_TO_GIVE);

        let esc = svm.get_account(&escrow).unwrap();
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
        assert_eq!(&d[0..32], maker.pubkey().as_ref());
        assert_eq!(
            u64::from_le_bytes(d[96..104].try_into().unwrap()),
            AMOUNT_TO_RECEIVE
        );
        assert_eq!(
            u64::from_le_bytes(d[104..112].try_into().unwrap()),
            AMOUNT_TO_GIVE
        );
        assert_eq!(d[112], bump);
    }

    #[test]
    pub fn test_take_instruction() {
        let (mut svm, maker, taker) = setup();

        let program_id = program_id();

        assert_eq!(program_id.to_string(), PROGRAM_ID);

        let mint_a = setup_mint(&mut svm, &maker, 6);
        println!("Mint A: {}", mint_a);

        let mint_b = setup_mint(&mut svm, &maker, 6);
        println!("Mint B: {}", mint_b);

        let (escrow, _bump) = setup_make(&mut svm, mint_a, mint_b, &maker);
        let vault = get_ata(&escrow, &mint_a);
        let maker_ata_b = get_ata(&maker.pubkey(), &mint_b);
        let taker_ata_a = get_ata(&taker.pubkey(), &mint_a);
        mint_b_to_taker_ata(&mut svm, mint_b, &taker, &maker);

        let vault_lamports_before = svm.get_account(&vault).unwrap().lamports;
        let escrow_lamports_before = svm.get_account(&escrow).unwrap().lamports;
        let maker_lamports_before = svm.get_account(&maker.pubkey()).unwrap().lamports;

        send_take_ix(&mut svm, &maker, &taker, mint_a, mint_b, escrow);

        // vault and escrow accounts are closed
        assert!(svm.get_account(&vault).is_none());
        assert!(svm.get_account(&escrow).is_none());

        // maker received token B
        let maker_ata_b_state =
            spl_token_2022::state::Account::unpack(&svm.get_account(&maker_ata_b).unwrap().data)
                .unwrap();
        assert_eq!(maker_ata_b_state.amount, AMOUNT_TO_RECEIVE);

        // taker received token A
        let taker_ata_a_state =
            spl_token_2022::state::Account::unpack(&svm.get_account(&taker_ata_a).unwrap().data)
                .unwrap();
        assert_eq!(taker_ata_a_state.amount, AMOUNT_TO_GIVE);

        // maker received the vault and escrow rent back
        let maker_lamports_after = svm.get_account(&maker.pubkey()).unwrap().lamports;
        assert_eq!(
            maker_lamports_after,
            maker_lamports_before + vault_lamports_before + escrow_lamports_before
        );
    }

    #[test]
    pub fn test_cancel_instruction() {
        let (mut svm, maker, ..) = setup();

        let program_id = program_id();

        assert_eq!(program_id.to_string(), PROGRAM_ID);

        let mint_a = setup_mint(&mut svm, &maker, 6);
        println!("Mint A: {}", mint_a);

        let mint_b = setup_mint(&mut svm, &maker, 6);
        println!("Mint B: {}", mint_b);

        let (escrow, _bump) = setup_make(&mut svm, mint_a, mint_b, &maker);
        let vault = get_ata(&escrow, &mint_a);
        let maker_ata_a = get_ata(&maker.pubkey(), &mint_a);

        let vault_lamports_before = svm.get_account(&vault).unwrap().lamports;
        let escrow_lamports_before = svm.get_account(&escrow).unwrap().lamports;
        let maker_lamports_before = svm.get_account(&maker.pubkey()).unwrap().lamports;
        let maker_ata_a_before =
            spl_token_2022::state::Account::unpack(&svm.get_account(&maker_ata_a).unwrap().data)
                .unwrap()
                .amount;

        send_cancel_ix(&mut svm, &maker, mint_a, escrow);

        // vault and escrow accounts are closed
        assert!(svm.get_account(&vault).is_none());
        assert!(svm.get_account(&escrow).is_none());

        // maker received back the deposited token A
        let maker_ata_a_state =
            spl_token_2022::state::Account::unpack(&svm.get_account(&maker_ata_a).unwrap().data)
                .unwrap();
        assert_eq!(maker_ata_a_state.amount, maker_ata_a_before + AMOUNT_TO_GIVE);

        // maker received the vault and escrow rent back (minus the tx fee, since maker is also the fee payer)
        const TX_FEE: u64 = 5000;
        let maker_lamports_after = svm.get_account(&maker.pubkey()).unwrap().lamports;
        assert_eq!(
            maker_lamports_after,
            maker_lamports_before + vault_lamports_before + escrow_lamports_before - TX_FEE
        );
    }

    #[test]
    pub fn test_invalid_escrow_take_rejected() {
        let (mut svm, maker, taker) = setup();

        let mint_a = setup_mint(&mut svm, &maker, 6);
        let mint_b = setup_mint(&mut svm, &maker, 6);

        let (escrow, _bump) = setup_make(&mut svm, mint_a, mint_b, &maker);
        mint_b_to_taker_ata(&mut svm, mint_b, &taker, &maker);

        let vault = get_ata(&escrow, &mint_a);
        let taker_ata_a = get_ata(&taker.pubkey(), &mint_a);
        let taker_ata_b = get_ata(&taker.pubkey(), &mint_b);
        let maker_ata_b = get_ata(&maker.pubkey(), &mint_b);

        let associated_token_program = ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();
        let token_program = TOKEN_PROGRAM_ID;
        let system_program = solana_sdk_ids::system_program::ID;

        // mint_b is owned by the token program, not the escrow program, so passing it
        // in place of the escrow account should trip the owner check.
        let take_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(taker.pubkey(), true),
                AccountMeta::new(maker.pubkey(), false),
                AccountMeta::new(mint_a, false),
                AccountMeta::new(mint_b, false),
                AccountMeta::new(mint_b, false),
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

        let err = svm.send_transaction(transaction).unwrap_err();
        println!("{:?}", err);
        let err_str = format!("{:?}", err);
        assert!(err_str.contains("IllegalOwner"), "{}", err_str);
    }

    #[test]
    pub fn test_invalid_mint_cancel_rejected() {
        let (mut svm, maker, ..) = setup();

        let mint_a = setup_mint(&mut svm, &maker, 6);
        let mint_b = setup_mint(&mut svm, &maker, 6);

        let (escrow, _bump) = setup_make(&mut svm, mint_a, mint_b, &maker);

        let vault = get_ata(&escrow, &mint_a);
        let maker_ata_a = get_ata(&maker.pubkey(), &mint_a);
        let token_program = TOKEN_PROGRAM_ID;

        // mint_b is owned by the token program, not the escrow program, so passing it
        // in place of the escrow account should trip the owner check.
        let cancel_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(maker.pubkey(), true),
                AccountMeta::new(mint_a, false),
                AccountMeta::new(mint_b, false),
                AccountMeta::new(vault, false),
                AccountMeta::new(maker_ata_a, false),
                AccountMeta::new(token_program, false),
            ],
            data: vec![2u8],
        };

        let message = Message::new(&[cancel_ix], Some(&maker.pubkey()));
        let recent_blockhash = svm.latest_blockhash();
        let transaction = Transaction::new(&[&maker], message, recent_blockhash);

        let err = svm.send_transaction(transaction).unwrap_err();
        println!("{:?}", err);
        let err_str = format!("{:?}", err);
        assert!(err_str.contains("IllegalOwner"), "{}", err_str);
    }
}
