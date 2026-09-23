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
    use solana_transaction::{InstructionError, Transaction, TransactionError};
    use solana_program_pack::Pack;

    const PROGRAM_ID: &str = "4ibrEMW5F6hKnkW4jVedswYv6H6VtwPN6ar6dvXDN1nT";
    const TOKEN_PROGRAM_ID: Pubkey = spl_token::ID;
    const ASSOCIATED_TOKEN_PROGRAM_ID: &str = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL";
    const INITIAL_A: u64 = 1_000_000_000;
    const AMOUNT_TO_GIVE: u64 = 500_000_000;
    const AMOUNT_TO_RECEIVE: u64 = 100_000_000;
    
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
        let fixture = make_escrow();
        assert_eq!(program_id().to_string(), PROGRAM_ID);
        assert_eq!(token_amount(&fixture.svm, &fixture.vault), AMOUNT_TO_GIVE);
        assert_eq!(
            token_amount(&fixture.svm, &fixture.maker_ata_a),
            INITIAL_A - AMOUNT_TO_GIVE
        );
        let escrow_account = fixture.svm.get_account(&fixture.escrow).unwrap();
        assert_eq!(escrow_account.owner, program_id());
        let data = &escrow_account.data;
        assert_eq!(data.len(), 113);
        assert_eq!(&data[0..32], fixture.maker.pubkey().as_ref());
        assert_eq!(&data[32..64], fixture.mint_a.as_ref());
        assert_eq!(&data[64..96], fixture.mint_b.as_ref());
        assert_eq!(u64::from_le_bytes(data[96..104].try_into().unwrap()), AMOUNT_TO_RECEIVE);
        assert_eq!(u64::from_le_bytes(data[104..112].try_into().unwrap()), AMOUNT_TO_GIVE);
        let (_, bump) = Pubkey::find_program_address(
            &[b"escrow", fixture.maker.pubkey().as_ref()],
            &program_id(),
        );
        assert_eq!(data[112], bump);
    }

    struct MadeEscrow {
        svm: LiteSVM,
        maker: Keypair,
        mint_a: Pubkey,
        mint_b: Pubkey,
        maker_ata_a: Pubkey,
        escrow: Pubkey,
        vault: Pubkey,
    }

    fn make_escrow() -> MadeEscrow {
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
        MintTo::new(&mut svm, &maker, &mint_a, &maker_ata_a, INITIAL_A)
            .send()
            .unwrap();

        let (escrow, _) = Pubkey::find_program_address(
            &[b"escrow", maker.pubkey().as_ref()],
            &program_id(),
        );
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
                AccountMeta::new_readonly(ASSOCIATED_TOKEN_PROGRAM_ID.parse().unwrap(), false),
            ],
            data: [
                vec![0u8],
                AMOUNT_TO_RECEIVE.to_le_bytes().to_vec(),
                AMOUNT_TO_GIVE.to_le_bytes().to_vec(),
            ]
            .concat(),
        };
        let tx = Transaction::new(
            &[&maker],
            Message::new(&[make_ix], Some(&maker.pubkey())),
            svm.latest_blockhash(),
        );
        let meta = svm.send_transaction(tx).expect("Make failed");
        println!("Make CUs: {}", meta.compute_units_consumed);

        MadeEscrow {
            svm,
            maker,
            mint_a,
            mint_b,
            maker_ata_a,
            escrow,
            vault,
        }
    }

    fn token_amount(svm: &LiteSVM, address: &Pubkey) -> u64 {
        let account = svm.get_account(address).unwrap();
        spl_token_2022::state::Account::unpack(&account.data)
            .unwrap()
            .amount
    }

    fn assert_closed(svm: &LiteSVM, address: &Pubkey) {
        assert!(
            svm.get_account(address).map_or(true, |account| {
                account.lamports == 0 && account.owner == solana_sdk_ids::system_program::ID
            }),
            "account {address} was not closed"
        );
    }

    fn funded_taker(fixture: &mut MadeEscrow, b_amount: u64) -> (Keypair, Pubkey) {
        let taker = Keypair::new();
        fixture
            .svm
            .airdrop(&taker.pubkey(), 2 * LAMPORTS_PER_SOL)
            .unwrap();
        let taker_ata_b = CreateAssociatedTokenAccount::new(
            &mut fixture.svm,
            &taker,
            &fixture.mint_b,
        )
        .send()
        .unwrap();
        MintTo::new(
            &mut fixture.svm,
            &fixture.maker,
            &fixture.mint_b,
            &taker_ata_b,
            b_amount,
        )
        .send()
        .unwrap();
        (taker, taker_ata_b)
    }

    fn take_instruction(fixture: &MadeEscrow, taker: &Keypair, taker_ata_b: Pubkey) -> Instruction {
        let taker_ata_a = spl_associated_token_account::get_associated_token_address(
            &taker.pubkey(),
            &fixture.mint_a,
        );
        let maker_ata_b = spl_associated_token_account::get_associated_token_address(
            &fixture.maker.pubkey(),
            &fixture.mint_b,
        );
        Instruction {
            program_id: program_id(),
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
                AccountMeta::new_readonly(ASSOCIATED_TOKEN_PROGRAM_ID.parse().unwrap(), false),
            ],
            data: vec![1u8],
        }
    }

    fn cancel_instruction(fixture: &MadeEscrow, caller: Pubkey, signer: bool) -> Instruction {
        Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(caller, signer),
                AccountMeta::new_readonly(fixture.mint_a, false),
                AccountMeta::new(fixture.escrow, false),
                AccountMeta::new(fixture.vault, false),
                AccountMeta::new(fixture.maker_ata_a, false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
            ],
            data: vec![2u8],
        }
    }

    #[test]
    fn test_take_instruction() {
        let mut fixture = make_escrow();
        let (taker, taker_ata_b) = funded_taker(&mut fixture, AMOUNT_TO_RECEIVE);
        let taker_ata_a = spl_associated_token_account::get_associated_token_address(
            &taker.pubkey(),
            &fixture.mint_a,
        );
        let maker_ata_b = spl_associated_token_account::get_associated_token_address(
            &fixture.maker.pubkey(),
            &fixture.mint_b,
        );
        let maker_balance_before = fixture.svm.get_balance(&fixture.maker.pubkey()).unwrap();
        let escrow_rent = fixture.svm.get_account(&fixture.escrow).unwrap().lamports;
        let vault_rent = fixture.svm.get_account(&fixture.vault).unwrap().lamports;
        let ix = take_instruction(&fixture, &taker, taker_ata_b);
        let tx = Transaction::new(
            &[&taker],
            Message::new(&[ix], Some(&taker.pubkey())),
            fixture.svm.latest_blockhash(),
        );
        let meta = fixture.svm.send_transaction(tx).expect("Take failed");
        println!("Take CUs: {}", meta.compute_units_consumed);

        assert_eq!(token_amount(&fixture.svm, &taker_ata_a), AMOUNT_TO_GIVE);
        assert_eq!(token_amount(&fixture.svm, &maker_ata_b), AMOUNT_TO_RECEIVE);
        assert_closed(&fixture.svm, &fixture.vault);
        assert_closed(&fixture.svm, &fixture.escrow);
        assert_eq!(
            fixture.svm.get_balance(&fixture.maker.pubkey()).unwrap(),
            maker_balance_before + escrow_rent + vault_rent
        );
    }

    #[test]
    fn test_cancel_instruction() {
        let mut fixture = make_escrow();
        let ix = cancel_instruction(&fixture, fixture.maker.pubkey(), true);
        let tx = Transaction::new(
            &[&fixture.maker],
            Message::new(&[ix], Some(&fixture.maker.pubkey())),
            fixture.svm.latest_blockhash(),
        );
        let meta = fixture.svm.send_transaction(tx).expect("Cancel failed");
        println!("Cancel CUs: {}", meta.compute_units_consumed);

        assert_eq!(token_amount(&fixture.svm, &fixture.maker_ata_a), INITIAL_A);
        assert_closed(&fixture.svm, &fixture.vault);
        assert_closed(&fixture.svm, &fixture.escrow);
    }

    #[test]
    fn test_take_insufficient_b() {
        let mut fixture = make_escrow();
        let (taker, taker_ata_b) = funded_taker(&mut fixture, AMOUNT_TO_RECEIVE / 2);
        let taker_ata_a = spl_associated_token_account::get_associated_token_address(
            &taker.pubkey(),
            &fixture.mint_a,
        );
        let maker_ata_b = spl_associated_token_account::get_associated_token_address(
            &fixture.maker.pubkey(),
            &fixture.mint_b,
        );
        let ix = take_instruction(&fixture, &taker, taker_ata_b);
        let tx = Transaction::new(
            &[&taker],
            Message::new(&[ix], Some(&taker.pubkey())),
            fixture.svm.latest_blockhash(),
        );
        let err = fixture.svm.send_transaction(tx).expect_err("underfunded Take succeeded");
        println!("Underfunded Take CUs: {}", err.meta.compute_units_consumed);
        assert_eq!(err.err, TransactionError::InstructionError(0, InstructionError::Custom(1)));
        assert_eq!(token_amount(&fixture.svm, &fixture.vault), AMOUNT_TO_GIVE);
        assert_eq!(token_amount(&fixture.svm, &taker_ata_b), AMOUNT_TO_RECEIVE / 2);
        assert_eq!(token_amount(&fixture.svm, &fixture.maker_ata_a), INITIAL_A - AMOUNT_TO_GIVE);
        assert!(fixture.svm.get_account(&taker_ata_a).is_none());
        assert!(fixture.svm.get_account(&maker_ata_b).is_none());
        assert!(fixture.svm.get_account(&fixture.escrow).is_some());
    }

    #[test]
    fn test_cancel_stranger_fails() {
        let mut fixture = make_escrow();
        let stranger = Keypair::new();
        fixture
            .svm
            .airdrop(&stranger.pubkey(), LAMPORTS_PER_SOL)
            .unwrap();
        let ix = cancel_instruction(&fixture, stranger.pubkey(), true);
        let tx = Transaction::new(
            &[&stranger],
            Message::new(&[ix], Some(&stranger.pubkey())),
            fixture.svm.latest_blockhash(),
        );
        let err = fixture.svm.send_transaction(tx).expect_err("stranger canceled escrow");
        println!("Stranger Cancel CUs: {}", err.meta.compute_units_consumed);
        assert_eq!(err.err, TransactionError::InstructionError(0, InstructionError::InvalidAccountData));

        let ix = cancel_instruction(&fixture, fixture.maker.pubkey(), false);
        let tx = Transaction::new(
            &[&stranger],
            Message::new(&[ix], Some(&stranger.pubkey())),
            fixture.svm.latest_blockhash(),
        );
        let err = fixture.svm.send_transaction(tx).expect_err("unsigned maker canceled escrow");
        println!("Unsigned Cancel CUs: {}", err.meta.compute_units_consumed);
        assert_eq!(err.err, TransactionError::InstructionError(0, InstructionError::MissingRequiredSignature));
        assert_eq!(token_amount(&fixture.svm, &fixture.vault), AMOUNT_TO_GIVE);
        assert!(fixture.svm.get_account(&fixture.escrow).is_some());
    }
}
