#[cfg(test)]
mod tests {

    use std::path::PathBuf;

    use litesvm::LiteSVM;
    use litesvm_token::{
        spl_token::{self},
        CreateAssociatedTokenAccount,
        CreateMint,
        MintTo,
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
    const ASSOCIATED_TOKEN_PROGRAM_ID: &str =
        "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL";

    const AMOUNT_TO_RECEIVE: u64 = 100_000_000; // 100 B
    const AMOUNT_TO_GIVE: u64 = 500_000_000; // 500 A
    const MAKER_INITIAL_A: u64 = 1_000_000_000; // 1000 A

    fn program_id() -> Pubkey {
        Pubkey::from(crate::ID)
    }

    fn setup() -> (LiteSVM, Keypair) {
        let mut svm = LiteSVM::new();
        let payer = Keypair::new();

        // LiteSVM 0.9 still ships the pre-SIMD-0194 Rent sysvar.
        // Pinocchio 0.11 expects the live folded rent rate.
        #[allow(deprecated)]
        svm.set_sysvar(&solana_rent::Rent {
            lamports_per_byte_year: 6960,
            exemption_threshold: 1.0,
            burn_percent: 50,
        });

        svm.airdrop(&payer.pubkey(), 10 * LAMPORTS_PER_SOL)
            .expect("Airdrop failed");

        let so_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/deploy/escrow.so");

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

    // Runs the common Make setup used by Take and Cancel tests.
    //
    // Returns:
    // svm, maker, mint_a, mint_b, escrow PDA, bump, vault, maker ATA A
    fn make_setup() -> (
        LiteSVM,
        Keypair,
        Pubkey,
        Pubkey,
        Pubkey,
        u8,
        Pubkey,
        Pubkey,
    ) {
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

        let maker_ata_a = CreateAssociatedTokenAccount::new(
            &mut svm,
            &maker,
            &mint_a,
        )
        .owner(&maker.pubkey())
        .send()
        .unwrap();

        let (escrow, bump) = Pubkey::find_program_address(
            &[b"escrow", maker.pubkey().as_ref()],
            &program_id,
        );

        let vault =
            spl_associated_token_account::get_associated_token_address(
                &escrow,
                &mint_a,
            );

        let associated_token_program =
            ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();

        let token_program = TOKEN_PROGRAM_ID;
        let system_program = solana_sdk_ids::system_program::ID;

        // Give the maker 1000 A.
        MintTo::new(
            &mut svm,
            &maker,
            &mint_a,
            &maker_ata_a,
            MAKER_INITIAL_A,
        )
        .send()
        .unwrap();

        let make_data = [
            vec![0u8],
            AMOUNT_TO_RECEIVE.to_le_bytes().to_vec(),
            AMOUNT_TO_GIVE.to_le_bytes().to_vec(),
        ]
        .concat();

        let make_ix = Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new(maker.pubkey(), true),
                AccountMeta::new(mint_a, false),
                AccountMeta::new(mint_b, false),
                AccountMeta::new(escrow, false),
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

        let transaction =
            Transaction::new(&[&maker], message, recent_blockhash);

        let tx = svm.send_transaction(transaction).unwrap();

        println!(
            "\nMake CUs Consumed: {}",
            tx.compute_units_consumed
        );

        // Verify Make actually deposited 500 A.
        let vault_acc = svm.get_account(&vault).unwrap();
        let vault_state =
            spl_token_2022::state::Account::unpack(&vault_acc.data).unwrap();

        assert_eq!(vault_state.amount, AMOUNT_TO_GIVE);

        // Maker should now have 500 A.
        let maker_acc = svm.get_account(&maker_ata_a).unwrap();
        let maker_state =
            spl_token_2022::state::Account::unpack(&maker_acc.data).unwrap();

        assert_eq!(
            maker_state.amount,
            MAKER_INITIAL_A - AMOUNT_TO_GIVE
        );

        (
            svm,
            maker,
            mint_a,
            mint_b,
            escrow,
            bump,
            vault,
            maker_ata_a,
        )
    }

    #[test]
    pub fn test_make_instruction() {
        let (
            svm,
            maker,
            mint_a,
            mint_b,
            escrow,
            bump,
            vault,
            maker_ata_a,
        ) = make_setup();

        let program_id = program_id();

        assert_eq!(program_id.to_string(), PROGRAM_ID);

        println!("Mint A: {}", mint_a);
        println!("Mint B: {}", mint_b);
        println!("Maker ATA A: {}", maker_ata_a);
        println!("Escrow PDA: {}", escrow);
        println!("Vault PDA: {}", vault);
        println!("Bump: {}", bump);

        // Read escrow state back.
        let esc = svm.get_account(&escrow).unwrap();

        assert_eq!(esc.owner, program_id);
        assert_eq!(esc.data.len(), 113);

        let d = &esc.data;

        println!(
            "Escrow maker: {}",
            Pubkey::new_from_array(
                d[0..32].try_into().unwrap()
            )
        );

        println!(
            "Escrow mint_a: {}",
            Pubkey::new_from_array(
                d[32..64].try_into().unwrap()
            )
        );

        println!(
            "Escrow mint_b: {}",
            Pubkey::new_from_array(
                d[64..96].try_into().unwrap()
            )
        );

        println!(
            "Escrow receive: {}",
            u64::from_le_bytes(
                d[96..104].try_into().unwrap()
            )
        );

        println!(
            "Escrow give: {}",
            u64::from_le_bytes(
                d[104..112].try_into().unwrap()
            )
        );

        println!("Escrow bump: {}", d[112]);

        assert_eq!(&d[0..32], maker.pubkey().as_ref());
        assert_eq!(
            u64::from_le_bytes(d[96..104].try_into().unwrap()),
            AMOUNT_TO_RECEIVE
        );
        assert_eq!(
            u64::from_le_bytes(d[104..112].try_into().unwrap()),
            AMOUNT_TO_GIVE
        );
        assert_eq!(d[112], bump);

        let vault_acc = svm.get_account(&vault).unwrap();
        let vault_state =
            spl_token_2022::state::Account::unpack(&vault_acc.data).unwrap();

        assert_eq!(vault_state.amount, AMOUNT_TO_GIVE);
    }

    #[test]
    pub fn test_take_instruction() {
        let (
            mut svm,
            maker,
            mint_a,
            mint_b,
            escrow,
            _bump,
            vault,
            _maker_ata_a,
        ) = make_setup();

        let taker = Keypair::new();

        svm.airdrop(&taker.pubkey(), 2 * LAMPORTS_PER_SOL)
            .unwrap();

        // Create taker's B account and give the taker 100 B.
        let taker_ata_b = CreateAssociatedTokenAccount::new(
            &mut svm,
            &taker,
            &mint_b,
        )
        .owner(&taker.pubkey())
        .send()
        .unwrap();

        MintTo::new(
            &mut svm,
            &maker,
            &mint_b,
            &taker_ata_b,
            AMOUNT_TO_RECEIVE,
        )
        .send()
        .unwrap();

        // These are intentionally NOT created.
        // Take must create them with CreateIdempotent.
        let taker_ata_a =
            spl_associated_token_account::get_associated_token_address(
                &taker.pubkey(),
                &mint_a,
            );

        let maker_ata_b =
            spl_associated_token_account::get_associated_token_address(
                &maker.pubkey(),
                &mint_b,
            );

        let system_program = solana_sdk_ids::system_program::ID;
        let token_program = TOKEN_PROGRAM_ID;
        let associated_token_program =
            ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();

        let take_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                // 0 taker
                AccountMeta::new(taker.pubkey(), true),
                // 1 maker
                AccountMeta::new(maker.pubkey(), false),
                // 2 mint A
                AccountMeta::new(mint_a, false),
                // 3 mint B
                AccountMeta::new(mint_b, false),
                // 4 escrow
                AccountMeta::new(escrow, false),
                // 5 vault
                AccountMeta::new(vault, false),
                // 6 taker ATA A
                AccountMeta::new(taker_ata_a, false),
                // 7 taker ATA B
                AccountMeta::new(taker_ata_b, false),
                // 8 maker ATA B
                AccountMeta::new(maker_ata_b, false),
                // 9 system
                AccountMeta::new(system_program, false),
                // 10 token
                AccountMeta::new(token_program, false),
                // 11 associated token
                AccountMeta::new(associated_token_program, false),
            ],
            data: vec![1u8],
        };

        let message =
            Message::new(&[take_ix], Some(&taker.pubkey()));

        let recent_blockhash = svm.latest_blockhash();

        let transaction =
            Transaction::new(&[&taker], message, recent_blockhash);

        let tx = svm.send_transaction(transaction).unwrap();

        println!(
            "\nTake CUs Consumed: {}",
            tx.compute_units_consumed
        );

        // Taker received 500 A.
        let taker_a_acc = svm.get_account(&taker_ata_a).unwrap();
        let taker_a_state =
            spl_token_2022::state::Account::unpack(
                &taker_a_acc.data
            )
            .unwrap();

        assert_eq!(taker_a_state.amount, AMOUNT_TO_GIVE);

        // Maker received 100 B.
        let maker_b_acc = svm.get_account(&maker_ata_b).unwrap();
        let maker_b_state =
            spl_token_2022::state::Account::unpack(
                &maker_b_acc.data
            )
            .unwrap();

        assert_eq!(
            maker_b_state.amount,
            AMOUNT_TO_RECEIVE
        );

        // Vault must be closed.
        if let Some(vault_acc) = svm.get_account(&vault) {
            assert_eq!(vault_acc.lamports, 0);
            assert_eq!(
                vault_acc.owner,
                solana_sdk_ids::system_program::ID
            );
        }

        // Escrow must be closed.
        if let Some(escrow_acc) = svm.get_account(&escrow) {
            assert_eq!(escrow_acc.lamports, 0);
            assert_eq!(
                escrow_acc.owner,
                solana_sdk_ids::system_program::ID
            );
        }
    }

