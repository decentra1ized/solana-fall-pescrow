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

    /// Helper: runs the Make instruction and returns (mint_a, mint_b, escrow_pda, vault, bump).
    /// The maker (payer) deposits `amount_to_give` of mint_a and wants `amount_to_receive` of mint_b.
    fn do_make(
        svm: &mut LiteSVM,
        maker: &Keypair,
        amount_to_receive: u64,
        amount_to_give: u64,
        mint_supply: u64,
    ) -> (Pubkey, Pubkey, Pubkey, Pubkey, u8) {
        let program_id = program_id();
        let associated_token_program = ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();
        let token_program = TOKEN_PROGRAM_ID;
        let system_program = solana_sdk_ids::system_program::ID;

        let mint_a = CreateMint::new(svm, maker).decimals(6).authority(&maker.pubkey()).send().unwrap();
        let mint_b = CreateMint::new(svm, maker).decimals(6).authority(&maker.pubkey()).send().unwrap();

        let maker_ata_a = CreateAssociatedTokenAccount::new(svm, maker, &mint_a)
            .owner(&maker.pubkey()).send().unwrap();

        MintTo::new(svm, maker, &mint_a, &maker_ata_a, mint_supply).send().unwrap();

        let (escrow_pda, bump) = Pubkey::find_program_address(
            &[b"escrow", maker.pubkey().as_ref()], &program_id,
        );
        let vault = spl_associated_token_account::get_associated_token_address(&escrow_pda, &mint_a);

        let make_data = [
            vec![0u8],
            amount_to_receive.to_le_bytes().to_vec(),
            amount_to_give.to_le_bytes().to_vec(),
        ].concat();

        let make_ix = Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new(maker.pubkey(), true),
                AccountMeta::new(mint_a, false),
                AccountMeta::new(mint_b, false),
                AccountMeta::new(escrow_pda, false),
                AccountMeta::new(maker_ata_a, false),
                AccountMeta::new(vault, false),
                AccountMeta::new(system_program, false),
                AccountMeta::new(token_program, false),
                AccountMeta::new(associated_token_program, false),
            ],
            data: make_data,
        };

        let msg = Message::new(&[make_ix], Some(&maker.pubkey()));
        let tx = Transaction::new(&[maker], msg, svm.latest_blockhash());
        let result = svm.send_transaction(tx).unwrap();
        println!("Make CUs: {}", result.compute_units_consumed);

        (mint_a, mint_b, escrow_pda, vault, bump)
    }

    #[test]
    pub fn test_take_instruction() {
        let (mut svm, maker) = setup();
        let taker = Keypair::new();
        svm.airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL).expect("Airdrop failed");

        let amount_to_receive: u64 = 100_000_000; // 100 tokens
        let amount_to_give: u64 = 500_000_000;    // 500 tokens
        let mint_supply: u64 = 1_000_000_000;      // 1000 tokens

        let (mint_a, mint_b, escrow_pda, vault, _bump) =
            do_make(&mut svm, &maker, amount_to_receive, amount_to_give, mint_supply);

        // Snapshot maker SOL before Take so we can verify rent refund
        let maker_sol_before = svm.get_account(&maker.pubkey()).unwrap().lamports;

        // Give the taker some of mint_b so they can pay
        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut svm, &taker, &mint_b)
            .owner(&taker.pubkey()).send().unwrap();
        // Mint B authority is the maker (from do_make), so maker mints to taker
        MintTo::new(&mut svm, &maker, &mint_b, &taker_ata_b, 200_000_000).send().unwrap();

        let program_id = program_id();
        let associated_token_program = ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();
        let token_program = TOKEN_PROGRAM_ID;
        let system_program = solana_sdk_ids::system_program::ID;

        // Derive taker's ATA for A and maker's ATA for B — do NOT create them,
        // the program does that with CreateIdempotent.
        let taker_ata_a = spl_associated_token_account::get_associated_token_address(
            &taker.pubkey(), &mint_a,
        );
        let maker_ata_b = spl_associated_token_account::get_associated_token_address(
            &maker.pubkey(), &mint_b,
        );

        // Take instruction: discriminator = 1, no extra data
        let take_ix = Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new(taker.pubkey(), true),   // 0 taker
                AccountMeta::new(maker.pubkey(), false),   // 1 maker
                AccountMeta::new_readonly(mint_a, false),  // 2 mint_a
                AccountMeta::new_readonly(mint_b, false),  // 3 mint_b
                AccountMeta::new(escrow_pda, false),       // 4 escrow
                AccountMeta::new(vault, false),            // 5 vault
                AccountMeta::new(taker_ata_a, false),      // 6 taker_ata_a
                AccountMeta::new(taker_ata_b, false),      // 7 taker_ata_b
                AccountMeta::new(maker_ata_b, false),      // 8 maker_ata_b
                AccountMeta::new_readonly(system_program, false),
                AccountMeta::new_readonly(token_program, false),
                AccountMeta::new_readonly(associated_token_program, false),
            ],
            data: vec![1u8], // discriminator for Take
        };

        let msg = Message::new(&[take_ix], Some(&taker.pubkey()));
        let tx = Transaction::new(&[&taker], msg, svm.latest_blockhash());
        let result = svm.send_transaction(tx).unwrap();
        println!("\n\nTake transaction successful");
        println!("CUs Consumed: {}", result.compute_units_consumed);

        // Verify: taker now has 500 A
        let taker_a_acc = svm.get_account(&taker_ata_a).unwrap();
        let taker_a_state = spl_token_2022::state::Account::unpack(&taker_a_acc.data).unwrap();
        println!("Taker A balance: {}", taker_a_state.amount);
        assert_eq!(taker_a_state.amount, amount_to_give);

        // Verify: taker B balance decreased by amount_to_receive
        let taker_b_acc = svm.get_account(&taker_ata_b).unwrap();
        let taker_b_state = spl_token_2022::state::Account::unpack(&taker_b_acc.data).unwrap();
        println!("Taker B balance: {}", taker_b_state.amount);
        assert_eq!(taker_b_state.amount, 200_000_000 - amount_to_receive);

        // Verify: maker received 100 B
        let maker_b_acc = svm.get_account(&maker_ata_b).unwrap();
        let maker_b_state = spl_token_2022::state::Account::unpack(&maker_b_acc.data).unwrap();
        println!("Maker B balance: {}", maker_b_state.amount);
        assert_eq!(maker_b_state.amount, amount_to_receive);

        // Verify: vault and escrow accounts are closed
        assert!(svm.get_account(&vault).is_none(), "vault should be closed");
        assert!(svm.get_account(&escrow_pda).is_none(), "escrow should be closed");

        // Verify: maker SOL went up by roughly the rent of both closed accounts
        let maker_sol_after = svm.get_account(&maker.pubkey()).unwrap().lamports;
        let sol_gained = maker_sol_after.saturating_sub(maker_sol_before);
        println!("Maker SOL gained from rent refunds: {} lamports", sol_gained);
        assert!(sol_gained > 0, "maker should have received rent refunds");
    }

    #[test]
    pub fn test_cancel_instruction() {
        let (mut svm, maker) = setup();

        let amount_to_receive: u64 = 100_000_000;
        let amount_to_give: u64 = 500_000_000;
        let mint_supply: u64 = 1_000_000_000;

        let (mint_a, _mint_b, escrow_pda, vault, _bump) =
            do_make(&mut svm, &maker, amount_to_receive, amount_to_give, mint_supply);

        let program_id = program_id();
        let token_program = TOKEN_PROGRAM_ID;

        let maker_ata_a = spl_associated_token_account::get_associated_token_address(
            &maker.pubkey(), &mint_a,
        );

        // Cancel instruction: discriminator = 2
        let cancel_ix = Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new(maker.pubkey(), true),    // 0 maker
                AccountMeta::new_readonly(mint_a, false),  // 1 mint_a
                AccountMeta::new(escrow_pda, false),       // 2 escrow
                AccountMeta::new(vault, false),            // 3 vault
                AccountMeta::new(maker_ata_a, false),      // 4 maker_ata_a
                AccountMeta::new_readonly(token_program, false), // 5 token_program
            ],
            data: vec![2u8],
        };

        let msg = Message::new(&[cancel_ix], Some(&maker.pubkey()));
        let tx = Transaction::new(&[&maker], msg, svm.latest_blockhash());
        let result = svm.send_transaction(tx).unwrap();
        println!("\n\nCancel transaction successful");
        println!("CUs Consumed: {}", result.compute_units_consumed);

        // Verify: maker got all A back
        let maker_a_acc = svm.get_account(&maker_ata_a).unwrap();
        let maker_a_state = spl_token_2022::state::Account::unpack(&maker_a_acc.data).unwrap();
        println!("Maker A balance after cancel: {}", maker_a_state.amount);
        assert_eq!(maker_a_state.amount, mint_supply);

        // Verify: vault and escrow accounts are closed
        assert!(svm.get_account(&vault).is_none(), "vault should be closed");
        assert!(svm.get_account(&escrow_pda).is_none(), "escrow should be closed");
    }

    #[test]
    pub fn test_cancel_only_maker() {
        let (mut svm, maker) = setup();
        let imposter = Keypair::new();
        svm.airdrop(&imposter.pubkey(), 10 * LAMPORTS_PER_SOL).expect("Airdrop failed");

        let (mint_a, _mint_b, escrow_pda, vault, _bump) =
            do_make(&mut svm, &maker, 100_000_000, 500_000_000, 1_000_000_000);

        let program_id = program_id();
        let token_program = TOKEN_PROGRAM_ID;

        // The imposter tries to cancel using their own key as maker — but the escrow
        // records the real maker, so the cross-check must fail.
        let imposter_ata_a = spl_associated_token_account::get_associated_token_address(
            &imposter.pubkey(), &mint_a,
        );

        let cancel_ix = Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new(imposter.pubkey(), true),
                AccountMeta::new_readonly(mint_a, false),
                AccountMeta::new(escrow_pda, false),
                AccountMeta::new(vault, false),
                AccountMeta::new(imposter_ata_a, false),
                AccountMeta::new_readonly(token_program, false),
            ],
            data: vec![2u8],
        };

        let msg = Message::new(&[cancel_ix], Some(&imposter.pubkey()));
        let tx = Transaction::new(&[&imposter], msg, svm.latest_blockhash());
        let result = svm.send_transaction(tx);
        assert!(result.is_err(), "Cancel by non-maker should fail");
        println!("\nCancel by non-maker correctly rejected");

        // Prove nothing moved: vault still holds 500 A
        let vault_acc = svm.get_account(&vault).unwrap();
        let vault_state = spl_token_2022::state::Account::unpack(&vault_acc.data).unwrap();
        assert_eq!(vault_state.amount, 500_000_000, "vault must still hold 500 A");
        println!("Vault still holds {} A — funds are safe", vault_state.amount);
    }

    #[test]
    pub fn test_take_insufficient_balance() {
        let (mut svm, maker) = setup();
        let taker = Keypair::new();
        svm.airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL).expect("Airdrop failed");

        let amount_to_receive: u64 = 100_000_000;
        let (mint_a, mint_b, escrow_pda, vault, _bump) =
            do_make(&mut svm, &maker, amount_to_receive, 500_000_000, 1_000_000_000);

        // Give taker only 50 B — not enough (needs 100)
        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut svm, &taker, &mint_b)
            .owner(&taker.pubkey()).send().unwrap();
        MintTo::new(&mut svm, &maker, &mint_b, &taker_ata_b, 50_000_000).send().unwrap();

        let program_id = program_id();
        let associated_token_program = ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();
        let token_program = TOKEN_PROGRAM_ID;
        let system_program = solana_sdk_ids::system_program::ID;

        let taker_ata_a = spl_associated_token_account::get_associated_token_address(
            &taker.pubkey(), &mint_a,
        );
        let maker_ata_b = spl_associated_token_account::get_associated_token_address(
            &maker.pubkey(), &mint_b,
        );

        let take_ix = Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new(taker.pubkey(), true),
                AccountMeta::new(maker.pubkey(), false),
                AccountMeta::new_readonly(mint_a, false),
                AccountMeta::new_readonly(mint_b, false),
                AccountMeta::new(escrow_pda, false),
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

        let msg = Message::new(&[take_ix], Some(&taker.pubkey()));
        let tx = Transaction::new(&[&taker], msg, svm.latest_blockhash());
        let result = svm.send_transaction(tx);
        assert!(result.is_err(), "Take with insufficient B should fail");
        println!("\nTake with insufficient balance correctly rejected");

        // Prove nothing moved: vault still holds 500 A
        let vault_acc = svm.get_account(&vault).unwrap();
        let vault_state = spl_token_2022::state::Account::unpack(&vault_acc.data).unwrap();
        assert_eq!(vault_state.amount, 500_000_000, "vault must still hold 500 A");
        println!("Vault still holds {} A — funds are safe", vault_state.amount);
    }

    #[test]
    pub fn test_take_mismatched_maker() {
        let (mut svm, maker) = setup();
        let taker = Keypair::new();
        let fake_maker = Keypair::new();
        svm.airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL).expect("Airdrop failed");
        svm.airdrop(&fake_maker.pubkey(), 10 * LAMPORTS_PER_SOL).expect("Airdrop failed");

        let amount_to_receive: u64 = 100_000_000;
        let amount_to_give: u64 = 500_000_000;

        let (mint_a, mint_b, escrow_pda, vault, _bump) =
            do_make(&mut svm, &maker, amount_to_receive, amount_to_give, 1_000_000_000);

        // Give the taker enough B to pay
        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut svm, &taker, &mint_b)
            .owner(&taker.pubkey()).send().unwrap();
        MintTo::new(&mut svm, &maker, &mint_b, &taker_ata_b, 200_000_000).send().unwrap();

        let program_id = program_id();
        let associated_token_program = ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();
        let token_program = TOKEN_PROGRAM_ID;
        let system_program = solana_sdk_ids::system_program::ID;

        let taker_ata_a = spl_associated_token_account::get_associated_token_address(
            &taker.pubkey(), &mint_a,
        );
        // Pass fake_maker instead of the real maker — the cross-check must catch this
        let fake_maker_ata_b = spl_associated_token_account::get_associated_token_address(
            &fake_maker.pubkey(), &mint_b,
        );

        let take_ix = Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new(taker.pubkey(), true),        // 0 taker
                AccountMeta::new(fake_maker.pubkey(), false),  // 1 WRONG maker
                AccountMeta::new_readonly(mint_a, false),
                AccountMeta::new_readonly(mint_b, false),
                AccountMeta::new(escrow_pda, false),
                AccountMeta::new(vault, false),
                AccountMeta::new(taker_ata_a, false),
                AccountMeta::new(taker_ata_b, false),
                AccountMeta::new(fake_maker_ata_b, false),
                AccountMeta::new_readonly(system_program, false),
                AccountMeta::new_readonly(token_program, false),
                AccountMeta::new_readonly(associated_token_program, false),
            ],
            data: vec![1u8],
        };

        let msg = Message::new(&[take_ix], Some(&taker.pubkey()));
        let tx = Transaction::new(&[&taker], msg, svm.latest_blockhash());
        let result = svm.send_transaction(tx);
        assert!(result.is_err(), "Take with mismatched maker must fail");
        println!("\nTake with mismatched maker correctly rejected");

        // Prove nothing moved: vault still holds 500 A
        let vault_acc = svm.get_account(&vault).unwrap();
        let vault_state = spl_token_2022::state::Account::unpack(&vault_acc.data).unwrap();
        assert_eq!(vault_state.amount, amount_to_give, "vault must still hold 500 A");
        println!("Vault still holds {} A — funds are safe", vault_state.amount);
    }
}