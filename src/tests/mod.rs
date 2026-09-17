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

    // The terms every test trades on: 500 A (6 decimals) for 100 B (6 decimals).
    const AMOUNT_TO_RECEIVE: u64 = 100_000_000;
    const AMOUNT_TO_GIVE: u64 = 500_000_000;
    const MAKER_STARTING_A: u64 = 1_000_000_000;

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

    /// Common setup every test starts from: two mints, a funded maker ATA for
    /// mint A, and a sent Make transaction. Returns everything the later
    /// steps need: the svm, the maker, both mints, the escrow PDA and its
    /// bump, and the vault address.
    fn make_escrow() -> (LiteSVM, Keypair, Pubkey, Pubkey, Pubkey, u8, Pubkey) {
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
            .owner(&maker.pubkey()).send().unwrap();

        let escrow = Pubkey::find_program_address(
            &[b"escrow".as_ref(), maker.pubkey().as_ref()],
            &program_id,
        );

        let vault = spl_associated_token_account::get_associated_token_address(
            &escrow.0,
            &mint_a,
        );

        let associated_token_program = ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();
        let token_program = TOKEN_PROGRAM_ID;
        let system_program = solana_sdk_ids::system_program::ID;

        MintTo::new(&mut svm, &maker, &mint_a, &maker_ata_a, MAKER_STARTING_A)
            .send()
            .unwrap();

        let bump: u8 = escrow.1;

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
                AccountMeta::new(escrow.0, false),
                AccountMeta::new(maker_ata_a, false),
                AccountMeta::new(vault, false),
                AccountMeta::new(system_program, false),
                AccountMeta::new(token_program, false),
                AccountMeta::new(associated_token_program, false),
            ],
            data: make_data,
        };

        let message = Message::new(&[make_ix], Some(&maker.pubkey()));
        let recent_blockhash = svm.latest_blockhash();
        let transaction = Transaction::new(&[&maker], message, recent_blockhash);
        svm.send_transaction(transaction).expect("Make failed");

        (svm, maker, mint_a, mint_b, escrow.0, bump, vault)
    }

    /// An account that has been closed is either gone entirely, or still
    /// present with 0 lamports (LiteSVM, like the real runtime, is free to
    /// do either before the account is actually purged).
    fn assert_closed(svm: &LiteSVM, pubkey: &Pubkey) {
        if let Some(account) = svm.get_account(pubkey) {
            assert_eq!(account.lamports, 0, "{pubkey} should have been closed (0 lamports)");
        }
    }

    #[test]
    pub fn test_make_instruction() {
        let (svm, maker, mint_a, _mint_b, escrow, bump, vault) = make_escrow();

        assert_eq!(program_id().to_string(), PROGRAM_ID);

        // --- verification ---
        let vault_acc = svm.get_account(&vault).unwrap();
        let vault_state = spl_token_2022::state::Account::unpack(&vault_acc.data).unwrap();
        println!("Vault owner: {} (escrow PDA? {})", vault_state.owner, vault_state.owner == escrow);
        println!("Vault balance: {}", vault_state.amount);
        assert_eq!(vault_state.amount, AMOUNT_TO_GIVE);

        let maker_ata_a = spl_associated_token_account::get_associated_token_address(&maker.pubkey(), &mint_a);
        let maker_acc = svm.get_account(&maker_ata_a).unwrap();
        let maker_state = spl_token_2022::state::Account::unpack(&maker_acc.data).unwrap();
        println!("Maker ATA balance: {}", maker_state.amount);
        assert_eq!(maker_state.amount, MAKER_STARTING_A - AMOUNT_TO_GIVE);

        let esc = svm.get_account(&escrow).unwrap();
        println!("Escrow account owner: {} (program? {})", esc.owner, esc.owner == program_id());
        println!("Escrow data len: {}", esc.data.len());
        let d = &esc.data;
        println!("  maker   = {}", Pubkey::new_from_array(d[0..32].try_into().unwrap()));
        println!("  mint_a  = {}", Pubkey::new_from_array(d[32..64].try_into().unwrap()));
        println!("  mint_b  = {}", Pubkey::new_from_array(d[64..96].try_into().unwrap()));
        println!("  receive = {}", u64::from_le_bytes(d[96..104].try_into().unwrap()));
        println!("  give    = {}", u64::from_le_bytes(d[104..112].try_into().unwrap()));
        println!("  bump    = {}", d[112]);
        assert_eq!(&d[0..32], maker.pubkey().as_ref());
        assert_eq!(u64::from_le_bytes(d[96..104].try_into().unwrap()), AMOUNT_TO_RECEIVE);
        assert_eq!(u64::from_le_bytes(d[104..112].try_into().unwrap()), AMOUNT_TO_GIVE);
        assert_eq!(d[112], bump);
    }

    #[test]
    pub fn test_take_instruction() {
        let (mut svm, maker, mint_a, mint_b, escrow, _bump, vault) = make_escrow();
        let program_id = program_id();

        let taker = Keypair::new();
        svm.airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut svm, &taker, &mint_b)
            .owner(&taker.pubkey()).send().unwrap();
        MintTo::new(&mut svm, &maker, &mint_b, &taker_ata_b, AMOUNT_TO_RECEIVE)
            .send()
            .unwrap();

        // Not created yet - the program creates these with CreateIdempotent.
        let taker_ata_a = spl_associated_token_account::get_associated_token_address(&taker.pubkey(), &mint_a);
        let maker_ata_b = spl_associated_token_account::get_associated_token_address(&maker.pubkey(), &mint_b);

        let associated_token_program = ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();
        let token_program = TOKEN_PROGRAM_ID;
        let system_program = solana_sdk_ids::system_program::ID;

        let take_ix = Instruction {
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
                AccountMeta::new_readonly(system_program, false),
                AccountMeta::new_readonly(token_program, false),
                AccountMeta::new_readonly(associated_token_program, false),
            ],
            data: vec![1u8],
        };

        let maker_lamports_before = svm.get_account(&maker.pubkey()).unwrap().lamports;

        let message = Message::new(&[take_ix], Some(&taker.pubkey()));
        let recent_blockhash = svm.latest_blockhash();
        let transaction = Transaction::new(&[&taker], message, recent_blockhash);
        let tx = svm.send_transaction(transaction).unwrap();

        println!("\nTake transaction successful");
        println!("CUs Consumed: {}", tx.compute_units_consumed);

        let taker_ata_a_acc = svm.get_account(&taker_ata_a).unwrap();
        let taker_ata_a_state = spl_token_2022::state::Account::unpack(&taker_ata_a_acc.data).unwrap();
        assert_eq!(taker_ata_a_state.amount, AMOUNT_TO_GIVE, "taker should receive all 500 A");

        let maker_ata_b_acc = svm.get_account(&maker_ata_b).unwrap();
        let maker_ata_b_state = spl_token_2022::state::Account::unpack(&maker_ata_b_acc.data).unwrap();
        assert_eq!(maker_ata_b_state.amount, AMOUNT_TO_RECEIVE, "maker should receive the 100 B payment");

        assert_closed(&svm, &vault);
        assert_closed(&svm, &escrow);

        let maker_lamports_after = svm.get_account(&maker.pubkey()).unwrap().lamports;
        assert!(
            maker_lamports_after > maker_lamports_before,
            "maker should receive the rent refunded from both closed accounts"
        );
    }

    #[test]
    pub fn test_cancel_instruction() {
        let (mut svm, maker, mint_a, _mint_b, escrow, _bump, vault) = make_escrow();
        let program_id = program_id();

        let maker_ata_a = spl_associated_token_account::get_associated_token_address(&maker.pubkey(), &mint_a);
        let token_program = TOKEN_PROGRAM_ID;

        let cancel_ix = Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new(maker.pubkey(), true),
                AccountMeta::new_readonly(mint_a, false),
                AccountMeta::new(escrow, false),
                AccountMeta::new(vault, false),
                AccountMeta::new(maker_ata_a, false),
                AccountMeta::new_readonly(token_program, false),
            ],
            data: vec![2u8],
        };

        let message = Message::new(&[cancel_ix], Some(&maker.pubkey()));
        let recent_blockhash = svm.latest_blockhash();
        let transaction = Transaction::new(&[&maker], message, recent_blockhash);
        let tx = svm.send_transaction(transaction).unwrap();

        println!("\nCancel transaction successful");
        println!("CUs Consumed: {}", tx.compute_units_consumed);

        let maker_ata_a_acc = svm.get_account(&maker_ata_a).unwrap();
        let maker_ata_a_state = spl_token_2022::state::Account::unpack(&maker_ata_a_acc.data).unwrap();
        assert_eq!(maker_ata_a_state.amount, MAKER_STARTING_A, "maker should get all 1000 A back");

        assert_closed(&svm, &vault);
        assert_closed(&svm, &escrow);
    }

    #[test]
    pub fn test_take_instruction_fails_when_taker_is_underfunded() {
        let (mut svm, maker, mint_a, mint_b, escrow, _bump, vault) = make_escrow();
        let program_id = program_id();

        let taker = Keypair::new();
        svm.airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut svm, &taker, &mint_b)
            .owner(&taker.pubkey()).send().unwrap();
        // Only half of what Take requires (100 B).
        MintTo::new(&mut svm, &maker, &mint_b, &taker_ata_b, AMOUNT_TO_RECEIVE / 2)
            .send()
            .unwrap();

        let taker_ata_a = spl_associated_token_account::get_associated_token_address(&taker.pubkey(), &mint_a);
        let maker_ata_b = spl_associated_token_account::get_associated_token_address(&maker.pubkey(), &mint_b);

        let associated_token_program = ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();
        let token_program = TOKEN_PROGRAM_ID;
        let system_program = solana_sdk_ids::system_program::ID;

        let take_ix = Instruction {
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
                AccountMeta::new_readonly(system_program, false),
                AccountMeta::new_readonly(token_program, false),
                AccountMeta::new_readonly(associated_token_program, false),
            ],
            data: vec![1u8],
        };

        let message = Message::new(&[take_ix], Some(&taker.pubkey()));
        let recent_blockhash = svm.latest_blockhash();
        let transaction = Transaction::new(&[&taker], message, recent_blockhash);
        let result = svm.send_transaction(transaction);

        assert!(result.is_err(), "a taker who cannot afford the price must not be able to take the escrow");

        // Nothing should have moved: the vault still holds all 500 A.
        let vault_acc = svm.get_account(&vault).unwrap();
        let vault_state = spl_token_2022::state::Account::unpack(&vault_acc.data).unwrap();
        assert_eq!(vault_state.amount, AMOUNT_TO_GIVE);
    }

    #[test]
    pub fn test_cancel_instruction_fails_for_a_stranger() {
        let (mut svm, maker, mint_a, _mint_b, escrow, _bump, vault) = make_escrow();
        let program_id = program_id();

        let stranger = Keypair::new();
        svm.airdrop(&stranger.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

        // The stranger signs and passes themselves in the `maker` slot,
        // hoping to drain someone else's escrow into their own wallet.
        let stranger_ata_a = spl_associated_token_account::get_associated_token_address(&stranger.pubkey(), &mint_a);
        let token_program = TOKEN_PROGRAM_ID;

        let cancel_ix = Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new(stranger.pubkey(), true),
                AccountMeta::new_readonly(mint_a, false),
                AccountMeta::new(escrow, false),
                AccountMeta::new(vault, false),
                AccountMeta::new(stranger_ata_a, false),
                AccountMeta::new_readonly(token_program, false),
            ],
            data: vec![2u8],
        };

        let message = Message::new(&[cancel_ix], Some(&stranger.pubkey()));
        let recent_blockhash = svm.latest_blockhash();
        let transaction = Transaction::new(&[&stranger], message, recent_blockhash);
        let result = svm.send_transaction(transaction);

        assert!(result.is_err(), "a stranger must not be able to cancel someone else's escrow");

        // The vault must still hold all 500 A - nothing was drained.
        let vault_acc = svm.get_account(&vault).unwrap();
        let vault_state = spl_token_2022::state::Account::unpack(&vault_acc.data).unwrap();
        assert_eq!(vault_state.amount, AMOUNT_TO_GIVE);

        // And the real maker's escrow is still open, still owned by the program.
        let escrow_acc = svm.get_account(&escrow).unwrap();
        assert_eq!(escrow_acc.owner, program_id);
        let _ = maker; // kept for symmetry with the other tests / clarity that this is *not* the maker's tx
    }
}
