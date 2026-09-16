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

    const AMOUNT_TO_RECEIVE: u64 = 100_000_000;   // 100 B, 6 decimals
    const AMOUNT_TO_GIVE: u64 = 500_000_000;      // 500 A
    const MAKER_INITIAL_A: u64 = 1_000_000_000;   // 1,000 A minted to the maker

    fn program_id() -> Pubkey {
        Pubkey::from(crate::ID)
    }

    fn associated_token_program() -> Pubkey {
        ASSOCIATED_TOKEN_PROGRAM_ID.parse().unwrap()
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

    struct MakeSetup {
        svm: LiteSVM,
        maker: Keypair,
        mint_a: Pubkey,
        mint_b: Pubkey,
        maker_ata_a: Pubkey,
        escrow: Pubkey,
        bump: u8,
        vault: Pubkey,
    }

    /// Runs `setup()` and a successful Make. Every test starts here.
    fn make_escrow() -> MakeSetup {
        let (mut svm, maker) = setup();
        let program_id = program_id();

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
            &program_id,
        );
        let vault = spl_associated_token_account::get_associated_token_address(&escrow, &mint_a);

        MintTo::new(&mut svm, &maker, &mint_a, &maker_ata_a, MAKER_INITIAL_A)
            .send()
            .unwrap();

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
                AccountMeta::new(associated_token_program(), false),
            ],
            data: make_data,
        };

        let message = Message::new(&[make_ix], Some(&maker.pubkey()));
        let transaction = Transaction::new(&[&maker], message, svm.latest_blockhash());
        let tx = svm.send_transaction(transaction).unwrap();
        println!("Make CUs Consumed: {}", tx.compute_units_consumed);

        MakeSetup { svm, maker, mint_a, mint_b, maker_ata_a, escrow, bump, vault }
    }

    fn token_balance(svm: &LiteSVM, ata: &Pubkey) -> u64 {
        let acc = svm.get_account(ata).unwrap();
        spl_token_2022::state::Account::unpack(&acc.data).unwrap().amount
    }

    /// A closed account is either gone or left with no lamports and no owner.
    fn assert_closed(svm: &LiteSVM, address: &Pubkey, label: &str) {
        if let Some(acc) = svm.get_account(address) {
            assert_eq!(acc.lamports, 0, "{label} still holds lamports");
            assert_eq!(acc.owner, solana_sdk_ids::system_program::ID, "{label} still has an owner");
        }
    }

    /// Take, account order from take.rs.
    fn take_ix(
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
                AccountMeta::new_readonly(*mint_a, false),
                AccountMeta::new_readonly(*mint_b, false),
                AccountMeta::new(*escrow, false),
                AccountMeta::new(*vault, false),
                AccountMeta::new(*taker_ata_a, false),
                AccountMeta::new(*taker_ata_b, false),
                AccountMeta::new(*maker_ata_b, false),
                AccountMeta::new_readonly(solana_sdk_ids::system_program::ID, false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
                AccountMeta::new_readonly(associated_token_program(), false),
            ],
            data: vec![1u8],
        }
    }

    /// Cancel, account order from cancel.rs.
    fn cancel_ix(
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
                AccountMeta::new_readonly(*mint_a, false),
                AccountMeta::new(*escrow, false),
                AccountMeta::new(*vault, false),
                AccountMeta::new(*maker_ata_a, false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
            ],
            data: vec![2u8],
        }
    }

    /// Creates a funded taker: SOL for fees, and `amount_b` of mint B in its ATA.
    fn fund_taker(svm: &mut LiteSVM, maker: &Keypair, mint_b: &Pubkey, amount_b: u64) -> (Keypair, Pubkey) {
        let taker = Keypair::new();
        svm.airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL).expect("Airdrop failed");

        let taker_ata_b = CreateAssociatedTokenAccount::new(svm, &taker, mint_b)
            .owner(&taker.pubkey())
            .send()
            .unwrap();
        MintTo::new(svm, maker, mint_b, &taker_ata_b, amount_b)
            .send()
            .unwrap();

        (taker, taker_ata_b)
    }

    #[test]
    pub fn test_make_instruction() {
        let MakeSetup { svm, maker, maker_ata_a, escrow, bump, vault, .. } = make_escrow();

        assert_eq!(program_id().to_string(), PROGRAM_ID);

        let vault_acc = svm.get_account(&vault).unwrap();
        let vault_state = spl_token_2022::state::Account::unpack(&vault_acc.data).unwrap();
        assert_eq!(vault_state.owner, escrow);
        assert_eq!(vault_state.amount, AMOUNT_TO_GIVE);

        assert_eq!(token_balance(&svm, &maker_ata_a), MAKER_INITIAL_A - AMOUNT_TO_GIVE);

        let esc = svm.get_account(&escrow).unwrap();
        assert_eq!(esc.owner, program_id());
        assert_eq!(esc.data.len(), 113);
        let d = &esc.data;
        assert_eq!(&d[0..32], maker.pubkey().as_ref());
        assert_eq!(u64::from_le_bytes(d[96..104].try_into().unwrap()), AMOUNT_TO_RECEIVE);
        assert_eq!(u64::from_le_bytes(d[104..112].try_into().unwrap()), AMOUNT_TO_GIVE);
        assert_eq!(d[112], bump);
    }

    #[test]
    pub fn test_take_instruction() {
        let MakeSetup { mut svm, maker, mint_a, mint_b, escrow, vault, .. } = make_escrow();

        let (taker, taker_ata_b) = fund_taker(&mut svm, &maker, &mint_b, AMOUNT_TO_RECEIVE);

        // Derived only. The program creates these with CreateIdempotent.
        let taker_ata_a = spl_associated_token_account::get_associated_token_address(&taker.pubkey(), &mint_a);
        let maker_ata_b = spl_associated_token_account::get_associated_token_address(&maker.pubkey(), &mint_b);

        let maker_sol_before = svm.get_balance(&maker.pubkey()).unwrap();
        let vault_rent = svm.get_account(&vault).unwrap().lamports;
        let escrow_rent = svm.get_account(&escrow).unwrap().lamports;

        let ix = take_ix(
            &taker.pubkey(), &maker.pubkey(), &mint_a, &mint_b,
            &escrow, &vault, &taker_ata_a, &taker_ata_b, &maker_ata_b,
        );
        let message = Message::new(&[ix], Some(&taker.pubkey()));
        let transaction = Transaction::new(&[&taker], message, svm.latest_blockhash());
        let tx = svm.send_transaction(transaction).unwrap();
        println!("Take CUs Consumed: {}", tx.compute_units_consumed);

        assert_eq!(token_balance(&svm, &taker_ata_a), AMOUNT_TO_GIVE, "taker did not receive A");
        assert_eq!(token_balance(&svm, &maker_ata_b), AMOUNT_TO_RECEIVE, "maker did not receive B");

        assert_closed(&svm, &vault, "vault");
        assert_closed(&svm, &escrow, "escrow");

        // The maker paid no fee here, so the refund is exactly both rents.
        let maker_sol_after = svm.get_balance(&maker.pubkey()).unwrap();
        assert_eq!(maker_sol_after, maker_sol_before + vault_rent + escrow_rent);
    }

    #[test]
    pub fn test_cancel_instruction() {
        let MakeSetup { mut svm, maker, mint_a, maker_ata_a, escrow, vault, .. } = make_escrow();

        let ix = cancel_ix(&maker.pubkey(), &mint_a, &escrow, &vault, &maker_ata_a);
        let message = Message::new(&[ix], Some(&maker.pubkey()));
        let transaction = Transaction::new(&[&maker], message, svm.latest_blockhash());
        let tx = svm.send_transaction(transaction).unwrap();
        println!("Cancel CUs Consumed: {}", tx.compute_units_consumed);

        assert_eq!(token_balance(&svm, &maker_ata_a), MAKER_INITIAL_A, "maker did not get A back");

        assert_closed(&svm, &vault, "vault");
        assert_closed(&svm, &escrow, "escrow");
    }

    #[test]
    pub fn test_take_fails_when_taker_is_underfunded() {
        let MakeSetup { mut svm, maker, mint_a, mint_b, escrow, vault, .. } = make_escrow();

        // Half the price.
        let (taker, taker_ata_b) = fund_taker(&mut svm, &maker, &mint_b, AMOUNT_TO_RECEIVE / 2);

        let taker_ata_a = spl_associated_token_account::get_associated_token_address(&taker.pubkey(), &mint_a);
        let maker_ata_b = spl_associated_token_account::get_associated_token_address(&maker.pubkey(), &mint_b);

        let ix = take_ix(
            &taker.pubkey(), &maker.pubkey(), &mint_a, &mint_b,
            &escrow, &vault, &taker_ata_a, &taker_ata_b, &maker_ata_b,
        );
        let message = Message::new(&[ix], Some(&taker.pubkey()));
        let transaction = Transaction::new(&[&taker], message, svm.latest_blockhash());
        let result = svm.send_transaction(transaction);
        assert!(result.is_err(), "an underfunded taker must not be able to take");

        // Nothing moved.
        assert_eq!(token_balance(&svm, &vault), AMOUNT_TO_GIVE);
        assert_eq!(token_balance(&svm, &taker_ata_b), AMOUNT_TO_RECEIVE / 2);
        assert_eq!(svm.get_account(&escrow).unwrap().owner, program_id());
    }

    #[test]
    pub fn test_cancel_fails_for_stranger() {
        let MakeSetup { mut svm, mint_a, escrow, vault, .. } = make_escrow();

        let stranger = Keypair::new();
        svm.airdrop(&stranger.pubkey(), 10 * LAMPORTS_PER_SOL).expect("Airdrop failed");
        let stranger_ata_a = CreateAssociatedTokenAccount::new(&mut svm, &stranger, &mint_a)
            .owner(&stranger.pubkey())
            .send()
            .unwrap();

        // The stranger signs, and passes themself as the maker.
        let ix = cancel_ix(&stranger.pubkey(), &mint_a, &escrow, &vault, &stranger_ata_a);
        let message = Message::new(&[ix], Some(&stranger.pubkey()));
        let transaction = Transaction::new(&[&stranger], message, svm.latest_blockhash());
        let result = svm.send_transaction(transaction);
        assert!(result.is_err(), "a stranger must not be able to cancel");

        // Nothing moved.
        assert_eq!(token_balance(&svm, &vault), AMOUNT_TO_GIVE);
        assert_eq!(token_balance(&svm, &stranger_ata_a), 0);
        assert_eq!(svm.get_account(&escrow).unwrap().owner, program_id());
    }
}
