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
    use solana_instruction::error::InstructionError;
    use solana_transaction_error::TransactionError;

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

    /// Everything `Make` leaves behind, ready for a Take or Cancel.
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

    const MAKER_STARTING_A: u64 = 1_000_000_000; // 1,000 tokens at 6 decimals
    const AMOUNT_TO_RECEIVE: u64 = 100_000_000;  // 100 token B the maker wants
    const AMOUNT_TO_GIVE: u64 = 500_000_000;     // 500 token A the maker deposits

    /// Runs a full `Make` and returns the resulting world.
    ///
    /// Factored out of `test_make_instruction` so Take and Cancel start from the same
    /// known-good state instead of each rebuilding it.
    fn setup_made_escrow() -> MadeEscrow {
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

        let (escrow, bump) = Pubkey::find_program_address(
            &[b"escrow".as_ref(), maker.pubkey().as_ref()],
            &program_id(),
        );
        let vault = spl_associated_token_account::get_associated_token_address(&escrow, &mint_a);

        MintTo::new(&mut svm, &maker, &mint_a, &maker_ata_a, MAKER_STARTING_A)
            .send()
            .unwrap();

        let make_data = [
            vec![0u8],
            AMOUNT_TO_RECEIVE.to_le_bytes().to_vec(),
            AMOUNT_TO_GIVE.to_le_bytes().to_vec(),
        ]
        .concat();

        let make_ix = Instruction {
            program_id: program_id(),
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
    fn token_balance(svm: &LiteSVM, ata: &Pubkey) -> u64 {
        let acc = svm.get_account(ata).expect("token account should exist");
        spl_token_2022::state::Account::unpack(&acc.data)
            .expect("should unpack as a token account")
            .amount
    }

    /// True when an account is gone, or exists only as an empty husk.
    ///
    /// `close()` zeroes the data length, lamports and owner; LiteSVM may still return
    /// such an account rather than `None`, so both shapes count as closed.
    fn is_closed(svm: &LiteSVM, address: &Pubkey) -> bool {
        match svm.get_account(address) {
            None => true,
            Some(acc) => acc.data.is_empty() && acc.lamports == 0,
        }
    }

    /// Builds the twelve-account `Take` instruction.
    fn build_take_ix(
        taker: &Pubkey,
        maker: &Pubkey,
        mint_a: &Pubkey,
        mint_b: &Pubkey,
        escrow: &Pubkey,
        vault: &Pubkey,
        taker_ata_a: &Pubkey,
        taker_ata_b: &Pubkey,
        maker_ata_b: &Pubkey,
    ) -> Instruction {
        Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(*taker, true),
                AccountMeta::new(*maker, false),
                AccountMeta::new(*mint_a, false),
                AccountMeta::new(*mint_b, false),
                AccountMeta::new(*escrow, false),
                AccountMeta::new(*vault, false),
                AccountMeta::new(*taker_ata_a, false),
                AccountMeta::new(*taker_ata_b, false),
                AccountMeta::new(*maker_ata_b, false),
                AccountMeta::new(solana_sdk_ids::system_program::ID, false),
                AccountMeta::new(TOKEN_PROGRAM_ID, false),
                AccountMeta::new(ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap(), false),
            ],
            data: vec![1u8],
        }
    }

    /// Builds the six-account `Cancel` instruction.
    fn build_cancel_ix(
        maker: &Pubkey,
        mint_a: &Pubkey,
        escrow: &Pubkey,
        vault: &Pubkey,
        maker_ata_a: &Pubkey,
    ) -> Instruction {
        Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(*maker, true),
                AccountMeta::new(*mint_a, false),
                AccountMeta::new(*escrow, false),
                AccountMeta::new(*vault, false),
                AccountMeta::new(*maker_ata_a, false),
                AccountMeta::new(TOKEN_PROGRAM_ID, false),
            ],
            data: vec![2u8],
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

    // ---------- Challenge 3: Take and Cancel ----------

    #[test]
    pub fn test_take_instruction() {
        let MadeEscrow { mut svm, maker, mint_a, mint_b, escrow, vault, .. } = setup_made_escrow();

        let taker = Keypair::new();
        svm.airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

        // The taker holds exactly what the maker asked for.
        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut svm, &taker, &mint_b)
            .owner(&taker.pubkey())
            .send()
            .unwrap();
        MintTo::new(&mut svm, &maker, &mint_b, &taker_ata_b, AMOUNT_TO_RECEIVE)
            .send()
            .unwrap();

        // Derived but deliberately not created: the program must make them itself with
        // CreateIdempotent. If it did not, this test would fail on a missing account.
        let taker_ata_a =
            spl_associated_token_account::get_associated_token_address(&taker.pubkey(), &mint_a);
        let maker_ata_b =
            spl_associated_token_account::get_associated_token_address(&maker.pubkey(), &mint_b);
        assert!(svm.get_account(&taker_ata_a).is_none_or(|a| a.data.is_empty()),
            "taker_ata_a must not exist before Take");
        assert!(svm.get_account(&maker_ata_b).is_none_or(|a| a.data.is_empty()),
            "maker_ata_b must not exist before Take");

        let maker_sol_before = svm.get_account(&maker.pubkey()).unwrap().lamports;

        let take_ix = build_take_ix(
            &taker.pubkey(), &maker.pubkey(), &mint_a, &mint_b,
            &escrow, &vault, &taker_ata_a, &taker_ata_b, &maker_ata_b,
        );
        let message = Message::new(&[take_ix], Some(&taker.pubkey()));
        let blockhash = svm.latest_blockhash();
        let tx = svm
            .send_transaction(Transaction::new(&[&taker], message, blockhash))
            .expect("take should succeed");

        println!("\n\nTake transaction successful");
        println!("CUs Consumed: {}", tx.compute_units_consumed);

        // Both sides got what they were promised.
        assert_eq!(token_balance(&svm, &taker_ata_a), AMOUNT_TO_GIVE,
            "taker should receive the vault's token A");
        assert_eq!(token_balance(&svm, &maker_ata_b), AMOUNT_TO_RECEIVE,
            "maker should receive the taker's token B");

        // And the escrow is fully torn down.
        assert!(is_closed(&svm, &vault), "vault should be closed");
        assert!(is_closed(&svm, &escrow), "escrow should be closed");

        // The maker gets the rent back from both accounts.
        let maker_sol_after = svm.get_account(&maker.pubkey()).unwrap().lamports;
        assert!(maker_sol_after > maker_sol_before,
            "maker should be refunded the vault and escrow rent (before {maker_sol_before}, after {maker_sol_after})");
    }

    #[test]
    pub fn test_cancel_instruction() {
        let MadeEscrow { mut svm, maker, mint_a, maker_ata_a, escrow, bump, vault, .. } =
            setup_made_escrow();

        // The bump Make stored is the canonical one. Cancel re-derives the PDA from it
        // with a single hash instead of searching, so a wrong byte here would surface
        // as InvalidSeeds rather than a successful cancel.
        let stored_bump = svm.get_account(&escrow).unwrap().data[112];
        assert_eq!(stored_bump, bump, "escrow should store the canonical bump");

        // Precondition: Make moved the deposit into the vault.
        assert_eq!(token_balance(&svm, &vault), AMOUNT_TO_GIVE);
        assert_eq!(token_balance(&svm, &maker_ata_a), MAKER_STARTING_A - AMOUNT_TO_GIVE);

        let cancel_ix = build_cancel_ix(&maker.pubkey(), &mint_a, &escrow, &vault, &maker_ata_a);
        let message = Message::new(&[cancel_ix], Some(&maker.pubkey()));
        let blockhash = svm.latest_blockhash();
        let tx = svm
            .send_transaction(Transaction::new(&[&maker], message, blockhash))
            .expect("cancel should succeed");

        println!("\n\nCancel transaction successful");
        println!("CUs Consumed: {}", tx.compute_units_consumed);

        // Every token came home.
        assert_eq!(token_balance(&svm, &maker_ata_a), MAKER_STARTING_A,
            "maker should have their full balance back");
        assert!(is_closed(&svm, &vault), "vault should be closed");
        assert!(is_closed(&svm, &escrow), "escrow should be closed");
    }

    #[test]
    pub fn test_take_fails_with_insufficient_token_b() {
        let MadeEscrow { mut svm, maker, mint_a, mint_b, escrow, vault, .. } = setup_made_escrow();

        let taker = Keypair::new();
        svm.airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

        // Half of what the maker asked for.
        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut svm, &taker, &mint_b)
            .owner(&taker.pubkey())
            .send()
            .unwrap();
        MintTo::new(&mut svm, &maker, &mint_b, &taker_ata_b, AMOUNT_TO_RECEIVE / 2)
            .send()
            .unwrap();

        let taker_ata_a =
            spl_associated_token_account::get_associated_token_address(&taker.pubkey(), &mint_a);
        let maker_ata_b =
            spl_associated_token_account::get_associated_token_address(&maker.pubkey(), &mint_b);

        let take_ix = build_take_ix(
            &taker.pubkey(), &maker.pubkey(), &mint_a, &mint_b,
            &escrow, &vault, &taker_ata_a, &taker_ata_b, &maker_ata_b,
        );
        let message = Message::new(&[take_ix], Some(&taker.pubkey()));
        let blockhash = svm.latest_blockhash();
        let res = svm.send_transaction(Transaction::new(&[&taker], message, blockhash));

        // Custom(1) is SPL Token's InsufficientFunds: the taker's payment leg is what
        // failed, which is precisely the scenario under test.
        let err = res.expect_err("take must fail when the taker cannot pay in full");
        assert_eq!(
            err.err,
            TransactionError::InstructionError(0, InstructionError::Custom(1)),
            "expected the token program to reject the underfunded transfer, got {:?}", err.err,
        );
        println!("\n\nTake with insufficient B rejected: {:?}", err.err);
        println!("CUs Consumed: {}", err.meta.compute_units_consumed);

        // The whole transaction rolled back, so the maker's deposit is untouched.
        assert_eq!(token_balance(&svm, &vault), AMOUNT_TO_GIVE,
            "vault must still hold the full deposit after a failed take");
        assert!(!is_closed(&svm, &escrow), "escrow must still be open");
    }

    #[test]
    pub fn test_cancel_by_stranger_fails() {
        let MadeEscrow { mut svm, maker, mint_a, maker_ata_a, escrow, vault, .. } =
            setup_made_escrow();

        // Somebody who is not the maker, signing for themselves.
        let stranger = Keypair::new();
        svm.airdrop(&stranger.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

        // Note the escrow and vault are the *maker's*; only the signer differs. This is
        // the theft attempt the maker check in cancel.rs exists to stop.
        let cancel_ix = build_cancel_ix(&stranger.pubkey(), &mint_a, &escrow, &vault, &maker_ata_a);
        let message = Message::new(&[cancel_ix], Some(&stranger.pubkey()));
        let blockhash = svm.latest_blockhash();
        let res = svm.send_transaction(Transaction::new(&[&stranger], message, blockhash));

        // Not merely "it failed": IllegalOwner is the maker check in cancel.rs firing.
        // A malformed instruction would also fail, and a bare is_err() would accept that
        // as proof the guard works.
        let err = res.expect_err("a stranger must not be able to cancel someone else's escrow");
        assert_eq!(
            err.err,
            TransactionError::InstructionError(0, InstructionError::IllegalOwner),
            "expected the maker check to reject the stranger, got {:?}", err.err,
        );
        println!("\n\nCancel by a stranger rejected: {:?}", err.err);
        println!("CUs Consumed: {}", err.meta.compute_units_consumed);

        assert_eq!(token_balance(&svm, &vault), AMOUNT_TO_GIVE,
            "vault balance must be unchanged after a rejected cancel");
        assert!(!is_closed(&svm, &escrow), "escrow must still be open");
        let _ = maker;
    }

    /// Bonus: Take with a maker account that is not the one in the escrow.
    ///
    /// Without the stored-maker check the program would happily send the taker's token B
    /// to an attacker-chosen address.
    #[test]
    pub fn test_take_with_mismatched_maker_fails() {
        let MadeEscrow { mut svm, maker, mint_a, mint_b, escrow, vault, .. } = setup_made_escrow();

        let taker = Keypair::new();
        svm.airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();
        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut svm, &taker, &mint_b)
            .owner(&taker.pubkey())
            .send()
            .unwrap();
        MintTo::new(&mut svm, &maker, &mint_b, &taker_ata_b, AMOUNT_TO_RECEIVE)
            .send()
            .unwrap();

        let impostor = Keypair::new();
        svm.airdrop(&impostor.pubkey(), LAMPORTS_PER_SOL).unwrap();

        let taker_ata_a =
            spl_associated_token_account::get_associated_token_address(&taker.pubkey(), &mint_a);
        let impostor_ata_b =
            spl_associated_token_account::get_associated_token_address(&impostor.pubkey(), &mint_b);

        let take_ix = build_take_ix(
            &taker.pubkey(), &impostor.pubkey(), &mint_a, &mint_b,
            &escrow, &vault, &taker_ata_a, &taker_ata_b, &impostor_ata_b,
        );
        let message = Message::new(&[take_ix], Some(&taker.pubkey()));
        let blockhash = svm.latest_blockhash();
        let res = svm.send_transaction(Transaction::new(&[&taker], message, blockhash));

        // InvalidAccountData is the stored-maker comparison in take.rs.
        let err = res.expect_err("take must reject a maker that does not match the escrow");
        assert_eq!(
            err.err,
            TransactionError::InstructionError(0, InstructionError::InvalidAccountData),
            "expected the stored-maker check to reject the impostor, got {:?}", err.err,
        );
        println!("\n\nTake with mismatched maker rejected: {:?}", err.err);
        println!("CUs Consumed: {}", err.meta.compute_units_consumed);

        assert_eq!(token_balance(&svm, &vault), AMOUNT_TO_GIVE, "vault must be untouched");
        assert_eq!(token_balance(&svm, &taker_ata_b), AMOUNT_TO_RECEIVE,
            "taker must not have paid anything");
    }
}
