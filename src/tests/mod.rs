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

    const INITIAL_MAKER_A: u64 = 1_000_000_000;
    const AMOUNT_TO_RECEIVE: u64 = 100_000_000;
    const AMOUNT_TO_GIVE: u64 = 500_000_000;

    struct MakeFixture {
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

        // LiteSVM 0.9 still ships the pre-SIMD-0194 Rent sysvar. Match the
        // folded rate used by live clusters and read by Pinocchio 0.11.
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

    fn make_fixture() -> MakeFixture {
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
        MintTo::new(&mut svm, &maker, &mint_a, &maker_ata_a, INITIAL_MAKER_A)
            .send()
            .unwrap();

        let (escrow, bump) =
            Pubkey::find_program_address(&[b"escrow", maker.pubkey().as_ref()], &program_id);
        let vault = spl_associated_token_account::get_associated_token_address(&escrow, &mint_a);

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
                AccountMeta::new(solana_sdk_ids::system_program::ID, false),
                AccountMeta::new(TOKEN_PROGRAM_ID, false),
                AccountMeta::new(ASSOCIATED_TOKEN_PROGRAM_ID.parse().unwrap(), false),
            ],
            data: make_data,
        };

        let message = Message::new(&[make_ix], Some(&maker.pubkey()));
        let transaction = Transaction::new(&[&maker], message, svm.latest_blockhash());
        let metadata = svm.send_transaction(transaction).unwrap();
        println!(
            "Make transaction successful; CUs consumed: {}",
            metadata.compute_units_consumed
        );

        MakeFixture {
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
                AccountMeta::new_readonly(mint_a, false),
                AccountMeta::new(escrow, false),
                AccountMeta::new(vault, false),
                AccountMeta::new(maker_ata_a, false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
            ],
            data: vec![2u8],
        }
    }

    fn token_balance(svm: &LiteSVM, address: &Pubkey) -> u64 {
        let account = svm.get_account(address).unwrap();
        spl_token_2022::state::Account::unpack(&account.data)
            .unwrap()
            .amount
    }

    fn assert_closed(svm: &LiteSVM, address: &Pubkey) {
        if let Some(account) = svm.get_account(address) {
            assert_eq!(account.lamports, 0, "closed account still has lamports");
            assert_eq!(
                account.owner,
                solana_sdk_ids::system_program::ID,
                "closed account still has its old owner"
            );
        }
    }

    #[test]
    fn test_make_instruction() {
        let fixture = make_fixture();

        assert_eq!(program_id().to_string(), PROGRAM_ID);
        assert_eq!(token_balance(&fixture.svm, &fixture.vault), AMOUNT_TO_GIVE);
        assert_eq!(
            token_balance(&fixture.svm, &fixture.maker_ata_a),
            INITIAL_MAKER_A - AMOUNT_TO_GIVE
        );

        let escrow_account = fixture.svm.get_account(&fixture.escrow).unwrap();
        assert_eq!(escrow_account.owner, program_id());
        assert_eq!(escrow_account.data.len(), 113);

        let data = &escrow_account.data;
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
        let MakeFixture {
            mut svm,
            maker,
            mint_a,
            mint_b,
            escrow,
            vault,
            ..
        } = make_fixture();

        let maker_balance_before = svm.get_balance(&maker.pubkey()).unwrap();
        let taker = Keypair::new();
        svm.airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

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
        assert!(svm.get_account(&taker_ata_a).is_none());
        assert!(svm.get_account(&maker_ata_b).is_none());

        let take_ix = take_instruction(
            taker.pubkey(),
            maker.pubkey(),
            mint_a,
            mint_b,
            escrow,
            vault,
            taker_ata_a,
            taker_ata_b,
            maker_ata_b,
        );
        let message = Message::new(&[take_ix], Some(&taker.pubkey()));
        let transaction = Transaction::new(&[&taker], message, svm.latest_blockhash());
        let metadata = svm.send_transaction(transaction).unwrap();
        println!(
            "Take transaction successful; CUs consumed: {}",
            metadata.compute_units_consumed
        );

        assert_eq!(token_balance(&svm, &taker_ata_a), AMOUNT_TO_GIVE);
        assert_eq!(token_balance(&svm, &maker_ata_b), AMOUNT_TO_RECEIVE);
        assert_closed(&svm, &vault);
        assert_closed(&svm, &escrow);
        assert!(svm.get_balance(&maker.pubkey()).unwrap() > maker_balance_before);
    }

    #[test]
    fn test_cancel_instruction() {
        let MakeFixture {
            mut svm,
            maker,
            mint_a,
            escrow,
            vault,
            maker_ata_a,
            ..
        } = make_fixture();

        let cancel_ix = cancel_instruction(maker.pubkey(), mint_a, escrow, vault, maker_ata_a);
        let message = Message::new(&[cancel_ix], Some(&maker.pubkey()));
        let transaction = Transaction::new(&[&maker], message, svm.latest_blockhash());
        let metadata = svm.send_transaction(transaction).unwrap();
        println!(
            "Cancel transaction successful; CUs consumed: {}",
            metadata.compute_units_consumed
        );

        assert_eq!(token_balance(&svm, &maker_ata_a), INITIAL_MAKER_A);
        assert_closed(&svm, &vault);
        assert_closed(&svm, &escrow);
    }

    #[test]
    fn test_take_fails_with_insufficient_token_b() {
        let MakeFixture {
            mut svm,
            maker,
            mint_a,
            mint_b,
            escrow,
            vault,
            ..
        } = make_fixture();

        let taker = Keypair::new();
        svm.airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();
        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut svm, &taker, &mint_b)
            .owner(&taker.pubkey())
            .send()
            .unwrap();
        let insufficient_b = AMOUNT_TO_RECEIVE / 2;
        MintTo::new(&mut svm, &maker, &mint_b, &taker_ata_b, insufficient_b)
            .send()
            .unwrap();

        let taker_ata_a =
            spl_associated_token_account::get_associated_token_address(&taker.pubkey(), &mint_a);
        let maker_ata_b =
            spl_associated_token_account::get_associated_token_address(&maker.pubkey(), &mint_b);
        let take_ix = take_instruction(
            taker.pubkey(),
            maker.pubkey(),
            mint_a,
            mint_b,
            escrow,
            vault,
            taker_ata_a,
            taker_ata_b,
            maker_ata_b,
        );
        let message = Message::new(&[take_ix], Some(&taker.pubkey()));
        let transaction = Transaction::new(&[&taker], message, svm.latest_blockhash());
        let failure = svm
            .send_transaction(transaction)
            .expect_err("Take must fail when the taker lacks B");
        println!(
            "Rejected Take; CUs consumed: {}",
            failure.meta.compute_units_consumed
        );
        assert_eq!(token_balance(&svm, &vault), AMOUNT_TO_GIVE);
        assert_eq!(token_balance(&svm, &taker_ata_b), insufficient_b);
        assert!(svm.get_account(&escrow).is_some());
        assert!(svm.get_account(&taker_ata_a).is_none());
        assert!(svm.get_account(&maker_ata_b).is_none());
    }

    #[test]
    fn test_cancel_fails_for_stranger() {
        let MakeFixture {
            mut svm,
            maker,
            mint_a,
            escrow,
            vault,
            maker_ata_a,
            ..
        } = make_fixture();

        let stranger = Keypair::new();
        svm.airdrop(&stranger.pubkey(), LAMPORTS_PER_SOL).unwrap();
        let cancel_ix = cancel_instruction(stranger.pubkey(), mint_a, escrow, vault, maker_ata_a);
        let message = Message::new(&[cancel_ix], Some(&stranger.pubkey()));
        let transaction = Transaction::new(&[&stranger], message, svm.latest_blockhash());
        let failure = svm
            .send_transaction(transaction)
            .expect_err("a stranger must not be able to cancel the escrow");
        println!(
            "Rejected stranger Cancel; CUs consumed: {}",
            failure.meta.compute_units_consumed
        );
        assert_eq!(token_balance(&svm, &vault), AMOUNT_TO_GIVE);
        assert_eq!(
            token_balance(&svm, &maker_ata_a),
            INITIAL_MAKER_A - AMOUNT_TO_GIVE
        );
        assert!(svm.get_account(&escrow).is_some());
        assert_eq!(svm.get_account(&escrow).unwrap().owner, program_id());
        assert_eq!(
            maker.pubkey().as_ref(),
            &svm.get_account(&escrow).unwrap().data[0..32]
        );
    }
}
