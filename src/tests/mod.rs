#[cfg(test)]
#[allow(clippy::module_inception)]
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
    const MAKER_STARTING_A: u64 = 1_000_000_000;
    const AMOUNT_TO_RECEIVE: u64 = 100_000_000;
    const AMOUNT_TO_GIVE: u64 = 500_000_000;

    struct EscrowFixture {
        svm: LiteSVM,
        maker: Keypair,
        mint_a: Pubkey,
        mint_b: Pubkey,
        maker_ata_a: Pubkey,
        escrow: Pubkey,
        bump: u8,
        vault: Pubkey,
    }

    fn program_id() -> Pubkey {
        crate::ID
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

    fn make_escrow() -> EscrowFixture {
        let (mut svm, maker) = setup();
        assert_eq!(program_id().to_string(), PROGRAM_ID);

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
        MintTo::new(&mut svm, &maker, &mint_a, &maker_ata_a, MAKER_STARTING_A)
            .send()
            .unwrap();

        let (escrow, bump) =
            Pubkey::find_program_address(&[b"escrow", maker.pubkey().as_ref()], &program_id());
        let vault = spl_associated_token_account::get_associated_token_address(&escrow, &mint_a);

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
                AccountMeta::new(associated_token_program(), false),
            ],
            data: make_data,
        };
        let message = Message::new(&[make_ix], Some(&maker.pubkey()));
        let transaction = Transaction::new(&[&maker], message, svm.latest_blockhash());
        let tx = svm.send_transaction(transaction).unwrap();
        println!("Make CUs Consumed: {}", tx.compute_units_consumed);

        EscrowFixture {
            svm,
            maker,
            mint_a,
            mint_b,
            maker_ata_a,
            escrow,
            bump,
            vault,
        }
    }

    fn associated_token_program() -> Pubkey {
        ASSOCIATED_TOKEN_PROGRAM_ID.parse().unwrap()
    }

    fn token_balance(svm: &LiteSVM, address: &Pubkey) -> u64 {
        let account = svm.get_account(address).expect("token account must exist");
        spl_token_2022::state::Account::unpack(&account.data)
            .expect("valid token account")
            .amount
    }

    fn assert_closed(svm: &LiteSVM, address: &Pubkey) {
        if let Some(account) = svm.get_account(address) {
            assert_eq!(account.lamports, 0, "closed account retained lamports");
            assert_eq!(
                account.owner,
                solana_sdk_ids::system_program::ID,
                "closed account retained its former owner"
            );
        }
    }

    fn create_taker_with_b(
        svm: &mut LiteSVM,
        maker: &Keypair,
        mint_b: &Pubkey,
        amount_b: u64,
    ) -> (Keypair, Pubkey) {
        let taker = Keypair::new();
        svm.airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL)
            .expect("taker airdrop failed");
        let taker_ata_b = CreateAssociatedTokenAccount::new(svm, &taker, mint_b)
            .owner(&taker.pubkey())
            .send()
            .unwrap();
        MintTo::new(svm, maker, mint_b, &taker_ata_b, amount_b)
            .send()
            .unwrap();
        (taker, taker_ata_b)
    }

    fn take_instruction(
        fixture: &EscrowFixture,
        taker: &Keypair,
        taker_ata_b: Pubkey,
    ) -> (Instruction, Pubkey, Pubkey) {
        let taker_ata_a = spl_associated_token_account::get_associated_token_address(
            &taker.pubkey(),
            &fixture.mint_a,
        );
        let maker_ata_b = spl_associated_token_account::get_associated_token_address(
            &fixture.maker.pubkey(),
            &fixture.mint_b,
        );
        let instruction = Instruction {
            program_id: program_id(),
            // Keep this order synchronized with src/instructions/take.rs.
            accounts: vec![
                AccountMeta::new(taker.pubkey(), true),
                AccountMeta::new(fixture.maker.pubkey(), false),
                AccountMeta::new_readonly(fixture.mint_a, false),
                AccountMeta::new_readonly(fixture.mint_b, false),
                AccountMeta::new(fixture.escrow, false),
                AccountMeta::new(fixture.vault, false),
                AccountMeta::new(taker_ata_a, false),
                AccountMeta::new(taker_ata_b, false),
                AccountMeta::new(maker_ata_b, false),
                AccountMeta::new_readonly(solana_sdk_ids::system_program::ID, false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
                AccountMeta::new_readonly(associated_token_program(), false),
            ],
            data: vec![1u8],
        };
        (instruction, taker_ata_a, maker_ata_b)
    }

    #[test]
    fn test_make_instruction() {
        let fixture = make_escrow();

        let vault_account = fixture.svm.get_account(&fixture.vault).unwrap();
        let vault_state = spl_token_2022::state::Account::unpack(&vault_account.data).unwrap();
        assert_eq!(vault_state.owner, fixture.escrow);
        assert_eq!(vault_state.mint, fixture.mint_a);
        assert_eq!(vault_state.amount, AMOUNT_TO_GIVE);
        assert_eq!(
            token_balance(&fixture.svm, &fixture.maker_ata_a),
            MAKER_STARTING_A - AMOUNT_TO_GIVE
        );

        let escrow_account = fixture.svm.get_account(&fixture.escrow).unwrap();
        assert_eq!(escrow_account.owner, program_id());
        assert_eq!(escrow_account.data.len(), 113);
        let data = escrow_account.data;
        assert_eq!(&data[0..32], fixture.maker.pubkey().as_ref());
        assert_eq!(&data[32..64], fixture.mint_a.as_ref());
        assert_eq!(&data[64..96], fixture.mint_b.as_ref());
        assert_eq!(
            u64::from_le_bytes(data[96..104].try_into().unwrap()),
            AMOUNT_TO_RECEIVE
        );
        assert_eq!(
            u64::from_le_bytes(data[104..112].try_into().unwrap()),
            AMOUNT_TO_GIVE
        );
        assert_eq!(data[112], fixture.bump);
    }

    #[test]
    fn test_take_instruction() {
        let mut fixture = make_escrow();
        let (taker, taker_ata_b) = create_taker_with_b(
            &mut fixture.svm,
            &fixture.maker,
            &fixture.mint_b,
            AMOUNT_TO_RECEIVE,
        );
        let (take_ix, taker_ata_a, maker_ata_b) = take_instruction(&fixture, &taker, taker_ata_b);

        let maker_balance_before = fixture.svm.get_balance(&fixture.maker.pubkey()).unwrap();
        let expected_refund = fixture.svm.get_account(&fixture.vault).unwrap().lamports
            + fixture.svm.get_account(&fixture.escrow).unwrap().lamports;
        let message = Message::new(&[take_ix], Some(&taker.pubkey()));
        let transaction = Transaction::new(&[&taker], message, fixture.svm.latest_blockhash());
        let tx = fixture.svm.send_transaction(transaction).unwrap();
        println!("Take CUs Consumed: {}", tx.compute_units_consumed);

        assert_eq!(token_balance(&fixture.svm, &taker_ata_a), AMOUNT_TO_GIVE);
        assert_eq!(token_balance(&fixture.svm, &maker_ata_b), AMOUNT_TO_RECEIVE);
        assert_eq!(token_balance(&fixture.svm, &taker_ata_b), 0);
        assert_closed(&fixture.svm, &fixture.vault);
        assert_closed(&fixture.svm, &fixture.escrow);
        assert_eq!(
            fixture.svm.get_balance(&fixture.maker.pubkey()).unwrap(),
            maker_balance_before + expected_refund
        );
    }

    #[test]
    fn test_cancel_instruction() {
        let mut fixture = make_escrow();
        let maker_balance_before = fixture.svm.get_balance(&fixture.maker.pubkey()).unwrap();
        let expected_refund = fixture.svm.get_account(&fixture.vault).unwrap().lamports
            + fixture.svm.get_account(&fixture.escrow).unwrap().lamports;
        let cancel_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(fixture.maker.pubkey(), true),
                AccountMeta::new_readonly(fixture.mint_a, false),
                AccountMeta::new(fixture.escrow, false),
                AccountMeta::new(fixture.vault, false),
                AccountMeta::new(fixture.maker_ata_a, false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
            ],
            data: vec![2u8],
        };
        let message = Message::new(&[cancel_ix], Some(&fixture.maker.pubkey()));
        let transaction =
            Transaction::new(&[&fixture.maker], message, fixture.svm.latest_blockhash());
        let tx = fixture.svm.send_transaction(transaction).unwrap();
        println!("Cancel CUs Consumed: {}", tx.compute_units_consumed);

        assert_eq!(
            token_balance(&fixture.svm, &fixture.maker_ata_a),
            MAKER_STARTING_A
        );
        assert_closed(&fixture.svm, &fixture.vault);
        assert_closed(&fixture.svm, &fixture.escrow);
        assert_eq!(
            fixture.svm.get_balance(&fixture.maker.pubkey()).unwrap(),
            maker_balance_before + expected_refund - tx.fee
        );
    }

    #[test]
    fn test_take_fails_when_taker_cannot_afford_asking_price() {
        let mut fixture = make_escrow();
        let underfunded_amount = AMOUNT_TO_RECEIVE / 2;
        let (taker, taker_ata_b) = create_taker_with_b(
            &mut fixture.svm,
            &fixture.maker,
            &fixture.mint_b,
            underfunded_amount,
        );
        let (take_ix, taker_ata_a, maker_ata_b) = take_instruction(&fixture, &taker, taker_ata_b);
        let message = Message::new(&[take_ix], Some(&taker.pubkey()));
        let transaction = Transaction::new(&[&taker], message, fixture.svm.latest_blockhash());
        let failure = fixture
            .svm
            .send_transaction(transaction)
            .expect_err("an underfunded taker must not settle the escrow");
        println!(
            "Underfunded Take CUs Consumed: {}",
            failure.meta.compute_units_consumed
        );

        assert_eq!(token_balance(&fixture.svm, &fixture.vault), AMOUNT_TO_GIVE);
        assert_eq!(
            token_balance(&fixture.svm, &fixture.maker_ata_a),
            MAKER_STARTING_A - AMOUNT_TO_GIVE
        );
        assert_eq!(
            token_balance(&fixture.svm, &taker_ata_b),
            underfunded_amount
        );
        assert!(fixture.svm.get_account(&fixture.escrow).is_some());
        assert!(fixture.svm.get_account(&taker_ata_a).is_none());
        assert!(fixture.svm.get_account(&maker_ata_b).is_none());
    }

    #[test]
    fn test_cancel_fails_for_stranger() {
        let mut fixture = make_escrow();
        let stranger = Keypair::new();
        fixture
            .svm
            .airdrop(&stranger.pubkey(), LAMPORTS_PER_SOL)
            .expect("stranger airdrop failed");
        let cancel_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(stranger.pubkey(), true),
                AccountMeta::new_readonly(fixture.mint_a, false),
                AccountMeta::new(fixture.escrow, false),
                AccountMeta::new(fixture.vault, false),
                AccountMeta::new(fixture.maker_ata_a, false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
            ],
            data: vec![2u8],
        };
        let message = Message::new(&[cancel_ix], Some(&stranger.pubkey()));
        let transaction = Transaction::new(&[&stranger], message, fixture.svm.latest_blockhash());
        let failure = fixture
            .svm
            .send_transaction(transaction)
            .expect_err("a stranger must not be able to cancel another maker's escrow");
        println!(
            "Stranger Cancel CUs Consumed: {}",
            failure.meta.compute_units_consumed
        );

        assert_eq!(token_balance(&fixture.svm, &fixture.vault), AMOUNT_TO_GIVE);
        assert_eq!(
            token_balance(&fixture.svm, &fixture.maker_ata_a),
            MAKER_STARTING_A - AMOUNT_TO_GIVE
        );
        assert!(fixture.svm.get_account(&fixture.escrow).is_some());
        assert!(fixture.svm.get_account(&fixture.vault).is_some());
    }
}
