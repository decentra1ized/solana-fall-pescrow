#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use litesvm::LiteSVM;
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

    const MAKER_A_START: u64 = 1_000_000_000;
    const AMOUNT_TO_RECEIVE: u64 = 100_000_000;
    const AMOUNT_TO_GIVE: u64 = 500_000_000;

    struct MakeContext {
        svm: LiteSVM,
        payer: Keypair,
        maker: Keypair,
        mint_a: Pubkey,
        mint_b: Pubkey,
        maker_ata_a: Pubkey,
        escrow: Pubkey,
        bump: u8,
        vault: Pubkey,
    }

    fn program_id() -> Pubkey {
        Pubkey::from(crate::ID)
    }

    fn associated_token_program() -> Pubkey {
        ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap()
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

    fn token_amount(svm: &LiteSVM, token_account: &Pubkey) -> u64 {
        let account = svm.get_account(token_account).unwrap();
        spl_token_2022::state::Account::unpack(&account.data)
            .unwrap()
            .amount
    }

    fn account_is_gone_or_closed(svm: &LiteSVM, address: &Pubkey) -> bool {
        svm.get_account(address).map_or(true, |account| {
            account.lamports == 0 && account.owner == solana_sdk_ids::system_program::ID
        })
    }

    fn send_ix(
        svm: &mut LiteSVM,
        payer: &Pubkey,
        signers: &[&Keypair],
        ix: Instruction,
    ) -> litesvm::types::TransactionMetadata {
        let message = Message::new(&[ix], Some(payer));
        let transaction = Transaction::new(signers, message, svm.latest_blockhash());
        svm.send_transaction(transaction).unwrap()
    }

    fn send_ix_err(svm: &mut LiteSVM, payer: &Pubkey, signers: &[&Keypair], ix: Instruction) {
        let message = Message::new(&[ix], Some(payer));
        let transaction = Transaction::new(signers, message, svm.latest_blockhash());
        assert!(svm.send_transaction(transaction).is_err());
    }

    fn test_make_instruction() -> MakeContext {
        let (mut svm, payer) = setup();
        let maker = Keypair::new();

        svm.airdrop(&maker.pubkey(), LAMPORTS_PER_SOL)
            .expect("Maker airdrop failed");

        let mint_a = CreateMint::new(&mut svm, &payer)
            .decimals(6)
            .authority(&payer.pubkey())
            .send()
            .unwrap();
        let mint_b = CreateMint::new(&mut svm, &payer)
            .decimals(6)
            .authority(&payer.pubkey())
            .send()
            .unwrap();

        let maker_ata_a = CreateAssociatedTokenAccount::new(&mut svm, &payer, &mint_a)
            .owner(&maker.pubkey())
            .send()
            .unwrap();

        MintTo::new(&mut svm, &payer, &mint_a, &maker_ata_a, MAKER_A_START)
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

        let tx = send_ix(&mut svm, &maker.pubkey(), &[&maker], make_ix);
        println!("Make compute_units_consumed: {}", tx.compute_units_consumed);

        assert_eq!(token_amount(&svm, &vault), AMOUNT_TO_GIVE);
        assert_eq!(
            token_amount(&svm, &maker_ata_a),
            MAKER_A_START - AMOUNT_TO_GIVE
        );

        let escrow_account = svm.get_account(&escrow).unwrap();
        let data = &escrow_account.data;
        assert_eq!(&data[0..32], maker.pubkey().as_ref());
        assert_eq!(&data[32..64], mint_a.as_ref());
        assert_eq!(&data[64..96], mint_b.as_ref());
        assert_eq!(
            u64::from_le_bytes(data[96..104].try_into().unwrap()),
            AMOUNT_TO_RECEIVE
        );
        assert_eq!(
            u64::from_le_bytes(data[104..112].try_into().unwrap()),
            AMOUNT_TO_GIVE
        );
        assert_eq!(data[112], bump);

        MakeContext {
            svm,
            payer,
            maker,
            mint_a,
            mint_b,
            maker_ata_a,
            escrow,
            bump,
            vault,
        }
    }

    fn take_instruction(
        ctx: &MakeContext,
        taker: Pubkey,
        taker_ata_a: Pubkey,
        taker_ata_b: Pubkey,
        maker_ata_b: Pubkey,
    ) -> Instruction {
        Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(taker, true),
                AccountMeta::new(ctx.maker.pubkey(), false),
                AccountMeta::new(ctx.mint_a, false),
                AccountMeta::new(ctx.mint_b, false),
                AccountMeta::new(ctx.escrow, false),
                AccountMeta::new(ctx.vault, false),
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

    fn cancel_instruction(ctx: &MakeContext, signer: Pubkey) -> Instruction {
        Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(signer, true),
                AccountMeta::new(ctx.mint_a, false),
                AccountMeta::new(ctx.escrow, false),
                AccountMeta::new(ctx.vault, false),
                AccountMeta::new(ctx.maker_ata_a, false),
                AccountMeta::new(TOKEN_PROGRAM_ID, false),
            ],
            data: vec![2u8],
        }
    }

    #[test]
    pub fn test_make_instruction_state() {
        let ctx = test_make_instruction();
        assert_eq!(program_id().to_string(), PROGRAM_ID);
        assert!(ctx.bump > 0);
    }

    #[test]
    pub fn test_take_instruction() {
        let mut ctx = test_make_instruction();
        let taker = Keypair::new();

        ctx.svm
            .airdrop(&taker.pubkey(), LAMPORTS_PER_SOL)
            .expect("Taker airdrop failed");

        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut ctx.svm, &ctx.payer, &ctx.mint_b)
            .owner(&taker.pubkey())
            .send()
            .unwrap();
        MintTo::new(
            &mut ctx.svm,
            &ctx.payer,
            &ctx.mint_b,
            &taker_ata_b,
            AMOUNT_TO_RECEIVE,
        )
        .send()
        .unwrap();

        let taker_ata_a = spl_associated_token_account::get_associated_token_address(
            &taker.pubkey(),
            &ctx.mint_a,
        );
        let maker_ata_b = spl_associated_token_account::get_associated_token_address(
            &ctx.maker.pubkey(),
            &ctx.mint_b,
        );
        let maker_lamports_before = ctx.svm.get_account(&ctx.maker.pubkey()).unwrap().lamports;

        let take_ix = take_instruction(&ctx, taker.pubkey(), taker_ata_a, taker_ata_b, maker_ata_b);
        let tx = send_ix(&mut ctx.svm, &taker.pubkey(), &[&taker], take_ix);
        println!("Take compute_units_consumed: {}", tx.compute_units_consumed);

        assert_eq!(token_amount(&ctx.svm, &taker_ata_a), AMOUNT_TO_GIVE);
        assert_eq!(token_amount(&ctx.svm, &maker_ata_b), AMOUNT_TO_RECEIVE);
        assert!(account_is_gone_or_closed(&ctx.svm, &ctx.vault));
        assert!(account_is_gone_or_closed(&ctx.svm, &ctx.escrow));

        let maker_lamports_after = ctx.svm.get_account(&ctx.maker.pubkey()).unwrap().lamports;
        assert!(maker_lamports_after > maker_lamports_before);
    }

    #[test]
    pub fn test_cancel_instruction() {
        let mut ctx = test_make_instruction();

        let cancel_ix = cancel_instruction(&ctx, ctx.maker.pubkey());
        let tx = send_ix(&mut ctx.svm, &ctx.maker.pubkey(), &[&ctx.maker], cancel_ix);
        println!(
            "Cancel compute_units_consumed: {}",
            tx.compute_units_consumed
        );

        assert_eq!(token_amount(&ctx.svm, &ctx.maker_ata_a), MAKER_A_START);
        assert!(account_is_gone_or_closed(&ctx.svm, &ctx.vault));
        assert!(account_is_gone_or_closed(&ctx.svm, &ctx.escrow));
    }

    #[test]
    pub fn test_take_instruction_underfunded_taker_fails() {
        let mut ctx = test_make_instruction();
        let taker = Keypair::new();

        ctx.svm
            .airdrop(&taker.pubkey(), LAMPORTS_PER_SOL)
            .expect("Taker airdrop failed");

        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut ctx.svm, &ctx.payer, &ctx.mint_b)
            .owner(&taker.pubkey())
            .send()
            .unwrap();
        MintTo::new(
            &mut ctx.svm,
            &ctx.payer,
            &ctx.mint_b,
            &taker_ata_b,
            50_000_000,
        )
        .send()
        .unwrap();

        let taker_ata_a = spl_associated_token_account::get_associated_token_address(
            &taker.pubkey(),
            &ctx.mint_a,
        );
        let maker_ata_b = spl_associated_token_account::get_associated_token_address(
            &ctx.maker.pubkey(),
            &ctx.mint_b,
        );
        let maker_a_before = token_amount(&ctx.svm, &ctx.maker_ata_a);
        let vault_before = token_amount(&ctx.svm, &ctx.vault);

        let take_ix = take_instruction(&ctx, taker.pubkey(), taker_ata_a, taker_ata_b, maker_ata_b);
        send_ix_err(&mut ctx.svm, &taker.pubkey(), &[&taker], take_ix);

        assert_eq!(token_amount(&ctx.svm, &ctx.maker_ata_a), maker_a_before);
        assert_eq!(token_amount(&ctx.svm, &ctx.vault), vault_before);
        assert!(ctx.svm.get_account(&ctx.escrow).is_some());
    }

    #[test]
    pub fn test_cancel_instruction_stranger_fails() {
        let mut ctx = test_make_instruction();
        let stranger = Keypair::new();

        ctx.svm
            .airdrop(&stranger.pubkey(), LAMPORTS_PER_SOL)
            .expect("Stranger airdrop failed");

        let cancel_ix = cancel_instruction(&ctx, stranger.pubkey());
        send_ix_err(&mut ctx.svm, &stranger.pubkey(), &[&stranger], cancel_ix);

        assert_eq!(token_amount(&ctx.svm, &ctx.vault), AMOUNT_TO_GIVE);
        assert_eq!(
            token_amount(&ctx.svm, &ctx.maker_ata_a),
            MAKER_A_START - AMOUNT_TO_GIVE
        );
        assert!(ctx.svm.get_account(&ctx.escrow).is_some());
    }
}
