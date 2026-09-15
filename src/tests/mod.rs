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

    const AMOUNT_TO_RECEIVE: u64 = 100000000; // 100 B
    const AMOUNT_TO_GIVE: u64 = 500000000;    // 500 A
    const MAKER_START_A: u64 = 1000000000;    // 1,000 A

    struct Escrow { mint_a: Pubkey, mint_b: Pubkey, maker_ata_a: Pubkey, escrow: Pubkey, vault: Pubkey }

    /// Creates both mints, funds the maker with 1,000 A and runs Make (500 A for 100 B).
    /// Takes the svm from `setup()` so the Rent sysvar override stays in place.
    fn make_escrow(svm: &mut LiteSVM, maker: &Keypair) -> Escrow {
        let mint_a = CreateMint::new(svm, maker).decimals(6).authority(&maker.pubkey()).send().unwrap();
        let mint_b = CreateMint::new(svm, maker).decimals(6).authority(&maker.pubkey()).send().unwrap();
        let maker_ata_a = CreateAssociatedTokenAccount::new(svm, maker, &mint_a)
            .owner(&maker.pubkey()).send().unwrap();
        MintTo::new(svm, maker, &mint_a, &maker_ata_a, MAKER_START_A).send().unwrap();

        let (escrow, _) = Pubkey::find_program_address(&[b"escrow".as_ref(), maker.pubkey().as_ref()], &program_id());
        let vault = spl_associated_token_account::get_associated_token_address(&escrow, &mint_a);

        let make_ix = Instruction {
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
                AccountMeta::new_readonly(ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap(), false),
            ],
            data: [vec![0u8], AMOUNT_TO_RECEIVE.to_le_bytes().to_vec(), AMOUNT_TO_GIVE.to_le_bytes().to_vec()].concat(),
        };
        let tx = Transaction::new(&[maker], Message::new(&[make_ix], Some(&maker.pubkey())), svm.latest_blockhash());
        svm.send_transaction(tx).unwrap();

        Escrow { mint_a, mint_b, maker_ata_a, escrow, vault }
    }

    fn token_amount(svm: &LiteSVM, ata: &Pubkey) -> u64 {
        spl_token_2022::state::Account::unpack(&svm.get_account(ata).unwrap().data).unwrap().amount
    }

    fn lamports(svm: &LiteSVM, key: &Pubkey) -> u64 {
        svm.get_account(key).map_or(0, |a| a.lamports)
    }

    /// Closed means gone, or left behind with 0 lamports and handed back to the System Program.
    fn is_closed(svm: &LiteSVM, key: &Pubkey) -> bool {
        svm.get_account(key).map_or(true, |a| a.lamports == 0 && a.owner == solana_sdk_ids::system_program::ID)
    }

    /// A taker with its own SOL and a mint-B account holding `amount_b`.
    fn fund_taker(svm: &mut LiteSVM, maker: &Keypair, e: &Escrow, amount_b: u64) -> (Keypair, Pubkey) {
        let taker = Keypair::new();
        svm.airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();
        let taker_ata_b = CreateAssociatedTokenAccount::new(svm, maker, &e.mint_b)
            .owner(&taker.pubkey()).send().unwrap();
        MintTo::new(svm, maker, &e.mint_b, &taker_ata_b, amount_b).send().unwrap();
        (taker, taker_ata_b)
    }

    /// Prints CU for a failed transaction and checks the deal is untouched: vault full, escrow open.
    fn assert_rejected_and_untouched(svm: &LiteSVM, result: litesvm::types::TransactionResult, e: &Escrow, why: &str) {
        let failed = match result {
            Ok(_) => panic!("{why}"),
            Err(failed) => failed,
        };
        println!("Rejected: {:?}\nCUs Consumed: {}", failed.err, failed.meta.compute_units_consumed);
        assert_eq!(token_amount(svm, &e.vault), AMOUNT_TO_GIVE);
        assert!(!is_closed(svm, &e.escrow));
    }

    #[test]
    pub fn test_take_instruction() {
        let (mut svm, maker) = setup();
        let e = make_escrow(&mut svm, &maker);
        let (taker, taker_ata_b) = fund_taker(&mut svm, &maker, &e, AMOUNT_TO_RECEIVE);

        // Derived, not created: Take has to create these itself.
        let taker_ata_a = spl_associated_token_account::get_associated_token_address(&taker.pubkey(), &e.mint_a);
        let maker_ata_b = spl_associated_token_account::get_associated_token_address(&maker.pubkey(), &e.mint_b);
        assert!(svm.get_account(&taker_ata_a).is_none());
        assert!(svm.get_account(&maker_ata_b).is_none());

        let maker_before = lamports(&svm, &maker.pubkey());
        let rent_back = lamports(&svm, &e.escrow) + lamports(&svm, &e.vault);

        let take_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(taker.pubkey(), true),                   // 0  taker, signs
                AccountMeta::new(maker.pubkey(), false),                  // 1  maker, gets B and the rent
                AccountMeta::new_readonly(e.mint_a, false),               // 2
                AccountMeta::new_readonly(e.mint_b, false),               // 3
                AccountMeta::new(e.escrow, false),                        // 4  closed by Take
                AccountMeta::new(e.vault, false),                         // 5  closed by Take
                AccountMeta::new(taker_ata_a, false),                     // 6  receives A
                AccountMeta::new(taker_ata_b, false),                     // 7  pays B
                AccountMeta::new(maker_ata_b, false),                     // 8  receives B
                AccountMeta::new_readonly(solana_sdk_ids::system_program::ID, false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
                AccountMeta::new_readonly(ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap(), false),
            ],
            data: vec![1u8], // Take: discriminator only
        };
        let tx = Transaction::new(&[&taker], Message::new(&[take_ix], Some(&taker.pubkey())), svm.latest_blockhash());
        let res = svm.send_transaction(tx).unwrap();
        println!("\n\nTake transaction successful\nCUs Consumed: {}", res.compute_units_consumed);

        assert_eq!(token_amount(&svm, &taker_ata_a), AMOUNT_TO_GIVE);
        assert_eq!(token_amount(&svm, &maker_ata_b), AMOUNT_TO_RECEIVE);
        assert_eq!(token_amount(&svm, &taker_ata_b), 0);
        assert!(is_closed(&svm, &e.vault));
        assert!(is_closed(&svm, &e.escrow));
        // The taker pays the fee and the new ATAs, so the maker gains exactly the rent of both closed accounts.
        assert_eq!(lamports(&svm, &maker.pubkey()), maker_before + rent_back);
    }

    #[test]
    pub fn test_cancel_instruction() {
        let (mut svm, maker) = setup();
        let e = make_escrow(&mut svm, &maker);
        assert_eq!(token_amount(&svm, &e.maker_ata_a), MAKER_START_A - AMOUNT_TO_GIVE);

        let cancel_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(maker.pubkey(), true),                   // 0  maker, signs
                AccountMeta::new_readonly(e.mint_a, false),               // 1
                AccountMeta::new(e.escrow, false),                        // 2  closed by Cancel
                AccountMeta::new(e.vault, false),                         // 3  closed by Cancel
                AccountMeta::new(e.maker_ata_a, false),                   // 4  gets the A back
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),       // 5
            ],
            data: vec![2u8], // Cancel: discriminator only
        };
        let tx = Transaction::new(&[&maker], Message::new(&[cancel_ix], Some(&maker.pubkey())), svm.latest_blockhash());
        let res = svm.send_transaction(tx).unwrap();
        println!("\n\nCancel transaction successful\nCUs Consumed: {}", res.compute_units_consumed);

        assert_eq!(token_amount(&svm, &e.maker_ata_a), MAKER_START_A);
        assert!(is_closed(&svm, &e.vault));
        assert!(is_closed(&svm, &e.escrow));
    }

    #[test]
    pub fn test_take_underfunded_fails() {
        let (mut svm, maker) = setup();
        let e = make_escrow(&mut svm, &maker);
        let (taker, taker_ata_b) = fund_taker(&mut svm, &maker, &e, AMOUNT_TO_RECEIVE / 2); // only 50 B

        let taker_ata_a = spl_associated_token_account::get_associated_token_address(&taker.pubkey(), &e.mint_a);
        let maker_ata_b = spl_associated_token_account::get_associated_token_address(&maker.pubkey(), &e.mint_b);

        let take_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(taker.pubkey(), true),
                AccountMeta::new(maker.pubkey(), false),
                AccountMeta::new_readonly(e.mint_a, false),
                AccountMeta::new_readonly(e.mint_b, false),
                AccountMeta::new(e.escrow, false),
                AccountMeta::new(e.vault, false),
                AccountMeta::new(taker_ata_a, false),
                AccountMeta::new(taker_ata_b, false),
                AccountMeta::new(maker_ata_b, false),
                AccountMeta::new_readonly(solana_sdk_ids::system_program::ID, false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
                AccountMeta::new_readonly(ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap(), false),
            ],
            data: vec![1u8],
        };
        let tx = Transaction::new(&[&taker], Message::new(&[take_ix], Some(&taker.pubkey())), svm.latest_blockhash());

        println!("\n\nUnderfunded Take");
        let result = svm.send_transaction(tx); // no unwrap: this one must fail
        assert_rejected_and_untouched(&svm, result, &e, "a taker with 50 B must not be able to take");

        // The whole transaction rolled back: the taker keeps its B, and the ATA Take created is gone too.
        assert_eq!(token_amount(&svm, &taker_ata_b), AMOUNT_TO_RECEIVE / 2);
        assert!(svm.get_account(&taker_ata_a).is_none());
    }

    #[test]
    pub fn test_stranger_cannot_cancel() {
        let (mut svm, maker) = setup();
        let e = make_escrow(&mut svm, &maker);

        let stranger = Keypair::new();
        svm.airdrop(&stranger.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

        let cancel_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(stranger.pubkey(), true),                // the stranger signs and poses as the maker
                AccountMeta::new_readonly(e.mint_a, false),
                AccountMeta::new(e.escrow, false),                        // ...of someone else's escrow
                AccountMeta::new(e.vault, false),
                AccountMeta::new(e.maker_ata_a, false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
            ],
            data: vec![2u8],
        };
        let tx = Transaction::new(&[&stranger], Message::new(&[cancel_ix], Some(&stranger.pubkey())), svm.latest_blockhash());

        println!("\n\nStranger's Cancel");
        let result = svm.send_transaction(tx); // no unwrap: this one must fail
        assert_rejected_and_untouched(&svm, result, &e, "a stranger must not be able to cancel someone else's escrow");
        assert_eq!(token_amount(&svm, &e.maker_ata_a), MAKER_START_A - AMOUNT_TO_GIVE);
    }

    #[test]
    pub fn test_take_with_wrong_maker_fails() {
        let (mut svm, maker) = setup();
        let e = make_escrow(&mut svm, &maker);
        let (taker, taker_ata_b) = fund_taker(&mut svm, &maker, &e, AMOUNT_TO_RECEIVE);

        // The taker names itself as the maker: it would pay the B to itself and pocket the rent.
        let fake_maker = taker.pubkey();
        let taker_ata_a = spl_associated_token_account::get_associated_token_address(&taker.pubkey(), &e.mint_a);
        let fake_maker_ata_b = spl_associated_token_account::get_associated_token_address(&fake_maker, &e.mint_b);

        let take_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(taker.pubkey(), true),
                AccountMeta::new(fake_maker, false),                      // not escrow.maker()
                AccountMeta::new_readonly(e.mint_a, false),
                AccountMeta::new_readonly(e.mint_b, false),
                AccountMeta::new(e.escrow, false),
                AccountMeta::new(e.vault, false),
                AccountMeta::new(taker_ata_a, false),
                AccountMeta::new(taker_ata_b, false),
                AccountMeta::new(fake_maker_ata_b, false),
                AccountMeta::new_readonly(solana_sdk_ids::system_program::ID, false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
                AccountMeta::new_readonly(ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap(), false),
            ],
            data: vec![1u8],
        };
        let tx = Transaction::new(&[&taker], Message::new(&[take_ix], Some(&taker.pubkey())), svm.latest_blockhash());

        println!("\n\nTake with wrong maker");
        let result = svm.send_transaction(tx); // no unwrap: this one must fail
        assert_rejected_and_untouched(&svm, result, &e, "Take must reject a maker that is not escrow.maker()");
        assert_eq!(token_amount(&svm, &taker_ata_b), AMOUNT_TO_RECEIVE);
    }
}
