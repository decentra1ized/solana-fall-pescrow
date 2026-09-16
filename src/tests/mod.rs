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
    use solana_transaction::{InstructionError, Transaction, TransactionError};

    const PROGRAM_ID: &str = "4ibrEMW5F6hKnkW4jVedswYv6H6VtwPN6ar6dvXDN1nT";
    const TOKEN_PROGRAM_ID: Pubkey = spl_token::ID;
    const ASSOCIATED_TOKEN_PROGRAM_ID: &str = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL";

    fn program_id() -> Pubkey {
        Pubkey::from(crate::ID)
    }

    /// The terms every test uses: the maker deposits 500 A and asks 100 B for it.
    const AMOUNT_TO_RECEIVE: u64 = 100_000_000; // 100 B, 6 decimals
    const AMOUNT_TO_GIVE: u64 = 500_000_000; // 500 A, 6 decimals
    const MAKER_START_A: u64 = 1_000_000_000; // 1,000 A
    const TAKER_START_B: u64 = 100_000_000; // 100 B

    fn system_program() -> Pubkey {
        solana_sdk_ids::system_program::ID
    }

    fn associated_token_program() -> Pubkey {
        ASSOCIATED_TOKEN_PROGRAM_ID.parse().unwrap()
    }

    /// Balance of an SPL token account.
    fn token_balance(svm: &LiteSVM, ata: &Pubkey) -> u64 {
        let account = svm
            .get_account(ata)
            .unwrap_or_else(|| panic!("token account {ata} does not exist"));
        spl_token_2022::state::Account::unpack(&account.data)
            .unwrap()
            .amount
    }

    /// Lamports held by an address. A closed account reads as 0 whether the VM drops it
    /// or keeps a zeroed record, so this works for both.
    fn lamports_of(svm: &LiteSVM, key: &Pubkey) -> u64 {
        svm.get_account(key).map(|a| a.lamports).unwrap_or(0)
    }

    /// The state a test starts from: `Make` has run, so 500 A sit in the vault.
    struct Escrowed {
        svm: LiteSVM,
        maker: Keypair,
        mint_a: Pubkey,
        mint_b: Pubkey,
        maker_ata_a: Pubkey,
        escrow: Pubkey,
        vault: Pubkey,
        /// Canonical bump for the escrow PDA. Unused by the happy-path tests, which never
        /// re-derive the address, but a negative test that tampers with the seeds will want it.
        #[allow(dead_code)]
        bump: u8,
    }

    /// Mint A and B, fund the maker with 1,000 A, then run `Make`.
    ///
    /// Every Take and Cancel test begins here, so the interesting part of each test is
    /// just the one instruction it sends and what it asserts afterwards.
    fn make_escrow() -> Escrowed {
        let (mut svm, maker) = setup();

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

        let maker_ata_a = CreateAssociatedTokenAccount::new(&mut svm, &maker, &mint_a)
            .owner(&maker.pubkey())
            .send()
            .unwrap();
        MintTo::new(&mut svm, &maker, &mint_a, &maker_ata_a, MAKER_START_A)
            .send()
            .unwrap();

        let (escrow, bump) = Pubkey::find_program_address(
            &[b"escrow".as_ref(), maker.pubkey().as_ref()],
            &program_id(),
        );
        let vault = spl_associated_token_account::get_associated_token_address(&escrow, &mint_a);

        let data = [
            vec![0u8], // Make
            AMOUNT_TO_RECEIVE.to_le_bytes().to_vec(),
            AMOUNT_TO_GIVE.to_le_bytes().to_vec(),
        ]
        .concat();

        let make_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(maker.pubkey(), true),
                AccountMeta::new_readonly(mint_a, false),
                AccountMeta::new_readonly(mint_b, false),
                AccountMeta::new(escrow, false),
                AccountMeta::new(maker_ata_a, false),
                AccountMeta::new(vault, false),
                AccountMeta::new_readonly(system_program(), false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
                AccountMeta::new_readonly(associated_token_program(), false),
            ],
            data,
        };

        let message = Message::new(&[make_ix], Some(&maker.pubkey()));
        let blockhash = svm.latest_blockhash();
        svm.send_transaction(Transaction::new(&[&maker], message, blockhash))
            .expect("Make failed while setting the fixture up");

        Escrowed {
            svm,
            maker,
            mint_a,
            mint_b,
            maker_ata_a,
            escrow,
            vault,
            bump,
        }
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
        let so_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/deploy/escrow.so");

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
            &escrow.0, // owner will be the escrow PDA
            &mint_a,   // mint
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
        let amount_to_give: u64 = 500000000; // 500 tokens with 6 decimal places
        let bump: u8 = escrow.1; // canonical bump; the program derives the same one on-chain

        println!("Bump: {}", bump);

        // Create the "Make" instruction to deposit tokens into the escrow
        let make_data = [
            vec![0u8], // Discriminator for "Make" instruction
            amount_to_receive.to_le_bytes().to_vec(),
            amount_to_give.to_le_bytes().to_vec(),
        ]
        .concat();
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
        println!(
            "Vault owner: {} (escrow PDA? {})",
            vault_state.owner,
            vault_state.owner == escrow.0
        );
        println!("Vault balance: {}", vault_state.amount);
        assert_eq!(vault_state.amount, amount_to_give);

        let maker_acc = svm.get_account(&maker_ata_a).unwrap();
        let maker_state = spl_token_2022::state::Account::unpack(&maker_acc.data).unwrap();
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

    /// Take, the happy path: the taker pays 100 B and walks away with the vault's 500 A,
    /// and both PDA accounts are gone afterwards.
    #[test]
    pub fn test_take_instruction() {
        let mut escrowed = make_escrow();

        // A taker holding exactly the asking price in B, and nothing else.
        let taker = Keypair::new();
        escrowed
            .svm
            .airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL)
            .expect("Airdrop failed");

        let taker_ata_b =
            CreateAssociatedTokenAccount::new(&mut escrowed.svm, &taker, &escrowed.mint_b)
                .owner(&taker.pubkey())
                .send()
                .unwrap();

        // The mint authority for B is the maker, set in make_escrow(), so the maker has to
        // sign this. The taker only funded its own ATA above.
        MintTo::new(
            &mut escrowed.svm,
            &escrowed.maker,
            &escrowed.mint_b,
            &taker_ata_b,
            TAKER_START_B,
        )
        .send()
        .unwrap();

        // Derived but deliberately not created: Take has to create both through
        // CreateIdempotent, funded by the taker, because the maker never signs Take.
        let taker_ata_a = spl_associated_token_account::get_associated_token_address(
            &taker.pubkey(),
            &escrowed.mint_a,
        );
        let maker_ata_b = spl_associated_token_account::get_associated_token_address(
            &escrowed.maker.pubkey(),
            &escrowed.mint_b,
        );
        assert_eq!(
            escrowed.svm.get_account(&taker_ata_a),
            None,
            "taker_ata_a must not exist yet, or CreateIdempotent is never exercised"
        );
        assert_eq!(
            escrowed.svm.get_account(&maker_ata_b),
            None,
            "maker_ata_b must not exist yet, or CreateIdempotent is never exercised"
        );

        // Account order must match the table at the top of src/instructions/take.rs.
        let take_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(taker.pubkey(), true),
                // Writable, since it receives the rent refunds, but not a signer: the maker
                // is not present for Take. That is why the taker funds both ATAs.
                AccountMeta::new(escrowed.maker.pubkey(), false),
                AccountMeta::new_readonly(escrowed.mint_a, false),
                AccountMeta::new_readonly(escrowed.mint_b, false),
                AccountMeta::new(escrowed.escrow, false),
                AccountMeta::new(escrowed.vault, false),
                AccountMeta::new(taker_ata_a, false),
                AccountMeta::new(taker_ata_b, false),
                AccountMeta::new(maker_ata_b, false),
                AccountMeta::new_readonly(system_program(), false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
                AccountMeta::new_readonly(associated_token_program(), false),
            ],
            data: vec![1u8], // Take
        };

        // Read the rent off the accounts rather than hardcoding it, so this keeps holding
        // if the rent parameters change.
        let maker_before = lamports_of(&escrowed.svm, &escrowed.maker.pubkey());
        let rent_to_refund = lamports_of(&escrowed.svm, &escrowed.vault)
            + lamports_of(&escrowed.svm, &escrowed.escrow);

        let message = Message::new(&[take_ix], Some(&taker.pubkey()));
        let blockhash = escrowed.svm.latest_blockhash();
        let tx = escrowed
            .svm
            .send_transaction(Transaction::new(&[&taker], message, blockhash))
            .expect("Take should succeed");

        println!("\n\nTake transaction successful");
        println!("CUs Consumed: {}", tx.compute_units_consumed);

        // The swap happened in both directions.
        assert_eq!(
            token_balance(&escrowed.svm, &taker_ata_a),
            AMOUNT_TO_GIVE,
            "the taker should hold everything the vault held"
        );
        assert_eq!(
            token_balance(&escrowed.svm, &maker_ata_b),
            AMOUNT_TO_RECEIVE,
            "the maker should hold the asking price"
        );
        assert_eq!(
            token_balance(&escrowed.svm, &taker_ata_b),
            TAKER_START_B - AMOUNT_TO_RECEIVE,
            "the taker should have paid out of their own B"
        );
        assert_eq!(
            token_balance(&escrowed.svm, &escrowed.maker_ata_a),
            MAKER_START_A - AMOUNT_TO_GIVE,
            "the maker's A is untouched by Take"
        );

        // Both PDA accounts are gone.
        assert_eq!(escrowed.svm.get_account(&escrowed.vault), None);
        assert_eq!(escrowed.svm.get_account(&escrowed.escrow), None);

        // The maker pays no fee here (the taker is the fee payer), so the increase is the
        // refunded rent exactly, not approximately.
        let maker_after = lamports_of(&escrowed.svm, &escrowed.maker.pubkey());
        println!("Maker SOL: {maker_before} -> {maker_after} (+{rent_to_refund} rent)");
        assert_eq!(
            maker_after,
            maker_before + rent_to_refund,
            "the maker should get the vault and escrow rent back"
        );

        // The taker funded both ATAs and paid the fee, so their balance can only have dropped.
        assert!(
            lamports_of(&escrowed.svm, &taker.pubkey()) < 10 * LAMPORTS_PER_SOL,
            "the taker pays the ATA rent and the transaction fee"
        );
    }

    /// Cancel, the happy path: the maker takes the 500 A back and both PDA accounts go away.
    #[test]
    pub fn test_cancel_instruction() {
        let mut escrowed = make_escrow();

        // Make moved 500 A out of the maker and into the vault.
        assert_eq!(
            token_balance(&escrowed.svm, &escrowed.maker_ata_a),
            MAKER_START_A - AMOUNT_TO_GIVE
        );
        assert_eq!(token_balance(&escrowed.svm, &escrowed.vault), AMOUNT_TO_GIVE);

        // Account order must match the table at the top of src/instructions/cancel.rs.
        // Exactly six: the handler matches without a trailing rest pattern.
        let cancel_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(escrowed.maker.pubkey(), true),
                AccountMeta::new_readonly(escrowed.mint_a, false),
                AccountMeta::new(escrowed.escrow, false),
                AccountMeta::new(escrowed.vault, false),
                AccountMeta::new(escrowed.maker_ata_a, false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
            ],
            data: vec![2u8], // Cancel
        };

        let maker_before = lamports_of(&escrowed.svm, &escrowed.maker.pubkey());
        let rent_to_refund = lamports_of(&escrowed.svm, &escrowed.vault)
            + lamports_of(&escrowed.svm, &escrowed.escrow);

        let message = Message::new(&[cancel_ix], Some(&escrowed.maker.pubkey()));
        let blockhash = escrowed.svm.latest_blockhash();
        let tx = escrowed
            .svm
            .send_transaction(Transaction::new(&[&escrowed.maker], message, blockhash))
            .expect("Cancel should succeed");

        println!("\n\nCancel transaction successful");
        println!("CUs Consumed: {}", tx.compute_units_consumed);

        // Every A is back where it started.
        assert_eq!(
            token_balance(&escrowed.svm, &escrowed.maker_ata_a),
            MAKER_START_A,
            "the deposit should be back in full"
        );

        assert_eq!(escrowed.svm.get_account(&escrowed.vault), None);
        assert_eq!(escrowed.svm.get_account(&escrowed.escrow), None);

        // Unlike Take, the maker is the fee payer here, so the rent comes back minus the fee.
        let maker_after = lamports_of(&escrowed.svm, &escrowed.maker.pubkey());
        println!("Maker SOL: {maker_before} -> {maker_after} (+{rent_to_refund} rent - fee)");
        assert_eq!(
            maker_after,
            maker_before + rent_to_refund - tx.fee,
            "the maker should get the rent back, less the fee they paid to send Cancel"
        );
    }

    /// A taker who cannot cover the asking price must not get the vault, and nothing at all
    /// may move: the whole transaction has to roll back, ATAs included.
    #[test]
    pub fn test_take_fails_when_taker_is_underfunded() {
        let mut escrowed = make_escrow();

        let taker = Keypair::new();
        escrowed
            .svm
            .airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL)
            .expect("Airdrop failed");

        let taker_ata_b =
            CreateAssociatedTokenAccount::new(&mut escrowed.svm, &taker, &escrowed.mint_b)
                .owner(&taker.pubkey())
                .send()
                .unwrap();

        // Half of what the escrow asks for.
        let underfunded = AMOUNT_TO_RECEIVE / 2;
        MintTo::new(
            &mut escrowed.svm,
            &escrowed.maker,
            &escrowed.mint_b,
            &taker_ata_b,
            underfunded,
        )
        .send()
        .unwrap();

        let taker_ata_a = spl_associated_token_account::get_associated_token_address(
            &taker.pubkey(),
            &escrowed.mint_a,
        );
        let maker_ata_b = spl_associated_token_account::get_associated_token_address(
            &escrowed.maker.pubkey(),
            &escrowed.mint_b,
        );

        let take_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(taker.pubkey(), true),
                AccountMeta::new(escrowed.maker.pubkey(), false),
                AccountMeta::new_readonly(escrowed.mint_a, false),
                AccountMeta::new_readonly(escrowed.mint_b, false),
                AccountMeta::new(escrowed.escrow, false),
                AccountMeta::new(escrowed.vault, false),
                AccountMeta::new(taker_ata_a, false),
                AccountMeta::new(taker_ata_b, false),
                AccountMeta::new(maker_ata_b, false),
                AccountMeta::new_readonly(system_program(), false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
                AccountMeta::new_readonly(associated_token_program(), false),
            ],
            data: vec![1u8],
        };

        let message = Message::new(&[take_ix], Some(&taker.pubkey()));
        let blockhash = escrowed.svm.latest_blockhash();
        let failure = escrowed
            .svm
            .send_transaction(Transaction::new(&[&taker], message, blockhash))
            .expect_err("a taker who cannot pay must not be able to take the escrow");

        println!("\n\nUnderfunded Take rejected: {:?}", failure.err);
        println!("CUs Consumed: {}", failure.meta.compute_units_consumed);

        // The token program rejects the transfer with its own InsufficientFunds (code 1).
        // Asserting the code, not merely "it failed", keeps the test honest: a different
        // failure would mean the escrow was rejected for the wrong reason.
        assert_eq!(
            failure.err,
            TransactionError::InstructionError(0, InstructionError::Custom(1)),
            "expected the token program's InsufficientFunds"
        );

        // Nothing moved. The transaction is atomic, so even the ATAs Take would have
        // created are still absent.
        assert_eq!(
            token_balance(&escrowed.svm, &escrowed.vault),
            AMOUNT_TO_GIVE,
            "the vault must still hold the deposit"
        );
        assert_eq!(
            token_balance(&escrowed.svm, &taker_ata_b),
            underfunded,
            "the taker's B must be untouched"
        );
        assert!(
            escrowed.svm.get_account(&escrowed.escrow).is_some(),
            "the escrow account must still be open"
        );
        assert_eq!(
            escrowed.svm.get_account(&taker_ata_a),
            None,
            "the rollback must undo the ATA creation too"
        );
        assert_eq!(escrowed.svm.get_account(&maker_ata_b), None);
    }

    /// The test that matters most: nobody but the maker may cancel an escrow.
    ///
    /// The stranger signs, so `maker.is_signer()` passes. What stops them is the stored-maker
    /// check, and the assertion below pins that specific error. Asserting only `is_err()`
    /// would stay green even if that check were deleted, because the transfer would then fail
    /// for an unrelated reason.
    #[test]
    pub fn test_cancel_by_a_stranger_is_rejected() {
        let mut escrowed = make_escrow();

        let stranger = Keypair::new();
        escrowed
            .svm
            .airdrop(&stranger.pubkey(), 10 * LAMPORTS_PER_SOL)
            .expect("Airdrop failed");

        // The stranger wants the A sent somewhere they control.
        let stranger_ata_a =
            CreateAssociatedTokenAccount::new(&mut escrowed.svm, &stranger, &escrowed.mint_a)
                .owner(&stranger.pubkey())
                .send()
                .unwrap();

        let cancel_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                // Position 0 is the stranger, and they really do sign.
                AccountMeta::new(stranger.pubkey(), true),
                AccountMeta::new_readonly(escrowed.mint_a, false),
                AccountMeta::new(escrowed.escrow, false),
                AccountMeta::new(escrowed.vault, false),
                AccountMeta::new(stranger_ata_a, false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
            ],
            data: vec![2u8],
        };

        let message = Message::new(&[cancel_ix], Some(&stranger.pubkey()));
        let blockhash = escrowed.svm.latest_blockhash();
        let failure = escrowed
            .svm
            .send_transaction(Transaction::new(&[&stranger], message, blockhash))
            .expect_err("a stranger must not be able to cancel someone else's escrow");

        println!("\n\nStranger's Cancel rejected: {:?}", failure.err);
        println!("CUs Consumed: {}", failure.meta.compute_units_consumed);

        // escrow.maker() != stranger, so Cancel bails out with InvalidAccountData before
        // touching a single token.
        assert_eq!(
            failure.err,
            TransactionError::InstructionError(0, InstructionError::InvalidAccountData),
            "Cancel must reject on the stored-maker check, not incidentally"
        );

        // The A never left.
        assert_eq!(
            token_balance(&escrowed.svm, &escrowed.vault),
            AMOUNT_TO_GIVE,
            "the deposit must still be in the vault"
        );
        assert_eq!(
            token_balance(&escrowed.svm, &stranger_ata_a),
            0,
            "the stranger must have received nothing"
        );
        assert!(
            escrowed.svm.get_account(&escrowed.escrow).is_some(),
            "the escrow account must still be open"
        );

        // And the real maker can still cancel afterwards.
        let cancel_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(escrowed.maker.pubkey(), true),
                AccountMeta::new_readonly(escrowed.mint_a, false),
                AccountMeta::new(escrowed.escrow, false),
                AccountMeta::new(escrowed.vault, false),
                AccountMeta::new(escrowed.maker_ata_a, false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
            ],
            data: vec![2u8],
        };
        let message = Message::new(&[cancel_ix], Some(&escrowed.maker.pubkey()));
        let blockhash = escrowed.svm.latest_blockhash();
        escrowed
            .svm
            .send_transaction(Transaction::new(&[&escrowed.maker], message, blockhash))
            .expect("the real maker should still be able to cancel");
        assert_eq!(
            token_balance(&escrowed.svm, &escrowed.maker_ata_a),
            MAKER_START_A
        );
    }
}
