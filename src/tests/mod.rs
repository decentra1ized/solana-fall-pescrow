#[cfg(test)]
mod tests {

    mod cancel;
    mod make;

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

    fn make_escrow() -> (LiteSVM, Keypair, Pubkey, Pubkey, Pubkey, Pubkey, Pubkey) {
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
        MintTo::new(&mut svm, &maker, &mint_a, &maker_ata_a, 1_000_000_000)
            .send()
            .unwrap();

        let escrow =
            Pubkey::find_program_address(&[b"escrow", maker.pubkey().as_ref()], &program_id());
        let vault = spl_associated_token_account::get_associated_token_address(&escrow.0, &mint_a);
        let make_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(maker.pubkey(), true),
                AccountMeta::new_readonly(mint_a, false),
                AccountMeta::new_readonly(mint_b, false),
                AccountMeta::new(escrow.0, false),
                AccountMeta::new(maker_ata_a, false),
                AccountMeta::new(vault, false),
                AccountMeta::new_readonly(solana_sdk_ids::system_program::ID, false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
                AccountMeta::new_readonly(ASSOCIATED_TOKEN_PROGRAM_ID.parse().unwrap(), false),
            ],
            data: [
                vec![0u8],
                100_000_000u64.to_le_bytes().to_vec(),
                500_000_000u64.to_le_bytes().to_vec(),
            ]
            .concat(),
        };
        let message = Message::new(&[make_ix], Some(&maker.pubkey()));
        let transaction = Transaction::new(&[&maker], message, svm.latest_blockhash());
        svm.send_transaction(transaction).unwrap();

        (svm, maker, mint_a, mint_b, maker_ata_a, escrow.0, vault)
    }

    fn token_amount(svm: &LiteSVM, address: &Pubkey) -> u64 {
        let account = svm.get_account(address).unwrap();
        spl_token_2022::state::Account::unpack(&account.data)
            .unwrap()
            .amount
    }

    fn assert_closed(svm: &LiteSVM, address: &Pubkey) {
        if let Some(account) = svm.get_account(address) {
            assert_eq!(account.lamports, 0);
            assert_eq!(account.owner, solana_sdk_ids::system_program::ID);
        }
    }

    #[test]
    fn test_take_instruction() {
        let (mut svm, maker, mint_a, mint_b, _maker_ata_a, escrow, vault) = make_escrow();
        let taker = Keypair::new();
        svm.airdrop(&taker.pubkey(), LAMPORTS_PER_SOL).unwrap();

        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut svm, &taker, &mint_b)
            .owner(&taker.pubkey())
            .send()
            .unwrap();
        MintTo::new(&mut svm, &maker, &mint_b, &taker_ata_b, 100_000_000)
            .send()
            .unwrap();

        let taker_ata_a =
            spl_associated_token_account::get_associated_token_address(&taker.pubkey(), &mint_a);
        let maker_ata_b =
            spl_associated_token_account::get_associated_token_address(&maker.pubkey(), &mint_b);
        let ix = Instruction {
            program_id: program_id(),
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
                AccountMeta::new_readonly(ASSOCIATED_TOKEN_PROGRAM_ID.parse().unwrap(), false),
            ],
            data: vec![1u8],
        };
        let message = Message::new(&[ix], Some(&taker.pubkey()));
        let transaction = Transaction::new(&[&taker], message, svm.latest_blockhash());
        let result = svm.send_transaction(transaction).unwrap();
        println!("Take CUs Consumed: {}", result.compute_units_consumed);

        assert_eq!(token_amount(&svm, &taker_ata_a), 500_000_000);
        assert_eq!(token_amount(&svm, &maker_ata_b), 100_000_000);
        assert_closed(&svm, &vault);
        assert_closed(&svm, &escrow);
    }

    #[test]
    fn test_take_rejects_underfunded_taker() {
        let (mut svm, maker, mint_a, mint_b, _maker_ata_a, escrow, vault) = make_escrow();
        let taker = Keypair::new();
        svm.airdrop(&taker.pubkey(), LAMPORTS_PER_SOL).unwrap();
        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut svm, &taker, &mint_b)
            .owner(&taker.pubkey())
            .send()
            .unwrap();
        MintTo::new(&mut svm, &maker, &mint_b, &taker_ata_b, 50_000_000)
            .send()
            .unwrap();
        let taker_ata_a =
            spl_associated_token_account::get_associated_token_address(&taker.pubkey(), &mint_a);
        let maker_ata_b =
            spl_associated_token_account::get_associated_token_address(&maker.pubkey(), &mint_b);
        let ix = Instruction {
            program_id: program_id(),
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
                AccountMeta::new_readonly(ASSOCIATED_TOKEN_PROGRAM_ID.parse().unwrap(), false),
            ],
            data: vec![1u8],
        };
        let message = Message::new(&[ix], Some(&taker.pubkey()));
        let transaction = Transaction::new(&[&taker], message, svm.latest_blockhash());

        assert!(svm.send_transaction(transaction).is_err());
        assert_eq!(token_amount(&svm, &vault), 500_000_000);
        assert!(svm.get_account(&escrow).is_some());
    }
}
