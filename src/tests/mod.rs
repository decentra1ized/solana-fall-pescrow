#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use litesvm::{types::TransactionResult, LiteSVM};
    use litesvm_token::{spl_token, CreateAssociatedTokenAccount, CreateMint, MintTo};
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
    const MAKER_START_AMOUNT: u64 = 1_000_000_000;
    const AMOUNT_TO_RECEIVE: u64 = 100_000_000;
    const AMOUNT_TO_GIVE: u64 = 500_000_000;

    struct EscrowFixture {
        svm: LiteSVM,
        maker: Keypair,
        mint_a: Pubkey,
        mint_b: Pubkey,
        escrow: Pubkey,
        bump: u8,
        vault: Pubkey,
        maker_ata_a: Pubkey,
    }

    fn program_id() -> Pubkey {
        Pubkey::from(crate::ID)
    }

    fn setup() -> (LiteSVM, Keypair) {
        let mut svm = LiteSVM::new();
        let payer = Keypair::new();

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

    fn associated_token_program() -> Pubkey {
        ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap()
    }

    fn escrow_pda(maker: &Pubkey) -> (Pubkey, u8) {
        Pubkey::find_program_address(
            &[b"escrow".as_ref(), maker.as_ref()],
            &PROGRAM_ID.parse().unwrap(),
        )
    }

    fn vault_address(escrow: &Pubkey, mint: &Pubkey) -> Pubkey {
        spl_associated_token_account::get_associated_token_address(escrow, mint)
    }

    fn make_instruction(
        maker: Pubkey,
        mint_a: Pubkey,
        mint_b: Pubkey,
        escrow: Pubkey,
        maker_ata_a: Pubkey,
        vault: Pubkey,
    ) -> Instruction {
        let data = [
            vec![0u8],
            AMOUNT_TO_RECEIVE.to_le_bytes().to_vec(),
            AMOUNT_TO_GIVE.to_le_bytes().to_vec(),
        ]
        .concat();

        Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(maker, true),
                AccountMeta::new(mint_a, false),
                AccountMeta::new(mint_b, false),
                AccountMeta::new(escrow, false),
                AccountMeta::new(maker_ata_a, false),
                AccountMeta::new(vault, false),
                AccountMeta::new(solana_sdk_ids::system_program::ID, false),
                AccountMeta::new(TOKEN_PROGRAM_ID, false),
                AccountMeta::new(associated_token_program(), false),
            ],
            data,
        }
    }

    fn take_instruction(
        taker: Pubkey,
        maker: Pubkey,
        mint_a: Pubkey,
        mint_b: Pubkey,
        escrow: Pubkey,
        vault: Pubkey,
        taker_ata_a: Pubkey,
        taker_ata_b: Pubkey,
        maker_ata_b: Pubkey,
    ) -> Instruction {
        Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(taker, true),
                AccountMeta::new(maker, false),
                AccountMeta::new(mint_a, false),
                AccountMeta::new(mint_b, false),
                AccountMeta::new(escrow, false),
                AccountMeta::new(vault, false),
                AccountMeta::new(taker_ata_a, false),
                AccountMeta::new(taker_ata_b, false),
                AccountMeta::new(maker_ata_b, false),
                AccountMeta::new(solana_sdk_ids::system_program::ID, false),
                AccountMeta::new(TOKEN_PROGRAM_ID, false),
                AccountMeta::new(associated_token_program(), false),
            ],
            data: vec![1u8],
        }
    }

    fn cancel_instruction(
        maker: Pubkey,
        mint_a: Pubkey,
        escrow: Pubkey,
        vault: Pubkey,
        maker_ata_a: Pubkey,
    ) -> Instruction {
        Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(maker, true),
                AccountMeta::new(mint_a, false),
                AccountMeta::new(escrow, false),
                AccountMeta::new(vault, false),
                AccountMeta::new(maker_ata_a, false),
                AccountMeta::new(TOKEN_PROGRAM_ID, false),
            ],
            data: vec![2u8],
        }
    }

    fn send_instruction(
        svm: &mut LiteSVM,
        instruction: Instruction,
        payer: &Keypair,
    ) -> TransactionResult {
        let message = Message::new(&[instruction], Some(&payer.pubkey()));
        let transaction = Transaction::new(&[payer], message, svm.latest_blockhash());
        svm.send_transaction(transaction)
    }

    fn token_amount(svm: &LiteSVM, token_account: &Pubkey) -> u64 {
        let account = svm.get_account(token_account).unwrap();
        spl_token_2022::state::Account::unpack(&account.data)
            .unwrap()
            .amount
    }

    fn account_closed(svm: &LiteSVM, address: &Pubkey) -> bool {
        svm.get_account(address)
            .map(|account| account.lamports == 0)
            .unwrap_or(true)
    }

    fn create_funded_taker(svm: &mut LiteSVM) -> Keypair {
        let taker = Keypair::new();
        svm.airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL)
            .expect("Airdrop failed");
        taker
    }

    fn setup_make() -> EscrowFixture {
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

        MintTo::new(&mut svm, &maker, &mint_a, &maker_ata_a, MAKER_START_AMOUNT)
            .send()
            .unwrap();

        let (escrow, bump) = escrow_pda(&maker.pubkey());
        let vault = vault_address(&escrow, &mint_a);
        let make_ix = make_instruction(maker.pubkey(), mint_a, mint_b, escrow, maker_ata_a, vault);
        let make_tx = send_instruction(&mut svm, make_ix, &maker).unwrap();

        println!("Make CUs Consumed: {}", make_tx.compute_units_consumed);

        assert_eq!(token_amount(&svm, &vault), AMOUNT_TO_GIVE);
        assert_eq!(
            token_amount(&svm, &maker_ata_a),
            MAKER_START_AMOUNT - AMOUNT_TO_GIVE
        );

        let escrow_account = svm.get_account(&escrow).unwrap();
        assert_eq!(escrow_account.owner, program_id());
        assert_eq!(escrow_account.data.len(), crate::state::Escrow::LEN);
        assert_eq!(&escrow_account.data[0..32], maker.pubkey().as_ref());
        assert_eq!(&escrow_account.data[32..64], mint_a.as_ref());
        assert_eq!(&escrow_account.data[64..96], mint_b.as_ref());
        assert_eq!(
            u64::from_le_bytes(escrow_account.data[96..104].try_into().unwrap()),
            AMOUNT_TO_RECEIVE
        );
        assert_eq!(
            u64::from_le_bytes(escrow_account.data[104..112].try_into().unwrap()),
            AMOUNT_TO_GIVE
        );
        assert_eq!(escrow_account.data[112], bump);

        EscrowFixture {
            svm,
            maker,
            mint_a,
            mint_b,
            escrow,
            bump,
            vault,
            maker_ata_a,
        }
    }

    #[test]
    pub fn test_make_instruction() {
        let fixture = setup_make();

        println!("Escrow PDA: {}", fixture.escrow);
        println!("Vault ATA: {}", fixture.vault);
        println!("Bump: {}", fixture.bump);
    }

    #[test]
    pub fn test_take_instruction() {
        let mut fixture = setup_make();
        let taker = create_funded_taker(&mut fixture.svm);
        let taker_ata_b =
            CreateAssociatedTokenAccount::new(&mut fixture.svm, &taker, &fixture.mint_b)
                .owner(&taker.pubkey())
                .send()
                .unwrap();

        MintTo::new(
            &mut fixture.svm,
            &fixture.maker,
            &fixture.mint_b,
            &taker_ata_b,
            AMOUNT_TO_RECEIVE,
        )
        .send()
        .unwrap();

        let taker_ata_a = spl_associated_token_account::get_associated_token_address(
            &taker.pubkey(),
            &fixture.mint_a,
        );
        let maker_ata_b = spl_associated_token_account::get_associated_token_address(
            &fixture.maker.pubkey(),
            &fixture.mint_b,
        );
        let maker_lamports_before = fixture
            .svm
            .get_balance(&fixture.maker.pubkey())
            .unwrap_or_default();

        let take_ix = take_instruction(
            taker.pubkey(),
            fixture.maker.pubkey(),
            fixture.mint_a,
            fixture.mint_b,
            fixture.escrow,
            fixture.vault,
            taker_ata_a,
            taker_ata_b,
            maker_ata_b,
        );
        let take_tx = send_instruction(&mut fixture.svm, take_ix, &taker).unwrap();

        println!("Take CUs Consumed: {}", take_tx.compute_units_consumed);

        assert_eq!(token_amount(&fixture.svm, &taker_ata_a), AMOUNT_TO_GIVE);
        assert_eq!(token_amount(&fixture.svm, &maker_ata_b), AMOUNT_TO_RECEIVE);
        assert!(account_closed(&fixture.svm, &fixture.vault));
        assert!(account_closed(&fixture.svm, &fixture.escrow));

        let maker_lamports_after = fixture
            .svm
            .get_balance(&fixture.maker.pubkey())
            .unwrap_or_default();
        assert!(maker_lamports_after > maker_lamports_before);
    }

    #[test]
    pub fn test_cancel_instruction() {
        let mut fixture = setup_make();
        let maker_lamports_before = fixture
            .svm
            .get_balance(&fixture.maker.pubkey())
            .unwrap_or_default();

        let cancel_ix = cancel_instruction(
            fixture.maker.pubkey(),
            fixture.mint_a,
            fixture.escrow,
            fixture.vault,
            fixture.maker_ata_a,
        );
        let cancel_tx = send_instruction(&mut fixture.svm, cancel_ix, &fixture.maker).unwrap();

        println!("Cancel CUs Consumed: {}", cancel_tx.compute_units_consumed);

        assert_eq!(
            token_amount(&fixture.svm, &fixture.maker_ata_a),
            MAKER_START_AMOUNT
        );
        assert!(account_closed(&fixture.svm, &fixture.vault));
        assert!(account_closed(&fixture.svm, &fixture.escrow));

        let maker_lamports_after = fixture
            .svm
            .get_balance(&fixture.maker.pubkey())
            .unwrap_or_default();
        assert!(maker_lamports_after > maker_lamports_before);
    }

    #[test]
    pub fn test_take_fails_when_taker_is_underfunded() {
        let mut fixture = setup_make();
        let taker = create_funded_taker(&mut fixture.svm);
        let taker_ata_b =
            CreateAssociatedTokenAccount::new(&mut fixture.svm, &taker, &fixture.mint_b)
                .owner(&taker.pubkey())
                .send()
                .unwrap();

        MintTo::new(
            &mut fixture.svm,
            &fixture.maker,
            &fixture.mint_b,
            &taker_ata_b,
            AMOUNT_TO_RECEIVE / 2,
        )
        .send()
        .unwrap();

        let taker_ata_a = spl_associated_token_account::get_associated_token_address(
            &taker.pubkey(),
            &fixture.mint_a,
        );
        let maker_ata_b = spl_associated_token_account::get_associated_token_address(
            &fixture.maker.pubkey(),
            &fixture.mint_b,
        );
        let take_ix = take_instruction(
            taker.pubkey(),
            fixture.maker.pubkey(),
            fixture.mint_a,
            fixture.mint_b,
            fixture.escrow,
            fixture.vault,
            taker_ata_a,
            taker_ata_b,
            maker_ata_b,
        );
        let err = send_instruction(&mut fixture.svm, take_ix, &taker).unwrap_err();

        println!(
            "Underfunded Take failed after {} CUs: {:?}",
            err.meta.compute_units_consumed, err.err
        );

        assert_eq!(token_amount(&fixture.svm, &fixture.vault), AMOUNT_TO_GIVE);
        assert_eq!(
            token_amount(&fixture.svm, &taker_ata_b),
            AMOUNT_TO_RECEIVE / 2
        );
        assert_eq!(
            token_amount(&fixture.svm, &fixture.maker_ata_a),
            MAKER_START_AMOUNT - AMOUNT_TO_GIVE
        );
        assert!(fixture.svm.get_account(&fixture.escrow).is_some());
        assert!(fixture.svm.get_account(&taker_ata_a).is_none());
        assert!(fixture.svm.get_account(&maker_ata_b).is_none());
    }

    #[test]
    pub fn test_cancel_fails_for_wrong_maker() {
        let mut fixture = setup_make();
        let stranger = create_funded_taker(&mut fixture.svm);
        let cancel_ix = cancel_instruction(
            stranger.pubkey(),
            fixture.mint_a,
            fixture.escrow,
            fixture.vault,
            fixture.maker_ata_a,
        );
        let err = send_instruction(&mut fixture.svm, cancel_ix, &stranger).unwrap_err();

        println!(
            "Wrong-maker Cancel failed after {} CUs: {:?}",
            err.meta.compute_units_consumed, err.err
        );

        assert_eq!(token_amount(&fixture.svm, &fixture.vault), AMOUNT_TO_GIVE);
        assert_eq!(
            token_amount(&fixture.svm, &fixture.maker_ata_a),
            MAKER_START_AMOUNT - AMOUNT_TO_GIVE
        );
        assert!(fixture.svm.get_account(&fixture.escrow).is_some());
    }

    #[test]
    pub fn test_take_fails_for_wrong_maker_account() {
        let mut fixture = setup_make();
        let taker = create_funded_taker(&mut fixture.svm);
        let fake_maker = Keypair::new();
        fixture
            .svm
            .airdrop(&fake_maker.pubkey(), LAMPORTS_PER_SOL)
            .expect("Airdrop failed");

        let taker_ata_b =
            CreateAssociatedTokenAccount::new(&mut fixture.svm, &taker, &fixture.mint_b)
                .owner(&taker.pubkey())
                .send()
                .unwrap();
        MintTo::new(
            &mut fixture.svm,
            &fixture.maker,
            &fixture.mint_b,
            &taker_ata_b,
            AMOUNT_TO_RECEIVE,
        )
        .send()
        .unwrap();

        let taker_ata_a = spl_associated_token_account::get_associated_token_address(
            &taker.pubkey(),
            &fixture.mint_a,
        );
        let fake_maker_ata_b = spl_associated_token_account::get_associated_token_address(
            &fake_maker.pubkey(),
            &fixture.mint_b,
        );
        let take_ix = take_instruction(
            taker.pubkey(),
            fake_maker.pubkey(),
            fixture.mint_a,
            fixture.mint_b,
            fixture.escrow,
            fixture.vault,
            taker_ata_a,
            taker_ata_b,
            fake_maker_ata_b,
        );
        let err = send_instruction(&mut fixture.svm, take_ix, &taker).unwrap_err();

        println!(
            "Wrong-maker Take failed after {} CUs: {:?}",
            err.meta.compute_units_consumed, err.err
        );

        assert_eq!(token_amount(&fixture.svm, &fixture.vault), AMOUNT_TO_GIVE);
        assert_eq!(token_amount(&fixture.svm, &taker_ata_b), AMOUNT_TO_RECEIVE);
        assert!(fixture.svm.get_account(&fixture.escrow).is_some());
        assert!(fixture.svm.get_account(&taker_ata_a).is_none());
        assert!(fixture.svm.get_account(&fake_maker_ata_b).is_none());
    }
}
