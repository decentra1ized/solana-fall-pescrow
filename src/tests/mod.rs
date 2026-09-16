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

        const AMOUNT_TO_RECEIVE: u64 = 100_000_000; // 100 B
    const AMOUNT_TO_GIVE: u64 = 500_000_000;    // 500 A
    const MAKER_INITIAL_A: u64 = 1_000_000_000; // 1,000 A

    struct Escrowed {
        svm: LiteSVM,
        maker: Keypair,
        mint_a: Pubkey,
        mint_b: Pubkey,
        maker_ata_a: Pubkey,
        escrow: Pubkey,
        bump: u8,
        vault: Pubkey,
        make_cus: u64,
    }

    /// Everything every test needs: two mints, a funded maker, and a live escrow.
    fn make_escrow() -> Escrowed {
        let (mut svm, payer) = setup();
        let program_id = program_id();
        assert_eq!(program_id.to_string(), PROGRAM_ID);

        let mint_a = CreateMint::new(&mut svm, &payer)
            .decimals(6).authority(&payer.pubkey()).send().unwrap();
        let mint_b = CreateMint::new(&mut svm, &payer)
            .decimals(6).authority(&payer.pubkey()).send().unwrap();

        let maker_ata_a = CreateAssociatedTokenAccount::new(&mut svm, &payer, &mint_a)
            .owner(&payer.pubkey()).send().unwrap();

        MintTo::new(&mut svm, &payer, &mint_a, &maker_ata_a, MAKER_INITIAL_A)
            .send().unwrap();

        let (escrow, bump) = Pubkey::find_program_address(
            &[b"escrow".as_ref(), payer.pubkey().as_ref()],
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
                AccountMeta::new(payer.pubkey(), true),
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

        let message = Message::new(&[make_ix], Some(&payer.pubkey()));
        let blockhash = svm.latest_blockhash();
        let tx = svm.send_transaction(Transaction::new(&[&payer], message, blockhash)).unwrap();

        Escrowed {
            svm, maker: payer, mint_a, mint_b, maker_ata_a,
            escrow, bump, vault, make_cus: tx.compute_units_consumed,
        }
    }

    fn token_amount(svm: &LiteSVM, ata: &Pubkey) -> u64 {
        let acc = svm.get_account(ata).unwrap();
        spl_token_2022::state::Account::unpack(&acc.data).unwrap().amount
    }

    /// close() leaves either nothing, or a zero-lamport system-owned husk.
    fn assert_closed(svm: &LiteSVM, key: &Pubkey, label: &str) {
        match svm.get_account(key) {
            None => {}
            Some(acc) => assert!(
                acc.lamports == 0 && acc.owner == solana_sdk_ids::system_program::ID,
                "{label} is still open: {} lamports, owner {}", acc.lamports, acc.owner
            ),
        }
    }

    #[test]
    pub fn test_make_instruction() {
        let e = make_escrow();
        println!("\nMake transaction successful");
        println!("CUs Consumed: {}", e.make_cus);

        assert_eq!(token_amount(&e.svm, &e.vault), AMOUNT_TO_GIVE);
        assert_eq!(token_amount(&e.svm, &e.maker_ata_a), MAKER_INITIAL_A - AMOUNT_TO_GIVE);

        let esc = e.svm.get_account(&e.escrow).unwrap();
        assert_eq!(esc.owner, program_id());
        let d = &esc.data;
        assert_eq!(d.len(), 113);
        assert_eq!(&d[0..32], e.maker.pubkey().as_ref());
        assert_eq!(&d[32..64], e.mint_a.as_ref());
        assert_eq!(&d[64..96], e.mint_b.as_ref());
        assert_eq!(u64::from_le_bytes(d[96..104].try_into().unwrap()), AMOUNT_TO_RECEIVE);
        assert_eq!(u64::from_le_bytes(d[104..112].try_into().unwrap()), AMOUNT_TO_GIVE);
        assert_eq!(d[112], e.bump);
    }

    /// Builds the 12 Take accounts. Order here is the program's API — keep it in sync with take.rs.
    fn take_ix(
        taker: &Pubkey, maker: &Pubkey, mint_a: &Pubkey, mint_b: &Pubkey,
        escrow: &Pubkey, vault: &Pubkey,
        taker_ata_a: &Pubkey, taker_ata_b: &Pubkey, maker_ata_b: &Pubkey,
    ) -> Instruction {
        Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(*taker, true),
                AccountMeta::new(*maker, false),
                AccountMeta::new_readonly(*mint_a, false),
                AccountMeta::new_readonly(*mint_b, false),
                AccountMeta::new(*escrow, false),
                AccountMeta::new(*vault, false),
                AccountMeta::new(*taker_ata_a, false),
                AccountMeta::new(*taker_ata_b, false),
                AccountMeta::new(*maker_ata_b, false),
                AccountMeta::new_readonly(solana_sdk_ids::system_program::ID, false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
                AccountMeta::new_readonly(ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap(), false),
            ],
            data: vec![1u8],
        }
    }

    /// A funded taker holding `b_balance` of mint B. Its ATA for A is NOT created —
    /// the program must do that with CreateIdempotent.
    fn fund_taker(e: &mut Escrowed, b_balance: u64) -> (Keypair, Pubkey, Pubkey, Pubkey) {
        let taker = Keypair::new();
        e.svm.airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut e.svm, &taker, &e.mint_b)
            .owner(&taker.pubkey()).send().unwrap();
        MintTo::new(&mut e.svm, &e.maker, &e.mint_b, &taker_ata_b, b_balance)
            .send().unwrap();

        let taker_ata_a = spl_associated_token_account::get_associated_token_address(&taker.pubkey(), &e.mint_a);
        let maker_ata_b = spl_associated_token_account::get_associated_token_address(&e.maker.pubkey(), &e.mint_b);
        (taker, taker_ata_a, taker_ata_b, maker_ata_b)
    }

    #[test]
    pub fn test_take_instruction() {
        let mut e = make_escrow();
        let (taker, taker_ata_a, taker_ata_b, maker_ata_b) = fund_taker(&mut e, AMOUNT_TO_RECEIVE);

        let maker_sol_before = e.svm.get_balance(&e.maker.pubkey()).unwrap();

        let ix = take_ix(
            &taker.pubkey(), &e.maker.pubkey(), &e.mint_a, &e.mint_b,
            &e.escrow, &e.vault, &taker_ata_a, &taker_ata_b, &maker_ata_b,
        );
        let message = Message::new(&[ix], Some(&taker.pubkey()));
        let blockhash = e.svm.latest_blockhash();
        let tx = e.svm.send_transaction(Transaction::new(&[&taker], message, blockhash)).unwrap();

        println!("\nTake transaction successful");
        println!("CUs Consumed: {} (Make was {})", tx.compute_units_consumed, e.make_cus);

        // The trade happened...
        assert_eq!(token_amount(&e.svm, &taker_ata_a), AMOUNT_TO_GIVE);
        assert_eq!(token_amount(&e.svm, &maker_ata_b), AMOUNT_TO_RECEIVE);
        // ...and the escrow cleaned up after itself.
        assert_closed(&e.svm, &e.vault, "vault");
        assert_closed(&e.svm, &e.escrow, "escrow");

        let maker_sol_after = e.svm.get_balance(&e.maker.pubkey()).unwrap();
        assert!(
            maker_sol_after > maker_sol_before,
            "maker should have received rent back: {maker_sol_before} -> {maker_sol_after}"
        );
    }

    fn cancel_ix(maker: &Pubkey, mint_a: &Pubkey, escrow: &Pubkey, vault: &Pubkey, maker_ata_a: &Pubkey) -> Instruction {
        Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(*maker, true),
                AccountMeta::new_readonly(*mint_a, false),
                AccountMeta::new(*escrow, false),
                AccountMeta::new(*vault, false),
                AccountMeta::new(*maker_ata_a, false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
            ],
            data: vec![2u8],
        }
    }

    #[test]
    pub fn test_cancel_instruction() {
        let mut e = make_escrow();

        let ix = cancel_ix(&e.maker.pubkey(), &e.mint_a, &e.escrow, &e.vault, &e.maker_ata_a);
        let message = Message::new(&[ix], Some(&e.maker.pubkey()));
        let blockhash = e.svm.latest_blockhash();
        let tx = e.svm.send_transaction(Transaction::new(&[&e.maker], message, blockhash)).unwrap();

        println!("\nCancel transaction successful");
        println!("CUs Consumed: {} (Make was {})", tx.compute_units_consumed, e.make_cus);

        assert_eq!(token_amount(&e.svm, &e.maker_ata_a), MAKER_INITIAL_A);
        assert_closed(&e.svm, &e.vault, "vault");
        assert_closed(&e.svm, &e.escrow, "escrow");
    }

    #[test]
    pub fn test_take_fails_when_taker_underfunded() {
        let mut e = make_escrow();
        // Only 50 B, the escrow wants 100.
        let (taker, taker_ata_a, taker_ata_b, maker_ata_b) = fund_taker(&mut e, 50_000_000);

        let ix = take_ix(
            &taker.pubkey(), &e.maker.pubkey(), &e.mint_a, &e.mint_b,
            &e.escrow, &e.vault, &taker_ata_a, &taker_ata_b, &maker_ata_b,
        );
        let message = Message::new(&[ix], Some(&taker.pubkey()));
        let blockhash = e.svm.latest_blockhash();
        let result = e.svm.send_transaction(Transaction::new(&[&taker], message, blockhash));

        assert!(result.is_err(), "an underfunded taker must not be able to take");
        // Atomic: the B transfer failed, so the A never left either.
        assert_eq!(token_amount(&e.svm, &e.vault), AMOUNT_TO_GIVE);
    }

    #[test]
    pub fn test_cancel_by_stranger_fails() {
        let mut e = make_escrow();

        let stranger = Keypair::new();
        e.svm.airdrop(&stranger.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();
        let stranger_ata_a = spl_associated_token_account::get_associated_token_address(
            &stranger.pubkey(), &e.mint_a,
        );

        // Real escrow, real vault — but the signer is not the stored maker.
        let ix = cancel_ix(&stranger.pubkey(), &e.mint_a, &e.escrow, &e.vault, &stranger_ata_a);
        let message = Message::new(&[ix], Some(&stranger.pubkey()));
        let blockhash = e.svm.latest_blockhash();
        let result = e.svm.send_transaction(Transaction::new(&[&stranger], message, blockhash));

        assert!(result.is_err(), "a stranger must not be able to cancel");
        assert_eq!(token_amount(&e.svm, &e.vault), AMOUNT_TO_GIVE, "the 500 A must still be in the vault");
    }
}