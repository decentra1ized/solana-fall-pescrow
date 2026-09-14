#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use litesvm::LiteSVM;

    use litesvm_token::{
        spl_token::{self},
        CreateAssociatedTokenAccount, CreateMint, MintTo,
    };

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

    const INITIAL_MAKER_A: u64 = 1_000_000_000;
    const AMOUNT_TO_RECEIVE: u64 = 100_000_000;
    const AMOUNT_TO_GIVE: u64 = 500_000_000;

    // ============================================================
    // Helpers
    // ============================================================

    fn program_id() -> Pubkey {
        Pubkey::from(crate::ID)
    }

    fn associated_token_program_id() -> Pubkey {
        ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap()
    }

    fn setup() -> (LiteSVM, Keypair) {
        let mut svm = LiteSVM::new();

        let payer = Keypair::new();

        // LiteSVM 0.9 uses the legacy Rent configuration.
        // Pinocchio 0.11 expects the SIMD-0194 configuration.
        #[allow(deprecated)]
        svm.set_sysvar(&solana_rent::Rent {
            lamports_per_byte_year: 6960,
            exemption_threshold: 1.0,
            burn_percent: 50,
        });

        svm.airdrop(&payer.pubkey(), 10 * LAMPORTS_PER_SOL)
            .expect("Airdrop failed");

        let so_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/deploy/escrow.so");

        let program_data = std::fs::read(&so_path).unwrap_or_else(|e| {
            panic!(
                "Failed to read program SO file at {}: {e}. \
                     Run `cargo build-sbf` first.",
                so_path.display()
            )
        });

        svm.add_program(program_id(), &program_data)
            .expect("Failed to add program");

        (svm, payer)
    }

    // ============================================================
    // Fixture
    // ============================================================

    struct EscrowFixture {
        svm: LiteSVM,

        maker: Keypair,

        mint_a: Pubkey,
        mint_b: Pubkey,

        maker_ata_a: Pubkey,

        escrow: Pubkey,
        bump: u8,
        vault: Pubkey,
    }

    // ============================================================
    // Create a complete Make escrow
    // ============================================================

    fn create_escrow() -> EscrowFixture {
        let (mut svm, maker) = setup();

        let pid = program_id();

        assert_eq!(pid.to_string(), PROGRAM_ID);

        // --------------------------------------------------------
        // Mint A
        // --------------------------------------------------------

        let mint_a = CreateMint::new(&mut svm, &maker)
            .decimals(6)
            .authority(&maker.pubkey())
            .send()
            .unwrap();

        // --------------------------------------------------------
        // Mint B
        // --------------------------------------------------------

        let mint_b = CreateMint::new(&mut svm, &maker)
            .decimals(6)
            .authority(&maker.pubkey())
            .send()
            .unwrap();

        // --------------------------------------------------------
        // Maker's Mint A ATA
        // --------------------------------------------------------

        let maker_ata_a = CreateAssociatedTokenAccount::new(&mut svm, &maker, &mint_a)
            .owner(&maker.pubkey())
            .send()
            .unwrap();

        // --------------------------------------------------------
        // Give maker 1000 A
        // --------------------------------------------------------

        MintTo::new(&mut svm, &maker, &mint_a, &maker_ata_a, INITIAL_MAKER_A)
            .send()
            .unwrap();

        // --------------------------------------------------------
        // Escrow PDA
        //
        // seeds:
        // ["escrow", maker]
        // --------------------------------------------------------

        let (escrow, bump) =
            Pubkey::find_program_address(&[b"escrow".as_ref(), maker.pubkey().as_ref()], &pid);

        // --------------------------------------------------------
        // Vault ATA
        //
        // owner = escrow PDA
        // mint  = Mint A
        // --------------------------------------------------------

        let vault = spl_associated_token_account::get_associated_token_address(&escrow, &mint_a);

        // --------------------------------------------------------
        // MAKE instruction data
        //
        // [0]
        // [amount_to_receive]
        // [amount_to_give]
        // --------------------------------------------------------

        let make_data = [
            vec![0u8],
            AMOUNT_TO_RECEIVE.to_le_bytes().to_vec(),
            AMOUNT_TO_GIVE.to_le_bytes().to_vec(),
        ]
        .concat();

        let make_ix = Instruction {
            program_id: pid,

            accounts: vec![
                AccountMeta::new(maker.pubkey(), true),
                AccountMeta::new_readonly(mint_a, false),
                AccountMeta::new_readonly(mint_b, false),
                AccountMeta::new(escrow, false),
                AccountMeta::new(maker_ata_a, false),
                AccountMeta::new(vault, false),
                AccountMeta::new_readonly(solana_sdk_ids::system_program::ID, false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
                AccountMeta::new_readonly(associated_token_program_id(), false),
            ],

            data: make_data,
        };

        let message = Message::new(&[make_ix], Some(&maker.pubkey()));

        let recent_blockhash = svm.latest_blockhash();

        let transaction = Transaction::new(&[&maker], message, recent_blockhash);

        let metadata = svm.send_transaction(transaction).unwrap();

        println!();
        println!("MAKE transaction successful");
        println!("Make CUs Consumed: {}", metadata.compute_units_consumed);

        EscrowFixture {
            svm,
            maker,
            mint_a,
            mint_b,
            maker_ata_a,
            escrow,
            bump,
            vault,
        }
    }

    // ============================================================
    // Read an SPL token account's amount
    // ============================================================

    fn token_amount(svm: &LiteSVM, token_account: &Pubkey) -> u64 {
        let account = svm
            .get_account(token_account)
            .expect("Token account does not exist");

        let token_state = spl_token_2022::state::Account::unpack(&account.data).unwrap();

        token_state.amount
    }

    // ============================================================
    // Check a closed account
    // ============================================================

    fn assert_account_closed(svm: &LiteSVM, address: &Pubkey) {
        match svm.get_account(address) {
            None => {}

            Some(account) => {
                assert_eq!(
                    account.lamports, 0,
                    "closed account should have zero lamports"
                );

                assert!(
                    account.data.is_empty(),
                    "closed account should have empty data"
                );
            }
        }
    }

    // ============================================================
    // Build Take instruction
    // ============================================================

    fn build_take_instruction(
        fixture: &EscrowFixture,
        taker: Pubkey,
        taker_ata_a: Pubkey,
        taker_ata_b: Pubkey,
        maker_ata_b: Pubkey,
    ) -> Instruction {
        Instruction {
            program_id: program_id(),

            accounts: vec![
                // 0 taker
                AccountMeta::new(taker, true),
                // 1 maker
                AccountMeta::new(fixture.maker.pubkey(), false),
                // 2 mint A
                AccountMeta::new_readonly(fixture.mint_a, false),
                // 3 mint B
                AccountMeta::new_readonly(fixture.mint_b, false),
                // 4 escrow
                AccountMeta::new(fixture.escrow, false),
                // 5 vault
                AccountMeta::new(fixture.vault, false),
                // 6 taker ATA A
                AccountMeta::new(taker_ata_a, false),
                // 7 taker ATA B
                AccountMeta::new(taker_ata_b, false),
                // 8 maker ATA B
                AccountMeta::new(maker_ata_b, false),
                // 9 system program
                AccountMeta::new_readonly(solana_sdk_ids::system_program::ID, false),
                // 10 token program
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
                // 11 associated token program
                AccountMeta::new_readonly(associated_token_program_id(), false),
            ],

            // Take discriminator
            data: vec![1u8],
        }
    }

    // ============================================================
    // TEST 1
    //
    // MAKE
    // ============================================================

    #[test]
    fn test_make_instruction() {
        let fixture = create_escrow();

        println!("Mint A: {}", fixture.mint_a);
        println!("Mint B: {}", fixture.mint_b);
        println!("Maker ATA A: {}", fixture.maker_ata_a);
        println!("Escrow PDA: {}", fixture.escrow);
        println!("Vault: {}", fixture.vault);
        println!("Bump: {}", fixture.bump);

        // --------------------------------------------------------
        // Vault = 500 A
        // --------------------------------------------------------

        assert_eq!(token_amount(&fixture.svm, &fixture.vault,), AMOUNT_TO_GIVE);

        // --------------------------------------------------------
        // Maker started with 1000 A,
        // deposited 500 A.
        // --------------------------------------------------------

        assert_eq!(
            token_amount(&fixture.svm, &fixture.maker_ata_a,),
            INITIAL_MAKER_A - AMOUNT_TO_GIVE
        );

        // --------------------------------------------------------
        // Verify escrow state
        // --------------------------------------------------------

        let escrow_account = fixture.svm.get_account(&fixture.escrow).unwrap();

        assert_eq!(escrow_account.owner, program_id());

        assert_eq!(escrow_account.data.len(), 113);

        let data = &escrow_account.data;

        assert_eq!(&data[0..32], fixture.maker.pubkey().as_ref());

        assert_eq!(
            Pubkey::new_from_array(data[32..64].try_into().unwrap()),
            fixture.mint_a
        );

        assert_eq!(
            Pubkey::new_from_array(data[64..96].try_into().unwrap()),
            fixture.mint_b
        );

        assert_eq!(
            u64::from_le_bytes(data[96..104].try_into().unwrap()),
            AMOUNT_TO_RECEIVE
        );

        assert_eq!(
            u64::from_le_bytes(data[104..112].try_into().unwrap()),
            AMOUNT_TO_GIVE
        );

        assert_eq!(data[112], fixture.bump);
    }

    // ============================================================
    // TEST 2
    //
    // TAKE SUCCESS
    // ============================================================

    #[test]
    fn test_take_instruction() {
        let mut fixture = create_escrow();

        let taker = Keypair::new();

        fixture
            .svm
            .airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL)
            .unwrap();

        // --------------------------------------------------------
        // Create taker's B account.
        // --------------------------------------------------------

        let taker_ata_b =
            CreateAssociatedTokenAccount::new(&mut fixture.svm, &taker, &fixture.mint_b)
                .owner(&taker.pubkey())
                .send()
                .unwrap();

        // --------------------------------------------------------
        // Give taker exactly 100 B.
        // --------------------------------------------------------

        MintTo::new(
            &mut fixture.svm,
            &fixture.maker,
            &fixture.mint_b,
            &taker_ata_b,
            AMOUNT_TO_RECEIVE,
        )
        .send()
        .unwrap();

        // --------------------------------------------------------
        // DO NOT create these.
        //
        // Take should create them via CreateIdempotent.
        // --------------------------------------------------------

        let taker_ata_a = spl_associated_token_account::get_associated_token_address(
            &taker.pubkey(),
            &fixture.mint_a,
        );

        let maker_ata_b = spl_associated_token_account::get_associated_token_address(
            &fixture.maker.pubkey(),
            &fixture.mint_b,
        );

        let maker_sol_before = fixture
            .svm
            .get_balance(&fixture.maker.pubkey())
            .unwrap_or(0);

        let take_ix = build_take_instruction(
            &fixture,
            taker.pubkey(),
            taker_ata_a,
            taker_ata_b,
            maker_ata_b,
        );

        let message = Message::new(&[take_ix], Some(&taker.pubkey()));

        let transaction = Transaction::new(&[&taker], message, fixture.svm.latest_blockhash());

        let metadata = fixture.svm.send_transaction(transaction).unwrap();

        println!();
        println!("TAKE transaction successful");

        println!("Take CUs Consumed: {}", metadata.compute_units_consumed);

        // --------------------------------------------------------
        // Taker received 500 A.
        // --------------------------------------------------------

        assert_eq!(token_amount(&fixture.svm, &taker_ata_a,), AMOUNT_TO_GIVE);

        // --------------------------------------------------------
        // Maker received 100 B.
        // --------------------------------------------------------

        assert_eq!(token_amount(&fixture.svm, &maker_ata_b,), AMOUNT_TO_RECEIVE);

        // --------------------------------------------------------
        // Taker paid all 100 B.
        // --------------------------------------------------------

        assert_eq!(token_amount(&fixture.svm, &taker_ata_b,), 0);

        // --------------------------------------------------------
        // Vault + escrow must be closed.
        // --------------------------------------------------------

        assert_account_closed(&fixture.svm, &fixture.vault);

        assert_account_closed(&fixture.svm, &fixture.escrow);

        // --------------------------------------------------------
        // Maker receives vault + escrow rent refund.
        //
        // Taker paid the transaction fee and ATA creation costs,
        // so maker should simply increase.
        // --------------------------------------------------------

        let maker_sol_after = fixture
            .svm
            .get_balance(&fixture.maker.pubkey())
            .unwrap_or(0);

        assert!(
            maker_sol_after > maker_sol_before,
            "maker should receive rent refunds"
        );
    }

    // ============================================================
    // TEST 3
    //
    // CANCEL SUCCESS
    // ============================================================

    #[test]
    fn test_cancel_instruction() {
        let mut fixture = create_escrow();

        let cancel_ix = Instruction {
            program_id: program_id(),

            accounts: vec![
                // 0 maker
                AccountMeta::new(fixture.maker.pubkey(), true),
                // 1 mint A
                AccountMeta::new_readonly(fixture.mint_a, false),
                // 2 escrow
                AccountMeta::new(fixture.escrow, false),
                // 3 vault
                AccountMeta::new(fixture.vault, false),
                // 4 maker ATA A
                AccountMeta::new(fixture.maker_ata_a, false),
                // 5 token program
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
            ],

            data: vec![2u8],
        };

        let message = Message::new(&[cancel_ix], Some(&fixture.maker.pubkey()));

        let transaction =
            Transaction::new(&[&fixture.maker], message, fixture.svm.latest_blockhash());

        let metadata = fixture.svm.send_transaction(transaction).unwrap();

        println!();
        println!("CANCEL transaction successful");

        println!("Cancel CUs Consumed: {}", metadata.compute_units_consumed);

        // --------------------------------------------------------
        // Maker should have all 1000 A again.
        // --------------------------------------------------------

        assert_eq!(
            token_amount(&fixture.svm, &fixture.maker_ata_a,),
            INITIAL_MAKER_A
        );

        // --------------------------------------------------------
        // Vault + escrow closed.
        // --------------------------------------------------------

        assert_account_closed(&fixture.svm, &fixture.vault);

        assert_account_closed(&fixture.svm, &fixture.escrow);
    }

    // ============================================================
    // TEST 4
    //
    // TAKE WITH ONLY 50 B MUST FAIL
    // ============================================================

    #[test]
    fn test_take_with_insufficient_b_fails() {
        let mut fixture = create_escrow();

        let taker = Keypair::new();

        fixture
            .svm
            .airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL)
            .unwrap();

        let taker_ata_b =
            CreateAssociatedTokenAccount::new(&mut fixture.svm, &taker, &fixture.mint_b)
                .owner(&taker.pubkey())
                .send()
                .unwrap();

        // --------------------------------------------------------
        // Only 50 B.
        //
        // Escrow requires 100 B.
        // --------------------------------------------------------

        let only_50_b = 50_000_000u64;

        MintTo::new(
            &mut fixture.svm,
            &fixture.maker,
            &fixture.mint_b,
            &taker_ata_b,
            only_50_b,
        )
        .send()
        .unwrap();

        let taker_ata_a = spl_associated_token_account::get_associated_token_address(
            &taker.pubkey(),
            &fixture.mint_a,
        );

        let maker_ata_b = spl_associated_token_account::get_associated_token_address(
            &fixture.maker.pubkey(),
            &fixture.mint_b,
        );

        let take_ix = build_take_instruction(
            &fixture,
            taker.pubkey(),
            taker_ata_a,
            taker_ata_b,
            maker_ata_b,
        );

        let message = Message::new(&[take_ix], Some(&taker.pubkey()));

        let transaction = Transaction::new(&[&taker], message, fixture.svm.latest_blockhash());

        let failed = fixture
            .svm
            .send_transaction(transaction)
            .expect_err("Take must fail when taker has only 50 B");

        println!();
        println!("UNDERFUNDED TAKE correctly failed");

        println!(
            "Failed Take CUs Consumed: {}",
            failed.meta.compute_units_consumed
        );

        println!("Error: {:?}", failed.err);

        // --------------------------------------------------------
        // Transaction atomicity:
        //
        // The failed transaction must not move anything.
        // --------------------------------------------------------

        assert_eq!(token_amount(&fixture.svm, &taker_ata_b,), only_50_b);

        assert_eq!(token_amount(&fixture.svm, &fixture.vault,), AMOUNT_TO_GIVE);

        assert_eq!(
            token_amount(&fixture.svm, &fixture.maker_ata_a,),
            INITIAL_MAKER_A - AMOUNT_TO_GIVE
        );

        assert!(
            fixture.svm.get_account(&fixture.escrow).is_some(),
            "escrow must remain open after failed Take"
        );
    }

    // ============================================================
    // TEST 5
    //
    // STRANGER MUST NOT CANCEL
    // ============================================================

    #[test]
    fn test_unauthorized_cancel_fails() {
        let mut fixture = create_escrow();

        let stranger = Keypair::new();

        fixture
            .svm
            .airdrop(&stranger.pubkey(), 10 * LAMPORTS_PER_SOL)
            .unwrap();

        // --------------------------------------------------------
        // We pass the REAL maker account...
        //
        // ...but deliberately mark it as NOT a signer.
        //
        // Stranger is the transaction fee payer + only signer.
        //
        // Therefore:
        //
        // maker.is_signer() == false
        //
        // and Cancel must reject it.
        // --------------------------------------------------------

        let cancel_ix = Instruction {
            program_id: program_id(),

            accounts: vec![
                AccountMeta::new(fixture.maker.pubkey(), false),
                AccountMeta::new_readonly(fixture.mint_a, false),
                AccountMeta::new(fixture.escrow, false),
                AccountMeta::new(fixture.vault, false),
                AccountMeta::new(fixture.maker_ata_a, false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
            ],

            data: vec![2u8],
        };

        let message = Message::new(&[cancel_ix], Some(&stranger.pubkey()));

        let transaction = Transaction::new(&[&stranger], message, fixture.svm.latest_blockhash());

        let failed = fixture.svm.send_transaction(transaction).expect_err(
            "a stranger must not be able to cancel \
                     someone else's escrow",
        );

        println!();
        println!("UNAUTHORIZED CANCEL correctly failed");

        println!(
            "Failed Cancel CUs Consumed: {}",
            failed.meta.compute_units_consumed
        );

        println!("Error: {:?}", failed.err);

        // --------------------------------------------------------
        // The 500 A must still be safely inside the vault.
        // --------------------------------------------------------

        assert_eq!(token_amount(&fixture.svm, &fixture.vault,), AMOUNT_TO_GIVE);

        // Maker still has only the remaining 500 A.
        assert_eq!(
            token_amount(&fixture.svm, &fixture.maker_ata_a,),
            INITIAL_MAKER_A - AMOUNT_TO_GIVE
        );

        // Escrow remains active.
        assert!(fixture.svm.get_account(&fixture.escrow).is_some());
    }
}
