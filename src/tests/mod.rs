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

    // -------------------------------------------------------------------------
    // Shared helper: runs Make and returns everything Take / Cancel tests need.
    // -------------------------------------------------------------------------

    struct MakeResult {
        svm: LiteSVM,
        maker: Keypair,
        mint_a: Pubkey,
        mint_b: Pubkey,
        escrow: Pubkey,
        #[allow(dead_code)]
        bump: u8,
        vault: Pubkey,
        maker_ata_a: Pubkey,
        amount_to_receive: u64,
        amount_to_give: u64,
    }

    fn make_escrow() -> MakeResult {
        let (mut svm, payer) = setup();

        let program_id = program_id();

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

        let maker_ata_a = CreateAssociatedTokenAccount::new(&mut svm, &payer, &mint_a)
            .owner(&payer.pubkey()).send().unwrap();

        let (escrow, bump) = Pubkey::find_program_address(
            &[b"escrow".as_ref(), payer.pubkey().as_ref()],
            &PROGRAM_ID.parse().unwrap(),
        );

        let vault = spl_associated_token_account::get_associated_token_address(&escrow, &mint_a);

        let associated_token_program = ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();
        let token_program = TOKEN_PROGRAM_ID;
        let system_program = solana_sdk_ids::system_program::ID;

        let amount_to_receive: u64 = 100_000_000;
        let amount_to_give: u64 = 500_000_000;

        MintTo::new(&mut svm, &payer, &mint_a, &maker_ata_a, 1_000_000_000)
            .send()
            .unwrap();

        let make_data = [
            vec![0u8],
            amount_to_receive.to_le_bytes().to_vec(),
            amount_to_give.to_le_bytes().to_vec(),
        ].concat();

        let make_ix = Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new(payer.pubkey(), true),
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

        let message = Message::new(&[make_ix], Some(&payer.pubkey()));
        let recent_blockhash = svm.latest_blockhash();
        let transaction = Transaction::new(&[&payer], message, recent_blockhash);
        svm.send_transaction(transaction).unwrap();

        MakeResult { svm, maker: payer, mint_a, mint_b, escrow, bump, vault, maker_ata_a, amount_to_receive, amount_to_give }
    }

    // -------------------------------------------------------------------------
    // Challenge 0 (original): Make
    // -------------------------------------------------------------------------

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
            &escrow.0,
            &mint_a
        );
        println!("Vault PDA: {}\n", vault);

        let associated_token_program = ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();
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

    // -------------------------------------------------------------------------
    // Challenge 1: Take (happy path)
    // -------------------------------------------------------------------------

    #[test]
    pub fn test_take_instruction() {
        let MakeResult { mut svm, maker, mint_a, mint_b, escrow, vault, amount_to_receive, amount_to_give, .. } = make_escrow();

        let program_id = program_id();
        let token_program = TOKEN_PROGRAM_ID;
        let system_program = solana_sdk_ids::system_program::ID;
        let associated_token_program = ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();

        let taker = Keypair::new();
        svm.airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

        // Create taker's ATA for mint B and mint the exact amount_to_receive into it.
        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut svm, &taker, &mint_b)
            .owner(&taker.pubkey()).send().unwrap();
        MintTo::new(&mut svm, &maker, &mint_b, &taker_ata_b, amount_to_receive).send().unwrap();

        // Our program creates these via CreateIdempotent — do NOT pre-create them.
        let taker_ata_a = spl_associated_token_account::get_associated_token_address(&taker.pubkey(), &mint_a);
        let maker_ata_b = spl_associated_token_account::get_associated_token_address(&maker.pubkey(), &mint_b);

        // Account order matches take.rs header comment.
        let take_ix = Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new(taker.pubkey(), true),            // 0 taker
                AccountMeta::new(maker.pubkey(), false),            // 1 maker
                AccountMeta::new(mint_a, false),                    // 2 mint_a
                AccountMeta::new(mint_b, false),                    // 3 mint_b
                AccountMeta::new(escrow, false),                    // 4 escrow_account
                AccountMeta::new(vault, false),                     // 5 vault
                AccountMeta::new(taker_ata_a, false),               // 6 taker_ata_a
                AccountMeta::new(taker_ata_b, false),               // 7 taker_ata_b
                AccountMeta::new(maker_ata_b, false),               // 8 maker_ata_b
                AccountMeta::new(system_program, false),            // 9 system_program
                AccountMeta::new(token_program, false),             // 10 token_program
                AccountMeta::new(associated_token_program, false),  // 11 associated_token_program
            ],
            data: vec![1u8],
        };

        let message = Message::new(&[take_ix], Some(&taker.pubkey()));
        let blockhash = svm.latest_blockhash();
        let tx = svm.send_transaction(Transaction::new(&[&taker], message, blockhash)).unwrap();

        println!("\n\nTake transaction successful");
        println!("CUs Consumed: {}", tx.compute_units_consumed);

        // taker_ata_a must hold amount_to_give (500 A).
        let taker_a = spl_token_2022::state::Account::unpack(&svm.get_account(&taker_ata_a).unwrap().data).unwrap();
        println!("Taker ATA A balance: {}", taker_a.amount);
        assert_eq!(taker_a.amount, amount_to_give, "taker should receive all vault A tokens");

        // maker_ata_b must hold amount_to_receive (100 B).
        let maker_b = spl_token_2022::state::Account::unpack(&svm.get_account(&maker_ata_b).unwrap().data).unwrap();
        println!("Maker ATA B balance: {}", maker_b.amount);
        assert_eq!(maker_b.amount, amount_to_receive, "maker should receive the agreed B tokens");

        // Vault and escrow must be closed.
        let vault_after = svm.get_account(&vault);
        println!("Vault lamports after take: {:?}", vault_after.as_ref().map(|a| a.lamports));
        assert!(vault_after.is_none() || vault_after.unwrap().lamports == 0, "vault should be closed");

        let escrow_after = svm.get_account(&escrow);
        println!("Escrow lamports after take: {:?}", escrow_after.as_ref().map(|a| a.lamports));
        assert!(escrow_after.is_none() || escrow_after.unwrap().lamports == 0, "escrow should be closed");
    }

    // -------------------------------------------------------------------------
    // Challenge 2: Cancel (happy path)
    // -------------------------------------------------------------------------

    #[test]
    pub fn test_cancel_instruction() {
        let MakeResult { mut svm, maker, mint_a, escrow, vault, maker_ata_a, amount_to_give, .. } = make_escrow();

        let program_id = program_id();
        let token_program = TOKEN_PROGRAM_ID;

        // Account order matches cancel.rs header comment.
        let cancel_ix = Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new(maker.pubkey(), true),  // 0 maker
                AccountMeta::new(mint_a, false),          // 1 mint_a
                AccountMeta::new(escrow, false),           // 2 escrow_account
                AccountMeta::new(vault, false),            // 3 vault
                AccountMeta::new(maker_ata_a, false),      // 4 maker_ata_a
                AccountMeta::new(token_program, false),    // 5 token_program
            ],
            data: vec![2u8],
        };

        let message = Message::new(&[cancel_ix], Some(&maker.pubkey()));
        let blockhash = svm.latest_blockhash();
        let tx = svm.send_transaction(Transaction::new(&[&maker], message, blockhash)).unwrap();

        println!("\n\nCancel transaction successful");
        println!("CUs Consumed: {}", tx.compute_units_consumed);

        // maker_ata_a must be back to 1,000 A (1_000_000_000 with 6 decimals).
        let maker_a = spl_token_2022::state::Account::unpack(&svm.get_account(&maker_ata_a).unwrap().data).unwrap();
        println!("Maker ATA A balance after cancel: {}", maker_a.amount);
        // Started with 1_000_000_000, put 500_000_000 in vault; cancel returns 500_000_000.
        let _ = amount_to_give; // used implicitly in the assertion below
        assert_eq!(maker_a.amount, 1_000_000_000, "maker should get all tokens back");

        // Vault and escrow must be closed.
        let vault_after = svm.get_account(&vault);
        assert!(vault_after.is_none() || vault_after.unwrap().lamports == 0, "vault should be closed after cancel");

        let escrow_after = svm.get_account(&escrow);
        assert!(escrow_after.is_none() || escrow_after.unwrap().lamports == 0, "escrow should be closed after cancel");
    }

    // -------------------------------------------------------------------------
    // Challenge 3 negative: taker with insufficient B balance must fail
    // -------------------------------------------------------------------------

    #[test]
    pub fn test_take_underfunded_taker() {
        let MakeResult { mut svm, maker, mint_a, mint_b, escrow, vault, amount_to_give, .. } = make_escrow();

        let program_id = program_id();
        let token_program = TOKEN_PROGRAM_ID;
        let system_program = solana_sdk_ids::system_program::ID;
        let associated_token_program = ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();

        let taker = Keypair::new();
        svm.airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut svm, &taker, &mint_b)
            .owner(&taker.pubkey()).send().unwrap();
        // Only 50 B minted — escrow requires 100 B.
        MintTo::new(&mut svm, &maker, &mint_b, &taker_ata_b, 50_000_000).send().unwrap();

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
        let blockhash = svm.latest_blockhash();
        let result = svm.send_transaction(Transaction::new(&[&taker], message, blockhash));
        assert!(result.is_err(), "underfunded taker must not succeed");

        // Vault must still hold the original amount — nothing moved.
        let vault_state = spl_token_2022::state::Account::unpack(&svm.get_account(&vault).unwrap().data).unwrap();
        println!("Vault balance after failed take: {}", vault_state.amount);
        assert_eq!(vault_state.amount, amount_to_give, "vault tokens must not move");
    }

    // -------------------------------------------------------------------------
    // Challenge 3 negative: stranger cancel must fail and leave vault intact
    // -------------------------------------------------------------------------

    #[test]
    pub fn test_cancel_by_stranger() {
        let MakeResult { mut svm, mint_a, escrow, vault, amount_to_give, .. } = make_escrow();

        let program_id = program_id();
        let token_program = TOKEN_PROGRAM_ID;

        let stranger = Keypair::new();
        svm.airdrop(&stranger.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

        // Give the stranger their own ATA so the instruction can be constructed.
        let stranger_ata_a = CreateAssociatedTokenAccount::new(&mut svm, &stranger, &mint_a)
            .owner(&stranger.pubkey()).send().unwrap();

        let cancel_ix = Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new(stranger.pubkey(), true), // 0 fake "maker"
                AccountMeta::new(mint_a, false),            // 1 mint_a
                AccountMeta::new(escrow, false),             // 2 escrow_account
                AccountMeta::new(vault, false),              // 3 vault
                AccountMeta::new(stranger_ata_a, false),     // 4 destination
                AccountMeta::new(token_program, false),      // 5 token_program
            ],
            data: vec![2u8],
        };

        let message = Message::new(&[cancel_ix], Some(&stranger.pubkey()));
        let blockhash = svm.latest_blockhash();
        let result = svm.send_transaction(Transaction::new(&[&stranger], message, blockhash));
        assert!(result.is_err(), "a stranger must not be able to cancel");

        // Vault tokens must be untouched.
        let vault_state = spl_token_2022::state::Account::unpack(&svm.get_account(&vault).unwrap().data).unwrap();
        println!("Vault balance after stranger cancel attempt: {}", vault_state.amount);
        assert_eq!(vault_state.amount, amount_to_give, "vault tokens must not move");
    }
}