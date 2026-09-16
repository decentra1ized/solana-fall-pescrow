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

    const AMOUNT_TO_RECEIVE: u64 = 100_000_000; // 100 B
    const AMOUNT_TO_GIVE: u64 = 500_000_000; // 500 A
    const MAKER_INITIAL_A: u64 = 1_000_000_000; // 1,000 A

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

    /// Shared Make setup: two mints, funded maker ATA, escrow opened with 500 A for 100 B.
    fn setup_make() -> (LiteSVM, Keypair, Pubkey, Pubkey, Pubkey, u8, Pubkey) {
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

        let vault =
            spl_associated_token_account::get_associated_token_address(&escrow, &mint_a);

        let associated_token_program = ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();
        let token_program = TOKEN_PROGRAM_ID;
        let system_program = solana_sdk_ids::system_program::ID;

        MintTo::new(&mut svm, &maker, &mint_a, &maker_ata_a, MAKER_INITIAL_A)
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
        let transaction = Transaction::new(&[&maker], message, recent_blockhash);
        svm.send_transaction(transaction).unwrap();

        (svm, maker, mint_a, mint_b, escrow, bump, vault)
    }

    fn token_amount(svm: &LiteSVM, ata: &Pubkey) -> u64 {
        let acc = svm.get_account(ata).unwrap();
        spl_token_2022::state::Account::unpack(&acc.data)
            .unwrap()
            .amount
    }

    fn assert_account_closed(svm: &LiteSVM, address: &Pubkey) {
        match svm.get_account(address) {
            None => {}
            Some(acc) => {
                assert_eq!(acc.lamports, 0, "closed account should have 0 lamports");
                assert_eq!(
                    acc.owner,
                    solana_sdk_ids::system_program::ID,
                    "closed account should be owned by system program"
                );
            }
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

        let maker_ata_a = CreateAssociatedTokenAccount::new(&mut svm, &payer, &mint_a)
            .owner(&payer.pubkey())
            .send()
            .unwrap();
        println!("Maker ATA A: {}\n", maker_ata_a);

        let escrow = Pubkey::find_program_address(
            &[b"escrow".as_ref(), payer.pubkey().as_ref()],
            &PROGRAM_ID.parse().unwrap(),
        );
        println!("Escrow PDA: {}\n", escrow.0);

        let vault = spl_associated_token_account::get_associated_token_address(&escrow.0, &mint_a);
        println!("Vault PDA: {}\n", vault);

        let associated_token_program = ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();
        let token_program = TOKEN_PROGRAM_ID;
        let system_program = solana_sdk_ids::system_program::ID;

        MintTo::new(&mut svm, &payer, &mint_a, &maker_ata_a, MAKER_INITIAL_A)
            .send()
            .unwrap();

        let bump: u8 = escrow.1;
        println!("Bump: {}", bump);

        let make_data = [
            vec![0u8],
            AMOUNT_TO_RECEIVE.to_le_bytes().to_vec(),
            AMOUNT_TO_GIVE.to_le_bytes().to_vec(),
        ]
        .concat();
        let make_ix = Instruction {
            program_id,
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

        let message = Message::new(&[make_ix], Some(&payer.pubkey()));
        let recent_blockhash = svm.latest_blockhash();
        let transaction = Transaction::new(&[&payer], message, recent_blockhash);
        let tx = svm.send_transaction(transaction).unwrap();

        println!("\n\nMake transaction successful");
        println!("CUs Consumed: {}", tx.compute_units_consumed);

        let vault_acc = svm.get_account(&vault).unwrap();
        let vault_state = spl_token_2022::state::Account::unpack(&vault_acc.data).unwrap();
        println!(
            "Vault owner: {} (escrow PDA? {})",
            vault_state.owner,
            vault_state.owner == escrow.0
        );
        println!("Vault balance: {}", vault_state.amount);
        assert_eq!(vault_state.amount, AMOUNT_TO_GIVE);

        let maker_acc = svm.get_account(&maker_ata_a).unwrap();
        let maker_state = spl_token_2022::state::Account::unpack(&maker_acc.data).unwrap();
        println!("Maker ATA balance: {}", maker_state.amount);
        assert_eq!(maker_state.amount, MAKER_INITIAL_A - AMOUNT_TO_GIVE);

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
            AMOUNT_TO_RECEIVE
        );
        assert_eq!(
            u64::from_le_bytes(d[104..112].try_into().unwrap()),
            AMOUNT_TO_GIVE
        );
        assert_eq!(d[112], bump);
    }

    #[test]
    pub fn test_take_instruction() {
        let (mut svm, maker, mint_a, mint_b, escrow, _bump, vault) = setup_make();
        let program_id = program_id();

        let taker = Keypair::new();
        svm.airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL)
            .expect("taker airdrop failed");

        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut svm, &taker, &mint_b)
            .owner(&taker.pubkey())
            .send()
            .unwrap();
        MintTo::new(&mut svm, &maker, &mint_b, &taker_ata_b, AMOUNT_TO_RECEIVE)
            .send()
            .unwrap();

        let taker_ata_a =
            spl_associated_token_account::get_associated_token_address(&taker.pubkey(), &mint_a);
        let maker_ata_b =
            spl_associated_token_account::get_associated_token_address(&maker.pubkey(), &mint_b);

        let associated_token_program = ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();
        let token_program = TOKEN_PROGRAM_ID;
        let system_program = solana_sdk_ids::system_program::ID;

        let escrow_lamports = svm.get_account(&escrow).unwrap().lamports;
        let vault_lamports = svm.get_account(&vault).unwrap().lamports;
        let maker_lamports_before = svm.get_account(&maker.pubkey()).unwrap().lamports;

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
        let tx = svm.send_transaction(transaction).unwrap();

        println!("Take CUs Consumed: {}", tx.compute_units_consumed);

        assert_eq!(token_amount(&svm, &taker_ata_a), AMOUNT_TO_GIVE);
        assert_eq!(token_amount(&svm, &maker_ata_b), AMOUNT_TO_RECEIVE);
        assert_account_closed(&svm, &vault);
        assert_account_closed(&svm, &escrow);

        let maker_lamports_after = svm.get_account(&maker.pubkey()).unwrap().lamports;
        assert_eq!(
            maker_lamports_after,
            maker_lamports_before + escrow_lamports + vault_lamports,
            "maker should reclaim rent from escrow + vault"
        );
    }

    #[test]
    pub fn test_cancel_instruction() {
        let (mut svm, maker, mint_a, _mint_b, escrow, _bump, vault) = setup_make();
        let program_id = program_id();

        let maker_ata_a =
            spl_associated_token_account::get_associated_token_address(&maker.pubkey(), &mint_a);
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

        println!("Cancel CUs Consumed: {}", tx.compute_units_consumed);

        assert_eq!(token_amount(&svm, &maker_ata_a), MAKER_INITIAL_A);
        assert_account_closed(&svm, &vault);
        assert_account_closed(&svm, &escrow);
    }

    #[test]
    pub fn test_take_fails_when_taker_underfunded() {
        let (mut svm, maker, mint_a, mint_b, escrow, _bump, vault) = setup_make();
        let program_id = program_id();

        let taker = Keypair::new();
        svm.airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL)
            .expect("taker airdrop failed");

        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut svm, &taker, &mint_b)
            .owner(&taker.pubkey())
            .send()
            .unwrap();
        // Only 50 B — escrow wants 100 B.
        MintTo::new(&mut svm, &maker, &mint_b, &taker_ata_b, 50_000_000)
            .send()
            .unwrap();

        let taker_ata_a =
            spl_associated_token_account::get_associated_token_address(&taker.pubkey(), &mint_a);
        let maker_ata_b =
            spl_associated_token_account::get_associated_token_address(&maker.pubkey(), &mint_b);

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
        assert!(result.is_err(), "underfunded take must fail");

        let vault_acc = svm.get_account(&vault).unwrap();
        let vault_state = spl_token_2022::state::Account::unpack(&vault_acc.data).unwrap();
        assert_eq!(vault_state.amount, AMOUNT_TO_GIVE);
        assert!(svm.get_account(&escrow).is_some());
    }

    #[test]
    pub fn test_cancel_fails_when_signed_by_stranger() {
        let (mut svm, maker, mint_a, _mint_b, escrow, _bump, vault) = setup_make();
        let program_id = program_id();

        let stranger = Keypair::new();
        svm.airdrop(&stranger.pubkey(), 10 * LAMPORTS_PER_SOL)
            .expect("stranger airdrop failed");

        let maker_ata_a =
            spl_associated_token_account::get_associated_token_address(&maker.pubkey(), &mint_a);
        let token_program = TOKEN_PROGRAM_ID;

        // Stranger in the maker slot as signer — fails maker check (and would fail
        // signer check if maker were listed without signing).
        let cancel_ix = Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new(stranger.pubkey(), true),
                AccountMeta::new_readonly(mint_a, false),
                AccountMeta::new(escrow, false),
                AccountMeta::new(vault, false),
                AccountMeta::new(maker_ata_a, false),
                AccountMeta::new_readonly(token_program, false),
            ],
            data: vec![2u8],
        };

        let message = Message::new(&[cancel_ix], Some(&stranger.pubkey()));
        let recent_blockhash = svm.latest_blockhash();
        let transaction = Transaction::new(&[&stranger], message, recent_blockhash);
        let result = svm.send_transaction(transaction);
        assert!(result.is_err(), "stranger cancel must fail");

        let vault_acc = svm.get_account(&vault).unwrap();
        let vault_state = spl_token_2022::state::Account::unpack(&vault_acc.data).unwrap();
        assert_eq!(
            vault_state.amount, AMOUNT_TO_GIVE,
            "500 A must still be in the vault"
        );
    }
}