    #[test]
    pub fn test_cancel_instruction() {
        let (
            mut svm,
            maker,
            mint_a,
            _mint_b,
            escrow,
            _bump,
            vault,
            maker_ata_a,
        ) = make_setup();

        let token_program = TOKEN_PROGRAM_ID;

        let cancel_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                // 0 maker
                AccountMeta::new(maker.pubkey(), true),
                // 1 mint A
                AccountMeta::new(mint_a, false),
                // 2 escrow
                AccountMeta::new(escrow, false),
                // 3 vault
                AccountMeta::new(vault, false),
                // 4 maker ATA A
                AccountMeta::new(maker_ata_a, false),
                // 5 token program
                AccountMeta::new(token_program, false),
            ],
            data: vec![2u8],
        };

        let message =
            Message::new(&[cancel_ix], Some(&maker.pubkey()));

        let recent_blockhash = svm.latest_blockhash();

        let transaction =
            Transaction::new(&[&maker], message, recent_blockhash);

        let tx = svm.send_transaction(transaction).unwrap();

        println!(
            "\nCancel CUs Consumed: {}",
            tx.compute_units_consumed
        );

        // Maker gets the original 500 A back.
        let maker_acc = svm.get_account(&maker_ata_a).unwrap();
        let maker_state =
            spl_token_2022::state::Account::unpack(
                &maker_acc.data
            )
            .unwrap();

        assert_eq!(
            maker_state.amount,
            MAKER_INITIAL_A
        );

        // Vault must be closed.
        if let Some(vault_acc) = svm.get_account(&vault) {
            assert_eq!(vault_acc.lamports, 0);
            assert_eq!(
                vault_acc.owner,
                solana_sdk_ids::system_program::ID
            );
        }

        // Escrow must be closed.
        if let Some(escrow_acc) = svm.get_account(&escrow) {
            assert_eq!(escrow_acc.lamports, 0);
            assert_eq!(
                escrow_acc.owner,
                solana_sdk_ids::system_program::ID
            );
        }
    }

    #[test]
    pub fn test_take_underfunded_taker_fails() {
        let (
            mut svm,
            maker,
            mint_a,
            mint_b,
            escrow,
            _bump,
            vault,
            _maker_ata_a,
        ) = make_setup();

        let taker = Keypair::new();

        svm.airdrop(&taker.pubkey(), 2 * LAMPORTS_PER_SOL)
            .unwrap();

        // Taker only gets 50 B, but the escrow requires 100 B.
        let taker_ata_b = CreateAssociatedTokenAccount::new(
            &mut svm,
            &taker,
            &mint_b,
        )
        .owner(&taker.pubkey())
        .send()
        .unwrap();

        MintTo::new(
            &mut svm,
            &maker,
            &mint_b,
            &taker_ata_b,
            50_000_000,
        )
        .send()
        .unwrap();

        let taker_ata_a =
            spl_associated_token_account::get_associated_token_address(
                &taker.pubkey(),
                &mint_a,
            );

        let maker_ata_b =
            spl_associated_token_account::get_associated_token_address(
                &maker.pubkey(),
                &mint_b,
            );

        let system_program = solana_sdk_ids::system_program::ID;
        let token_program = TOKEN_PROGRAM_ID;
        let associated_token_program =
            ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();

        let take_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(taker.pubkey(), true),
                AccountMeta::new(maker.pubkey(), false),
                AccountMeta::new(mint_a, false),
                AccountMeta::new(mint_b, false),
                AccountMeta::new(escrow, false),
                AccountMeta::new(vault, false),
                AccountMeta::new(taker_ata_a, false),
                AccountMeta::new(taker_ata_b, false),
                AccountMeta::new(maker_ata_b, false),
                AccountMeta::new(system_program, false),
                AccountMeta::new(token_program, false),
                AccountMeta::new(associated_token_program, false),
            ],
            data: vec![1u8],
        };

        let message =
            Message::new(&[take_ix], Some(&taker.pubkey()));

        let recent_blockhash = svm.latest_blockhash();

        let transaction =
            Transaction::new(&[&taker], message, recent_blockhash);

        let result = svm.send_transaction(transaction);

        assert!(
            result.is_err(),
            "Take must fail when the taker only has 50 B"
        );

        // The failed transaction must not have drained the vault.
        let vault_acc = svm.get_account(&vault).unwrap();
        let vault_state =
            spl_token_2022::state::Account::unpack(
                &vault_acc.data
            )
            .unwrap();

        assert_eq!(
            vault_state.amount,
            AMOUNT_TO_GIVE
        );

        println!(
            "\nUnderfunded Take correctly failed; no state was committed."
        );
    }

    #[test]
    pub fn test_cancel_by_stranger_fails() {
        let (
            mut svm,
            maker,
            mint_a,
            _mint_b,
            escrow,
            _bump,
            vault,
            _maker_ata_a,
        ) = make_setup();

        let stranger = Keypair::new();

        svm.airdrop(&stranger.pubkey(), LAMPORTS_PER_SOL)
            .unwrap();

        let token_program = TOKEN_PROGRAM_ID;

        // The stranger is supplied as the maker account.
        // The program must reject this because:
        // 1. the stranger signs, but
        // 2. escrow.maker() is the real maker.
        let cancel_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(stranger.pubkey(), true),
                AccountMeta::new(mint_a, false),
                AccountMeta::new(escrow, false),
                AccountMeta::new(vault, false),
                AccountMeta::new(
                    spl_associated_token_account::get_associated_token_address(
                        &stranger.pubkey(),
                        &mint_a,
                    ),
                    false,
                ),
                AccountMeta::new(token_program, false),
            ],
            data: vec![2u8],
        };

        let message =
            Message::new(&[cancel_ix], Some(&stranger.pubkey()));

        let recent_blockhash = svm.latest_blockhash();

        let transaction =
            Transaction::new(&[&stranger], message, recent_blockhash);

        let result = svm.send_transaction(transaction);

        assert!(
            result.is_err(),
            "A stranger must not be able to cancel the escrow"
        );

        // Prove the 500 A are still locked in the vault.
        let vault_acc = svm.get_account(&vault).unwrap();
        let vault_state =
            spl_token_2022::state::Account::unpack(
                &vault_acc.data
            )
            .unwrap();

        assert_eq!(
            vault_state.amount,
            AMOUNT_TO_GIVE
        );

        // Escrow must still exist.
        let escrow_acc = svm.get_account(&escrow).unwrap();

        assert_eq!(
            escrow_acc.owner,
            program_id()
        );

        println!(
            "\nStranger Cancel correctly failed; vault still contains {} A.",
            vault_state.amount
        );
    }
}