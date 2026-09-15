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

    // Deal terms, shared by every test. 6 decimals throughout.
    const MAKER_START_A: u64 = 1_000_000_000; // 1,000 A
    const AMOUNT_TO_GIVE: u64 = 500_000_000;  // 500 A into the vault
    const AMOUNT_TO_RECEIVE: u64 = 100_000_000; // 100 B asked in return

    /// Everything a test needs after a successful Make.
    struct Escrowed {
        svm: LiteSVM,
        maker: Keypair,
        mint_a: Pubkey,
        mint_b: Pubkey,
        maker_ata_a: Pubkey,
        escrow: Pubkey,
        vault: Pubkey,
        // No `bump`. The guide's sketch returns one, but nothing here needs it:
        // the program re-derives its own from state, and the shipped Make test
        // already asserts the stored byte. Carrying it only to match the sketch
        // would be an unused field.
    }

    fn associated_token_program() -> Pubkey {
        ASSOCIATED_TOKEN_PROGRAM_ID.parse().unwrap()
    }

    /// Run a Make and hand back the resulting world.
    ///
    /// Built on `setup()` rather than a fresh `LiteSVM`, so the SIMD-0194 Rent
    /// override comes along. Without it every Make here dies with
    /// InsufficientFundsForRent, because pinocchio 0.11 asks for half of what
    /// LiteSVM 0.9's legacy sysvar makes the runtime require.
    fn make_escrow() -> Escrowed {
        let (mut svm, maker) = setup();
        let program_id = program_id();

        let mint_a = CreateMint::new(&mut svm, &maker).decimals(6)
            .authority(&maker.pubkey()).send().unwrap();
        let mint_b = CreateMint::new(&mut svm, &maker).decimals(6)
            .authority(&maker.pubkey()).send().unwrap();

        let maker_ata_a = CreateAssociatedTokenAccount::new(&mut svm, &maker, &mint_a)
            .owner(&maker.pubkey()).send().unwrap();
        MintTo::new(&mut svm, &maker, &mint_a, &maker_ata_a, MAKER_START_A).send().unwrap();

        let (escrow, _bump) = Pubkey::find_program_address(
            &[b"escrow".as_ref(), maker.pubkey().as_ref()], &program_id);
        let vault = spl_associated_token_account::get_associated_token_address(&escrow, &mint_a);

        let data = [
            vec![0u8],
            AMOUNT_TO_RECEIVE.to_le_bytes().to_vec(),
            AMOUNT_TO_GIVE.to_le_bytes().to_vec(),
        ].concat();

        let ix = Instruction {
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
                AccountMeta::new(associated_token_program(), false),
            ],
            data,
        };

        let message = Message::new(&[ix], Some(&maker.pubkey()));
        let blockhash = svm.latest_blockhash();
        let tx = svm.send_transaction(Transaction::new(&[&maker], message, blockhash)).unwrap();
        println!("Make CUs Consumed: {}", tx.compute_units_consumed);

        Escrowed { svm, maker, mint_a, mint_b, maker_ata_a, escrow, vault }
    }

    fn token_amount(svm: &LiteSVM, ata: &Pubkey) -> u64 {
        let acc = svm.get_account(ata).expect("token account should exist");
        spl_token_2022::state::Account::unpack(&acc.data).unwrap().amount
    }

    /// The guide allows either shape for a closed account: gone from the SVM, or
    /// present with no lamports and handed back to the system program.
    fn assert_closed(svm: &LiteSVM, address: &Pubkey, label: &str) {
        match svm.get_account(address) {
            None => {}
            Some(acc) => assert!(
                acc.lamports == 0 && acc.owner == solana_sdk_ids::system_program::ID,
                "{label} should be closed, but holds {} lamports and is owned by {}",
                acc.lamports, acc.owner
            ),
        }
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
    /// Happy path for Take: Bob pays the asking price, gets the A, everything closes.
    #[test]
    pub fn test_take_instruction() {
        let Escrowed { mut svm, maker, mint_a, mint_b, maker_ata_a, escrow, vault, .. } = make_escrow();
        let program_id = program_id();

        let taker = Keypair::new();
        svm.airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

        // The taker holds exactly the asking price in B.
        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut svm, &maker, &mint_b)
            .owner(&taker.pubkey()).send().unwrap();
        MintTo::new(&mut svm, &maker, &mint_b, &taker_ata_b, AMOUNT_TO_RECEIVE).send().unwrap();

        // Derived, deliberately NOT created: the program makes both with
        // CreateIdempotent. Asserting they are absent first is what turns the
        // balance checks below into evidence that it did.
        let taker_ata_a = spl_associated_token_account::get_associated_token_address(&taker.pubkey(), &mint_a);
        let maker_ata_b = spl_associated_token_account::get_associated_token_address(&maker.pubkey(), &mint_b);
        assert!(svm.get_account(&taker_ata_a).map_or(true, |a| a.lamports == 0),
            "taker_ata_a must not exist before Take, or this proves nothing");
        assert!(svm.get_account(&maker_ata_b).map_or(true, |a| a.lamports == 0),
            "maker_ata_b must not exist before Take, or this proves nothing");

        let maker_sol_before = svm.get_account(&maker.pubkey()).unwrap().lamports;

        let ix = Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new(taker.pubkey(), true),
                AccountMeta::new(maker.pubkey(), false),
                AccountMeta::new_readonly(mint_a, false),
                AccountMeta::new_readonly(mint_b, false),
                AccountMeta::new(escrow, false),
                AccountMeta::new(vault, false),
                AccountMeta::new(taker_ata_a, false),
                AccountMeta::new(taker_ata_b, false),
                AccountMeta::new(maker_ata_b, false),
                AccountMeta::new_readonly(solana_sdk_ids::system_program::ID, false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
                AccountMeta::new_readonly(associated_token_program(), false),
            ],
            data: vec![1u8],
        };

        let message = Message::new(&[ix], Some(&taker.pubkey()));
        let blockhash = svm.latest_blockhash();
        let tx = svm.send_transaction(Transaction::new(&[&taker], message, blockhash))
            .expect("Take should succeed");
        println!("Take CUs Consumed: {}", tx.compute_units_consumed);

        assert_eq!(token_amount(&svm, &taker_ata_a), AMOUNT_TO_GIVE, "taker should hold the 500 A");
        assert_eq!(token_amount(&svm, &maker_ata_b), AMOUNT_TO_RECEIVE, "maker should have been paid 100 B");
        assert_eq!(token_amount(&svm, &maker_ata_a), MAKER_START_A - AMOUNT_TO_GIVE,
            "the maker's A balance should not have moved since Make");
        assert_closed(&svm, &vault, "vault");
        assert_closed(&svm, &escrow, "escrow");

        // The taker paid the fees, so the maker's SOL can only have gone one way.
        let maker_sol_after = svm.get_account(&maker.pubkey()).unwrap().lamports;
        assert!(maker_sol_after > maker_sol_before,
            "the maker should have been refunded the rent of both closed accounts");
        println!("Maker rent refund: {} lamports", maker_sol_after - maker_sol_before);
    }

    /// Happy path for Cancel: Alice changes her mind and gets all of it back.
    #[test]
    pub fn test_cancel_instruction() {
        let Escrowed { mut svm, maker, mint_a, maker_ata_a, escrow, vault, .. } = make_escrow();
        let program_id = program_id();

        assert_eq!(token_amount(&svm, &maker_ata_a), MAKER_START_A - AMOUNT_TO_GIVE);

        let ix = Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new(maker.pubkey(), true),
                AccountMeta::new_readonly(mint_a, false),
                AccountMeta::new(escrow, false),
                AccountMeta::new(vault, false),
                AccountMeta::new(maker_ata_a, false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
            ],
            data: vec![2u8],
        };

        let message = Message::new(&[ix], Some(&maker.pubkey()));
        let blockhash = svm.latest_blockhash();
        let tx = svm.send_transaction(Transaction::new(&[&maker], message, blockhash))
            .expect("Cancel should succeed");
        println!("Cancel CUs Consumed: {}", tx.compute_units_consumed);

        assert_eq!(token_amount(&svm, &maker_ata_a), MAKER_START_A, "all 1,000 A should be back");
        assert_closed(&svm, &vault, "vault");
        assert_closed(&svm, &escrow, "escrow");
    }

    /// Negative: a taker who cannot cover the asking price.
    ///
    /// The token program rejects the underfunded transfer, and because the whole
    /// instruction is atomic, nothing else moves either — which is the half worth
    /// asserting. A Take that paid out the vault and then failed to collect would
    /// also "fail".
    #[test]
    pub fn test_take_fails_when_the_taker_cannot_afford_it() {
        let Escrowed { mut svm, maker, mint_a, mint_b, escrow, vault, .. } = make_escrow();
        let program_id = program_id();

        let taker = Keypair::new();
        svm.airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

        // Half the asking price.
        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut svm, &maker, &mint_b)
            .owner(&taker.pubkey()).send().unwrap();
        MintTo::new(&mut svm, &maker, &mint_b, &taker_ata_b, 50_000_000).send().unwrap();

        let taker_ata_a = spl_associated_token_account::get_associated_token_address(&taker.pubkey(), &mint_a);
        let maker_ata_b = spl_associated_token_account::get_associated_token_address(&maker.pubkey(), &mint_b);

        let ix = Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new(taker.pubkey(), true),
                AccountMeta::new(maker.pubkey(), false),
                AccountMeta::new_readonly(mint_a, false),
                AccountMeta::new_readonly(mint_b, false),
                AccountMeta::new(escrow, false),
                AccountMeta::new(vault, false),
                AccountMeta::new(taker_ata_a, false),
                AccountMeta::new(taker_ata_b, false),
                AccountMeta::new(maker_ata_b, false),
                AccountMeta::new_readonly(solana_sdk_ids::system_program::ID, false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
                AccountMeta::new_readonly(associated_token_program(), false),
            ],
            data: vec![1u8],
        };

        let message = Message::new(&[ix], Some(&taker.pubkey()));
        let blockhash = svm.latest_blockhash();
        let res = svm.send_transaction(Transaction::new(&[&taker], message, blockhash));
        let err = res.expect_err("a taker holding 50 B must not be able to take a 100 B deal");
        // A rejected transaction still burns what it ran before failing.
        println!("Underfunded Take CUs Consumed: {}", err.meta.compute_units_consumed);
        println!("Underfunded Take rejected: {:?}", err.err);

        assert_eq!(token_amount(&svm, &vault), AMOUNT_TO_GIVE, "the vault must be untouched");
        assert_eq!(token_amount(&svm, &taker_ata_b), 50_000_000, "the taker's B must be untouched");
        assert!(svm.get_account(&escrow).is_some(), "the escrow must still be open");
    }

    /// Negative, and the one that matters most: a stranger must not be able to
    /// Cancel someone else's escrow.
    ///
    /// Two attempts, and each asserts *which* error came back rather than merely
    /// that one did. That distinction was not academic here. An earlier version of
    /// this test checked only `is_err()`, and deliberately breaking Cancel showed
    /// what it was really worth:
    ///
    ///   * remove `maker.is_signer()`            → attempt 1 succeeds, test fails. Good.
    ///   * remove the stored-maker check         → test still passed.
    ///   * remove the PDA re-derivation as well  → test still passed.
    ///
    /// Both of those last two are covered by the runtime: the maker is a PDA seed,
    /// so a stranger naming themselves signs with seeds deriving a different
    /// address and the token program refuses the transfer. The test was passing on
    /// the strength of a check it was not testing. Asserting the specific error
    /// pins each layer to the layer above it, so deleting any one of the three
    /// turns this red instead of silently shifting the work onto the next one.
    #[test]
    pub fn test_cancel_by_a_stranger_fails() {
        let Escrowed { mut svm, maker, mint_a, maker_ata_a, escrow, vault, .. } = make_escrow();
        let program_id = program_id();

        let stranger = Keypair::new();
        svm.airdrop(&stranger.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();
        let stranger_ata_a = CreateAssociatedTokenAccount::new(&mut svm, &stranger, &mint_a)
            .owner(&stranger.pubkey()).send().unwrap();

        // 1 — the real maker, passed as a non-signer.
        let ix = Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new(maker.pubkey(), false),
                AccountMeta::new_readonly(mint_a, false),
                AccountMeta::new(escrow, false),
                AccountMeta::new(vault, false),
                AccountMeta::new(maker_ata_a, false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
            ],
            data: vec![2u8],
        };
        let message = Message::new(&[ix], Some(&stranger.pubkey()));
        let blockhash = svm.latest_blockhash();
        let err = svm.send_transaction(Transaction::new(&[&stranger], message, blockhash))
            .expect_err("Cancel must require the maker's own signature");
        let rendered = format!("{:?}", err.err);
        assert!(rendered.contains("MissingRequiredSignature"),
            "expected the signer check to reject this, got: {rendered}");
        println!("Unsigned-maker Cancel CUs Consumed: {}", err.meta.compute_units_consumed);
        println!("Unsigned-maker Cancel rejected: {rendered}");

        // 2 — the stranger as the maker, paying themselves.
        let ix = Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new(stranger.pubkey(), true),
                AccountMeta::new_readonly(mint_a, false),
                AccountMeta::new(escrow, false),
                AccountMeta::new(vault, false),
                AccountMeta::new(stranger_ata_a, false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
            ],
            data: vec![2u8],
        };
        let message = Message::new(&[ix], Some(&stranger.pubkey()));
        let blockhash = svm.latest_blockhash();
        let err = svm.send_transaction(Transaction::new(&[&stranger], message, blockhash))
            .expect_err("a stranger must not be able to cancel an escrow to themselves");
        let rendered = format!("{:?}", err.err);
        assert!(rendered.contains("InvalidAccountData"),
            "expected the stored-maker check to reject this first, got: {rendered}");
        println!("Stranger-as-maker Cancel CUs Consumed: {}", err.meta.compute_units_consumed);
        println!("Stranger-as-maker Cancel rejected: {rendered}");

        // Neither attempt moved anything.
        assert_eq!(token_amount(&svm, &vault), AMOUNT_TO_GIVE, "the 500 A must still be in the vault");
        assert_eq!(token_amount(&svm, &stranger_ata_a), 0, "the stranger must have gained nothing");
        assert!(svm.get_account(&escrow).is_some(), "the escrow must still be open");
    }
}
