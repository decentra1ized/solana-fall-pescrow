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


    // Amounts shared by every test: the maker offers 500 A and wants 100 B.
    const MAKER_STARTING_A: u64 = 1_000_000_000; // 1,000 tokens at 6 decimals
    const AMOUNT_TO_RECEIVE: u64 = 100_000_000;  // 100 B
    const AMOUNT_TO_GIVE: u64 = 500_000_000;     // 500 A

    /// Everything a post-Make test needs to drive Take or Cancel.
    #[allow(dead_code)]
    struct MadeEscrow {
        svm: LiteSVM,
        maker: Keypair,
        mint_a: Pubkey,
        mint_b: Pubkey,
        maker_ata_a: Pubkey,
        escrow: Pubkey,
        bump: u8,
        vault: Pubkey,
    }

    /// Runs setup() and a full Make, so each test starts from a live escrow.
    ///
    /// This goes through `setup()` rather than building its own LiteSVM,
    /// because the Rent sysvar override lives there: without it pinocchio asks
    /// for half the lamports the runtime wants and Make fails with
    /// InsufficientFundsForRent.
    fn make_escrow() -> MadeEscrow {
        let (mut svm, maker) = setup();
        let program_id = program_id();

        let mint_a = CreateMint::new(&mut svm, &maker)
            .decimals(6).authority(&maker.pubkey()).send().unwrap();
        let mint_b = CreateMint::new(&mut svm, &maker)
            .decimals(6).authority(&maker.pubkey()).send().unwrap();

        let maker_ata_a = CreateAssociatedTokenAccount::new(&mut svm, &maker, &mint_a)
            .owner(&maker.pubkey()).send().unwrap();
        MintTo::new(&mut svm, &maker, &mint_a, &maker_ata_a, MAKER_STARTING_A)
            .send().unwrap();

        let (escrow, bump) = Pubkey::find_program_address(
            &[b"escrow".as_ref(), maker.pubkey().as_ref()],
            &program_id,
        );
        let vault = spl_associated_token_account::get_associated_token_address(&escrow, &mint_a);

        let make_data = [
            vec![0u8],
            AMOUNT_TO_RECEIVE.to_le_bytes().to_vec(),
            AMOUNT_TO_GIVE.to_le_bytes().to_vec(),
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
                AccountMeta::new(solana_sdk_ids::system_program::ID, false),
                AccountMeta::new(TOKEN_PROGRAM_ID, false),
                AccountMeta::new(ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap(), false),
            ],
            data: make_data,
        };

        let message = Message::new(&[make_ix], Some(&maker.pubkey()));
        let blockhash = svm.latest_blockhash();
        svm.send_transaction(Transaction::new(&[&maker], message, blockhash))
            .expect("make should succeed");

        MadeEscrow { svm, maker, mint_a, mint_b, maker_ata_a, escrow, bump, vault }
    }

    /// Reads an SPL token account's balance.
    fn token_balance(svm: &LiteSVM, address: &Pubkey) -> u64 {
        let acc = svm.get_account(address).expect("token account should exist");
        spl_token_2022::state::Account::unpack(&acc.data).unwrap().amount
    }

    /// The instruction error behind a failed transaction, as a string.
    ///
    /// Negative tests assert on this rather than on `is_err()` alone. A bare
    /// `is_err()` passes for any failure, so it keeps passing when the check it
    /// is supposed to be exercising is deleted and a different one catches the
    /// case instead.
    fn failure_reason<T, E: std::fmt::Debug>(result: &Result<T, E>) -> String {
        format!("{:?}", result.as_ref().err().expect("expected the transaction to fail"))
    }

    /// True when an account has been closed: gone, or drained and disowned.
    fn is_closed(svm: &LiteSVM, address: &Pubkey) -> bool {
        match svm.get_account(address) {
            None => true,
            Some(acc) => acc.lamports == 0 || acc.data.is_empty(),
        }
    }

    fn take_accounts(
        e: &MadeEscrow,
        taker: &Pubkey,
        taker_ata_a: &Pubkey,
        taker_ata_b: &Pubkey,
        maker_ata_b: &Pubkey,
        maker: &Pubkey,
    ) -> Vec<AccountMeta> {
        vec![
            AccountMeta::new(*taker, true),
            AccountMeta::new(*maker, false),
            AccountMeta::new_readonly(e.mint_a, false),
            AccountMeta::new_readonly(e.mint_b, false),
            AccountMeta::new(e.escrow, false),
            AccountMeta::new(e.vault, false),
            AccountMeta::new(*taker_ata_a, false),
            AccountMeta::new(*taker_ata_b, false),
            AccountMeta::new(*maker_ata_b, false),
            AccountMeta::new_readonly(solana_sdk_ids::system_program::ID, false),
            AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
            AccountMeta::new_readonly(ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap(), false),
        ]
    }

    fn cancel_accounts(e: &MadeEscrow, maker: &Pubkey, maker_ata_a: &Pubkey) -> Vec<AccountMeta> {
        vec![
            AccountMeta::new(*maker, true),
            AccountMeta::new_readonly(e.mint_a, false),
            AccountMeta::new(e.escrow, false),
            AccountMeta::new(e.vault, false),
            AccountMeta::new(*maker_ata_a, false),
            AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
        ]
    }

    /// Funds a taker with `amount` of mint B and returns
    /// (taker, taker_ata_a, taker_ata_b, maker_ata_b).
    ///
    /// taker_ata_a and maker_ata_b are only derived, never created — Take does
    /// that itself with CreateIdempotent, and letting the test create them
    /// would hide a bug in that step.
    fn fund_taker(e: &mut MadeEscrow, amount: u64) -> (Keypair, Pubkey, Pubkey, Pubkey) {
        let taker = Keypair::new();
        e.svm.airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut e.svm, &taker, &e.mint_b)
            .owner(&taker.pubkey()).send().unwrap();
        if amount > 0 {
            MintTo::new(&mut e.svm, &e.maker, &e.mint_b, &taker_ata_b, amount)
                .send().unwrap();
        }

        let taker_ata_a =
            spl_associated_token_account::get_associated_token_address(&taker.pubkey(), &e.mint_a);
        let maker_ata_b =
            spl_associated_token_account::get_associated_token_address(&e.maker.pubkey(), &e.mint_b);

        (taker, taker_ata_a, taker_ata_b, maker_ata_b)
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
    pub fn test_take_instruction() {
        let mut e = make_escrow();
        let (taker, taker_ata_a, taker_ata_b, maker_ata_b) = fund_taker(&mut e, AMOUNT_TO_RECEIVE);
        let maker_pk = e.maker.pubkey();
        let maker_sol_before = e.svm.get_balance(&maker_pk).unwrap();

        let ix = Instruction {
            program_id: program_id(),
            accounts: take_accounts(&e, &taker.pubkey(), &taker_ata_a, &taker_ata_b, &maker_ata_b, &maker_pk),
            data: vec![1u8], // discriminator only; the terms live in the escrow
        };
        let message = Message::new(&[ix], Some(&taker.pubkey()));
        let blockhash = e.svm.latest_blockhash();
        let tx = e.svm
            .send_transaction(Transaction::new(&[&taker], message, blockhash))
            .expect("take should succeed");

        println!("\nTake transaction successful");
        println!("CUs Consumed: {}", tx.compute_units_consumed);

        // A transaction that succeeded is not the same as one that did the
        // right thing, so read every side of the trade back.
        assert_eq!(token_balance(&e.svm, &taker_ata_a), AMOUNT_TO_GIVE, "taker should hold all of A");
        assert_eq!(token_balance(&e.svm, &maker_ata_b), AMOUNT_TO_RECEIVE, "maker should be paid in B");
        assert_eq!(token_balance(&e.svm, &taker_ata_b), 0, "taker's B should be spent");
        assert!(is_closed(&e.svm, &e.vault), "vault should be closed");
        assert!(is_closed(&e.svm, &e.escrow), "escrow should be closed");

        let maker_sol_after = e.svm.get_balance(&maker_pk).unwrap();
        assert!(
            maker_sol_after > maker_sol_before,
            "maker should get the rent back from both closed accounts: {maker_sol_before} -> {maker_sol_after}"
        );
    }

    #[test]
    pub fn test_cancel_instruction() {
        let mut e = make_escrow();
        let maker_pk = e.maker.pubkey();
        let maker_ata_a = e.maker_ata_a;

        assert_eq!(
            token_balance(&e.svm, &maker_ata_a),
            MAKER_STARTING_A - AMOUNT_TO_GIVE,
            "make should have moved A into the vault"
        );

        let ix = Instruction {
            program_id: program_id(),
            accounts: cancel_accounts(&e, &maker_pk, &maker_ata_a),
            data: vec![2u8],
        };
        let message = Message::new(&[ix], Some(&maker_pk));
        let blockhash = e.svm.latest_blockhash();
        let tx = e.svm
            .send_transaction(Transaction::new(&[&e.maker], message, blockhash))
            .expect("cancel should succeed");

        println!("\nCancel transaction successful");
        println!("CUs Consumed: {}", tx.compute_units_consumed);

        assert_eq!(token_balance(&e.svm, &maker_ata_a), MAKER_STARTING_A, "maker should have every token back");
        assert!(is_closed(&e.svm, &e.vault), "vault should be closed");
        assert!(is_closed(&e.svm, &e.escrow), "escrow should be closed");
    }

    /// A taker who cannot cover the asking price must not get the tokens.
    #[test]
    pub fn test_take_with_insufficient_b_fails() {
        let mut e = make_escrow();
        // Half of what the escrow asks for.
        let (taker, taker_ata_a, taker_ata_b, maker_ata_b) = fund_taker(&mut e, AMOUNT_TO_RECEIVE / 2);
        let maker_pk = e.maker.pubkey();

        let ix = Instruction {
            program_id: program_id(),
            accounts: take_accounts(&e, &taker.pubkey(), &taker_ata_a, &taker_ata_b, &maker_ata_b, &maker_pk),
            data: vec![1u8],
        };
        let message = Message::new(&[ix], Some(&taker.pubkey()));
        let blockhash = e.svm.latest_blockhash();
        let result = e.svm.send_transaction(Transaction::new(&[&taker], message, blockhash));

        // Custom(1) is the SPL Token program's InsufficientFunds.
        let reason = failure_reason(&result);
        assert!(
            reason.contains("Custom(1)"),
            "expected the token program to reject the underfunded transfer, got: {reason}"
        );

        // The whole transaction is atomic, so nothing at all should have moved.
        assert_eq!(token_balance(&e.svm, &e.vault), AMOUNT_TO_GIVE, "vault must be untouched");
        assert!(!is_closed(&e.svm, &e.escrow), "escrow must still be open");
    }

    /// The test that matters most: a stranger must not be able to cancel.
    #[test]
    pub fn test_cancel_by_stranger_fails() {
        let mut e = make_escrow();

        let stranger = Keypair::new();
        e.svm.airdrop(&stranger.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();
        let stranger_ata_a =
            spl_associated_token_account::get_associated_token_address(&stranger.pubkey(), &e.mint_a);

        // The stranger signs, and puts themselves in the maker slot. is_signer
        // is satisfied; the stored-maker check is what has to stop this.
        let ix = Instruction {
            program_id: program_id(),
            accounts: cancel_accounts(&e, &stranger.pubkey(), &stranger_ata_a),
            data: vec![2u8],
        };
        let message = Message::new(&[ix], Some(&stranger.pubkey()));
        let blockhash = e.svm.latest_blockhash();
        let result = e.svm.send_transaction(Transaction::new(&[&stranger], message, blockhash));

        // Specifically the stored-maker check, not some later guard. Without
        // this assertion the test still passes when that check is deleted,
        // because the PDA re-derivation catches it and returns InvalidSeeds.
        let reason = failure_reason(&result);
        assert!(
            reason.contains("InvalidAccountData"),
            "expected the stored-maker check to reject the stranger, got: {reason}"
        );

        assert_eq!(token_balance(&e.svm, &e.vault), AMOUNT_TO_GIVE, "the maker's deposit must still be in the vault");
        assert!(!is_closed(&e.svm, &e.escrow), "escrow must still be open");
    }

    /// Take with a maker account that is not the one recorded in the escrow.
    /// It must fail on the cross-check, before any tokens move.
    #[test]
    pub fn test_take_with_wrong_maker_fails() {
        let mut e = make_escrow();
        let (taker, taker_ata_a, taker_ata_b, _) = fund_taker(&mut e, AMOUNT_TO_RECEIVE);

        let impostor = Keypair::new();
        let impostor_ata_b =
            spl_associated_token_account::get_associated_token_address(&impostor.pubkey(), &e.mint_b);

        let ix = Instruction {
            program_id: program_id(),
            accounts: take_accounts(&e, &taker.pubkey(), &taker_ata_a, &taker_ata_b, &impostor_ata_b, &impostor.pubkey()),
            data: vec![1u8],
        };
        let message = Message::new(&[ix], Some(&taker.pubkey()));
        let blockhash = e.svm.latest_blockhash();
        let result = e.svm.send_transaction(Transaction::new(&[&taker], message, blockhash));

        let reason = failure_reason(&result);
        assert!(
            reason.contains("InvalidAccountData"),
            "expected the maker cross-check to fire before any tokens move, got: {reason}"
        );

        assert_eq!(token_balance(&e.svm, &e.vault), AMOUNT_TO_GIVE, "vault must be untouched");
        assert_eq!(token_balance(&e.svm, &taker_ata_b), AMOUNT_TO_RECEIVE, "taker must not have paid");
    }

    /// The real maker is passed, but never signs. Only `is_signer` stands
    /// between a bystander and pushing someone's deposit back at them.
    ///
    /// The stranger test above cannot cover this: there the caller *does* sign,
    /// so the stored-maker check is what fires.
    #[test]
    pub fn test_cancel_without_maker_signature_fails() {
        let mut e = make_escrow();
        let maker_pk = e.maker.pubkey();
        let maker_ata_a = e.maker_ata_a;

        let bystander = Keypair::new();
        e.svm.airdrop(&bystander.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

        let accounts = vec![
            AccountMeta::new(maker_pk, false), // present, but not a signer
            AccountMeta::new_readonly(e.mint_a, false),
            AccountMeta::new(e.escrow, false),
            AccountMeta::new(e.vault, false),
            AccountMeta::new(maker_ata_a, false),
            AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
        ];
        let ix = Instruction { program_id: program_id(), accounts, data: vec![2u8] };
        let message = Message::new(&[ix], Some(&bystander.pubkey()));
        let blockhash = e.svm.latest_blockhash();
        let result = e.svm.send_transaction(Transaction::new(&[&bystander], message, blockhash));

        let reason = failure_reason(&result);
        assert!(
            reason.contains("MissingRequiredSignature"),
            "expected the signer check to reject an unsigned cancel, got: {reason}"
        );
        assert_eq!(token_balance(&e.svm, &e.vault), AMOUNT_TO_GIVE, "vault must be untouched");
    }
}
