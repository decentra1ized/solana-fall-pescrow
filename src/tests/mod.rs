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

    // ---------------------------------------------------------------------------------
    // Challenge 3: Take and Cancel
    // ---------------------------------------------------------------------------------

    const AMOUNT_TO_RECEIVE: u64 = 100_000_000; // 100 B
    const AMOUNT_TO_GIVE: u64 = 500_000_000; // 500 A
    const MAKER_START_A: u64 = 1_000_000_000; // 1,000 A

    /// Everything a test needs after a successful Make.
    struct Made {
        svm: LiteSVM,
        maker: Keypair,
        mint_a: Pubkey,
        mint_b: Pubkey,
        escrow: Pubkey,
        bump: u8,
        vault: Pubkey,
        maker_ata_a: Pubkey,
    }

    fn token_amount(svm: &LiteSVM, ata: &Pubkey) -> u64 {
        let acc = svm.get_account(ata).expect("token account missing");
        spl_token_2022::state::Account::unpack(&acc.data).unwrap().amount
    }

    /// Closed accounts either disappear from LiteSVM or linger as 0-lamport system accounts.
    fn assert_closed(svm: &LiteSVM, key: &Pubkey, what: &str) {
        match svm.get_account(key) {
            None => {}
            Some(acc) => {
                assert_eq!(acc.lamports, 0, "{what} still holds lamports");
                assert_eq!(acc.owner, solana_sdk_ids::system_program::ID, "{what} not returned to system");
            }
        }
    }

    /// Two mints, a funded maker ATA, and a successful Make. Reuses `setup()`, so the
    /// Rent sysvar override is kept.
    fn setup_make() -> Made {
        let (mut svm, maker) = setup();

        let mint_a = CreateMint::new(&mut svm, &maker).decimals(6).authority(&maker.pubkey()).send().unwrap();
        let mint_b = CreateMint::new(&mut svm, &maker).decimals(6).authority(&maker.pubkey()).send().unwrap();

        let maker_ata_a = CreateAssociatedTokenAccount::new(&mut svm, &maker, &mint_a)
            .owner(&maker.pubkey()).send().unwrap();
        MintTo::new(&mut svm, &maker, &mint_a, &maker_ata_a, MAKER_START_A).send().unwrap();

        let (escrow, bump) = Pubkey::find_program_address(&[b"escrow".as_ref(), maker.pubkey().as_ref()], &program_id());
        let vault = spl_associated_token_account::get_associated_token_address(&escrow, &mint_a);

        let data = [vec![0u8], AMOUNT_TO_RECEIVE.to_le_bytes().to_vec(), AMOUNT_TO_GIVE.to_le_bytes().to_vec()].concat();
        let ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(maker.pubkey(), true),
                AccountMeta::new_readonly(mint_a, false),
                AccountMeta::new_readonly(mint_b, false),
                AccountMeta::new(escrow, false),
                AccountMeta::new(maker_ata_a, false),
                AccountMeta::new(vault, false),
                AccountMeta::new_readonly(solana_sdk_ids::system_program::ID, false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
                AccountMeta::new_readonly(ASSOCIATED_TOKEN_PROGRAM_ID.parse().unwrap(), false),
            ],
            data,
        };
        let tx = Transaction::new(&[&maker], Message::new(&[ix], Some(&maker.pubkey())), svm.latest_blockhash());
        let res = svm.send_transaction(tx).expect("Make failed");
        println!("Make CUs Consumed: {}", res.compute_units_consumed);

        assert_eq!(token_amount(&svm, &vault), AMOUNT_TO_GIVE);

        Made { svm, maker, mint_a, mint_b, escrow, bump, vault, maker_ata_a }
    }

    /// A funded taker holding `b_amount` of mint B in their own ATA.
    fn new_taker(m: &mut Made, b_amount: u64) -> (Keypair, Pubkey) {
        let taker = Keypair::new();
        m.svm.airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();
        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut m.svm, &taker, &m.mint_b)
            .owner(&taker.pubkey()).send().unwrap();
        MintTo::new(&mut m.svm, &m.maker, &m.mint_b, &taker_ata_b, b_amount).send().unwrap();
        (taker, taker_ata_b)
    }

    /// Take instruction, twelve accounts in the order documented in take.rs.
    fn take_ix(m: &Made, taker: &Pubkey, maker: &Pubkey, taker_ata_b: &Pubkey) -> Instruction {
        let taker_ata_a = spl_associated_token_account::get_associated_token_address(taker, &m.mint_a);
        let maker_ata_b = spl_associated_token_account::get_associated_token_address(maker, &m.mint_b);
        Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(*taker, true),
                AccountMeta::new(*maker, false),
                AccountMeta::new_readonly(m.mint_a, false),
                AccountMeta::new_readonly(m.mint_b, false),
                AccountMeta::new(m.escrow, false),
                AccountMeta::new(m.vault, false),
                AccountMeta::new(taker_ata_a, false),
                AccountMeta::new(*taker_ata_b, false),
                AccountMeta::new(maker_ata_b, false),
                AccountMeta::new_readonly(solana_sdk_ids::system_program::ID, false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
                AccountMeta::new_readonly(ASSOCIATED_TOKEN_PROGRAM_ID.parse().unwrap(), false),
            ],
            data: vec![1u8],
        }
    }

    /// Cancel instruction, six accounts in the order documented in cancel.rs.
    fn cancel_ix(m: &Made, signer: &Pubkey, maker_ata_a: &Pubkey) -> Instruction {
        Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(*signer, true),
                AccountMeta::new_readonly(m.mint_a, false),
                AccountMeta::new(m.escrow, false),
                AccountMeta::new(m.vault, false),
                AccountMeta::new(*maker_ata_a, false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
            ],
            data: vec![2u8],
        }
    }

    #[test]
    pub fn test_take_instruction() {
        let mut m = setup_make();
        let (taker, taker_ata_b) = new_taker(&mut m, AMOUNT_TO_RECEIVE);
        let maker_pk = m.maker.pubkey();

        let vault_rent = m.svm.get_account(&m.vault).unwrap().lamports;
        let escrow_rent = m.svm.get_account(&m.escrow).unwrap().lamports;
        let maker_sol_before = m.svm.get_balance(&maker_pk).unwrap();

        let ix = take_ix(&m, &taker.pubkey(), &maker_pk, &taker_ata_b);
        let tx = Transaction::new(&[&taker], Message::new(&[ix], Some(&taker.pubkey())), m.svm.latest_blockhash());
        let res = m.svm.send_transaction(tx).expect("Take failed");
        println!("\n\nTake transaction successful");
        println!("CUs Consumed: {}", res.compute_units_consumed);

        let taker_ata_a = spl_associated_token_account::get_associated_token_address(&taker.pubkey(), &m.mint_a);
        let maker_ata_b = spl_associated_token_account::get_associated_token_address(&maker_pk, &m.mint_b);
        assert_eq!(token_amount(&m.svm, &taker_ata_a), AMOUNT_TO_GIVE, "taker got 500 A");
        assert_eq!(token_amount(&m.svm, &maker_ata_b), AMOUNT_TO_RECEIVE, "maker got 100 B");
        assert_eq!(token_amount(&m.svm, &taker_ata_b), 0, "taker spent all B");
        assert_closed(&m.svm, &m.vault, "vault");
        assert_closed(&m.svm, &m.escrow, "escrow");

        // The taker paid fees and ATA rent, so the maker gains exactly both closed rents.
        let maker_sol_after = m.svm.get_balance(&maker_pk).unwrap();
        println!("Maker SOL +{} (vault rent {} + escrow rent {})", maker_sol_after - maker_sol_before, vault_rent, escrow_rent);
        assert_eq!(maker_sol_after - maker_sol_before, vault_rent + escrow_rent);
    }

    #[test]
    pub fn test_cancel_instruction() {
        let mut m = setup_make();
        let maker_pk = m.maker.pubkey();
        let rent_back = m.svm.get_account(&m.vault).unwrap().lamports + m.svm.get_account(&m.escrow).unwrap().lamports;
        let maker_sol_before = m.svm.get_balance(&maker_pk).unwrap();

        let ix = cancel_ix(&m, &maker_pk, &m.maker_ata_a);
        let tx = Transaction::new(&[&m.maker], Message::new(&[ix], Some(&maker_pk)), m.svm.latest_blockhash());
        let res = m.svm.send_transaction(tx).expect("Cancel failed");
        println!("\n\nCancel transaction successful");
        println!("CUs Consumed: {}", res.compute_units_consumed);

        assert_eq!(token_amount(&m.svm, &m.maker_ata_a), MAKER_START_A, "maker back to 1,000 A");
        assert_closed(&m.svm, &m.vault, "vault");
        assert_closed(&m.svm, &m.escrow, "escrow");

        // Maker paid a 5000-lamport fee and got both rents back.
        let maker_sol_after = m.svm.get_balance(&maker_pk).unwrap();
        assert_eq!(maker_sol_after + 5000, maker_sol_before + rent_back);
    }

    #[test]
    pub fn test_take_fails_when_taker_underfunded() {
        let mut m = setup_make();
        let (taker, taker_ata_b) = new_taker(&mut m, 50_000_000); // only 50 B
        let maker_pk = m.maker.pubkey();

        let ix = take_ix(&m, &taker.pubkey(), &maker_pk, &taker_ata_b);
        let tx = Transaction::new(&[&taker], Message::new(&[ix], Some(&taker.pubkey())), m.svm.latest_blockhash());
        let result = m.svm.send_transaction(tx);
        match &result {
            Err(e) => println!("Underfunded Take rejected: {:?}\nCUs Consumed: {}", e.err, e.meta.compute_units_consumed),
            Ok(_) => {}
        }
        assert!(result.is_err(), "a taker with only 50 B must not be able to take");

        // Nothing moved.
        assert_eq!(token_amount(&m.svm, &m.vault), AMOUNT_TO_GIVE);
        assert_eq!(token_amount(&m.svm, &taker_ata_b), 50_000_000);
        assert_eq!(m.svm.get_account(&m.escrow).unwrap().owner, program_id());
    }

    #[test]
    pub fn test_cancel_fails_for_stranger() {
        let mut m = setup_make();
        let stranger = Keypair::new();
        m.svm.airdrop(&stranger.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();
        let stranger_ata_a = CreateAssociatedTokenAccount::new(&mut m.svm, &stranger, &m.mint_a)
            .owner(&stranger.pubkey()).send().unwrap();

        // Attempt 1: stranger signs in the maker slot, and routes A to their own ATA.
        let ix = cancel_ix(&m, &stranger.pubkey(), &stranger_ata_a);
        let tx = Transaction::new(&[&stranger], Message::new(&[ix], Some(&stranger.pubkey())), m.svm.latest_blockhash());
        let result = m.svm.send_transaction(tx);
        match &result {
            Err(e) => println!("Stranger Cancel rejected: {:?}\nCUs Consumed: {}", e.err, e.meta.compute_units_consumed),
            Ok(_) => {}
        }
        assert!(result.is_err(), "a stranger must not be able to cancel someone else's escrow");

        // Attempt 2: real maker in slot 0 but NOT signing (stranger pays). Program must
        // reject on is_signer. The runtime needs the slot to be a non-signer meta.
        let mut ix = cancel_ix(&m, &m.maker.pubkey(), &m.maker_ata_a);
        ix.accounts[0] = AccountMeta::new(m.maker.pubkey(), false);
        let tx = Transaction::new(&[&stranger], Message::new(&[ix], Some(&stranger.pubkey())), m.svm.latest_blockhash());
        let result = m.svm.send_transaction(tx);
        match &result {
            Err(e) => println!("Unsigned maker Cancel rejected: {:?}\nCUs Consumed: {}", e.err, e.meta.compute_units_consumed),
            Ok(_) => {}
        }
        assert!(result.is_err(), "cancel without the maker's signature must fail");

        assert_eq!(token_amount(&m.svm, &m.vault), AMOUNT_TO_GIVE, "A never left the vault");
        assert_eq!(token_amount(&m.svm, &stranger_ata_a), 0);
        assert_eq!(m.svm.get_account(&m.escrow).unwrap().owner, program_id());
    }

    #[test]
    pub fn test_take_fails_with_mismatched_maker() {
        let mut m = setup_make();
        let (taker, taker_ata_b) = new_taker(&mut m, AMOUNT_TO_RECEIVE);
        let fake_maker = Keypair::new().pubkey();

        let ix = take_ix(&m, &taker.pubkey(), &fake_maker, &taker_ata_b);
        let tx = Transaction::new(&[&taker], Message::new(&[ix], Some(&taker.pubkey())), m.svm.latest_blockhash());
        let result = m.svm.send_transaction(tx);
        match &result {
            Err(e) => println!("Mismatched-maker Take rejected: {:?}\nCUs Consumed: {}", e.err, e.meta.compute_units_consumed),
            Ok(_) => {}
        }
        assert!(result.is_err(), "take with the wrong maker must fail");

        assert_eq!(token_amount(&m.svm, &m.vault), AMOUNT_TO_GIVE);
        assert_eq!(token_amount(&m.svm, &taker_ata_b), AMOUNT_TO_RECEIVE);
        assert_eq!(m.svm.get_account(&m.escrow).unwrap().data[112], m.bump, "escrow state untouched");
    }
}