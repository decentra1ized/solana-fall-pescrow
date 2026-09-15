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

    struct MakeContext {
        svm: LiteSVM,
        maker: Keypair,
        mint_a: Pubkey,
        mint_b: Pubkey,
        maker_ata_a: Pubkey,
        escrow: (Pubkey, u8),
        vault: Pubkey,
        amount_to_receive: u64,
        amount_to_give: u64,
    }

    fn setup_make() -> MakeContext {
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

        let escrow = Pubkey::find_program_address(
            &[b"escrow".as_ref(), maker.pubkey().as_ref()],
            &PROGRAM_ID.parse().unwrap(),
        );

        let vault = spl_associated_token_account::get_associated_token_address(
            &escrow.0,
            &mint_a,
        );

        let associated_token_program = ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();
        let token_program = TOKEN_PROGRAM_ID;
        let system_program = solana_sdk_ids::system_program::ID;

        MintTo::new(&mut svm, &maker, &mint_a, &maker_ata_a, 1000000000)
            .send()
            .unwrap();

        let amount_to_receive: u64 = 100000000; // 100 tokens with 6 decimal places
        let amount_to_give: u64 = 500000000;    // 500 tokens with 6 decimal places

        let make_data = [
            vec![0u8], // Discriminator for Make
            amount_to_receive.to_le_bytes().to_vec(),
            amount_to_give.to_le_bytes().to_vec(),
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

        let tx = svm.send_transaction(transaction).unwrap();
        println!("Make transaction successful");
        println!("CUs Consumed: {}", tx.compute_units_consumed);

        MakeContext {
            svm,
            maker,
            mint_a,
            mint_b,
            maker_ata_a,
            escrow,
            vault,
            amount_to_receive,
            amount_to_give,
        }
    }



    #[test]
    pub fn test_make_instruction() {
        let ctx = setup_make();

        assert_eq!(program_id().to_string(), PROGRAM_ID);

        let vault_acc = ctx.svm.get_account(&ctx.vault).unwrap();
        let vault_state = spl_token_2022::state::Account::unpack(&vault_acc.data).unwrap();
        assert_eq!(vault_state.amount, ctx.amount_to_give);

        let maker_acc = ctx.svm.get_account(&ctx.maker_ata_a).unwrap();
        let maker_state = spl_token_2022::state::Account::unpack(&maker_acc.data).unwrap();
        assert_eq!(maker_state.amount, 1000000000 - ctx.amount_to_give);

        let esc = ctx.svm.get_account(&ctx.escrow.0).unwrap();
        assert_eq!(esc.owner, program_id());
        assert_eq!(esc.data.len(), 113);
        let d = &esc.data;
        assert_eq!(&d[0..32], ctx.maker.pubkey().as_ref());
        assert_eq!(u64::from_le_bytes(d[96..104].try_into().unwrap()), ctx.amount_to_receive);
        assert_eq!(u64::from_le_bytes(d[104..112].try_into().unwrap()), ctx.amount_to_give);
        assert_eq!(d[112], ctx.escrow.1);
    }

    #[test]
    pub fn test_take_instruction() {
        let mut ctx = setup_make();
        let program_id = program_id();

        let taker = Keypair::new();
        ctx.svm
            .airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL)
            .expect("Airdrop to taker failed");

        // Taker creates ATA for token B and mints 100 B into it
        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut ctx.svm, &taker, &ctx.mint_b)
            .owner(&taker.pubkey())
            .send()
            .unwrap();

        MintTo::new(&mut ctx.svm, &ctx.maker, &ctx.mint_b, &taker_ata_b, ctx.amount_to_receive)
            .send()
            .unwrap();

        // Derive taker_ata_a and maker_ata_b; do not create them (CreateIdempotent handles them).
        let taker_ata_a = spl_associated_token_account::get_associated_token_address(
            &taker.pubkey(),
            &ctx.mint_a,
        );
        let maker_ata_b = spl_associated_token_account::get_associated_token_address(
            &ctx.maker.pubkey(),
            &ctx.mint_b,
        );

        let associated_token_program = ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();
        let token_program = TOKEN_PROGRAM_ID;
        let system_program = solana_sdk_ids::system_program::ID;

        let maker_balance_before = ctx.svm.get_account(&ctx.maker.pubkey()).unwrap().lamports;

        // Take instruction: discriminator 1
        let take_ix = Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new(taker.pubkey(), true),
                AccountMeta::new(ctx.maker.pubkey(), false),
                AccountMeta::new_readonly(ctx.mint_a, false),
                AccountMeta::new_readonly(ctx.mint_b, false),
                AccountMeta::new(ctx.escrow.0, false),
                AccountMeta::new(ctx.vault, false),
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
        let recent_blockhash = ctx.svm.latest_blockhash();
        let transaction = Transaction::new(&[&taker], message, recent_blockhash);

        let tx = ctx.svm.send_transaction(transaction).unwrap();
        println!("\nTake transaction successful");
        println!("CUs Consumed: {}", tx.compute_units_consumed);

        // Verification assertions
        let taker_ata_a_acc = ctx.svm.get_account(&taker_ata_a).expect("taker_ata_a should exist");
        let taker_ata_a_state = spl_token_2022::state::Account::unpack(&taker_ata_a_acc.data).unwrap();
        assert_eq!(taker_ata_a_state.amount, ctx.amount_to_give);

        let maker_ata_b_acc = ctx.svm.get_account(&maker_ata_b).expect("maker_ata_b should exist");
        let maker_ata_b_state = spl_token_2022::state::Account::unpack(&maker_ata_b_acc.data).unwrap();
        assert_eq!(maker_ata_b_state.amount, ctx.amount_to_receive);

        assert!(ctx.svm.get_account(&ctx.vault).map_or(true, |acc| acc.lamports == 0), "vault must be closed");
        assert!(ctx.svm.get_account(&ctx.escrow.0).map_or(true, |acc| acc.lamports == 0), "escrow must be closed");

        let maker_balance_after = ctx.svm.get_account(&ctx.maker.pubkey()).unwrap().lamports;
        assert!(
            maker_balance_after > maker_balance_before,
            "maker balance must increase due to rent refund"
        );
    }

    #[test]
    pub fn test_cancel_instruction() {
        let mut ctx = setup_make();
        let program_id = program_id();

        let token_program = TOKEN_PROGRAM_ID;
        let maker_balance_before = ctx.svm.get_account(&ctx.maker.pubkey()).unwrap().lamports;

        // Cancel instruction: discriminator 2
        let cancel_ix = Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new(ctx.maker.pubkey(), true),
                AccountMeta::new_readonly(ctx.mint_a, false),
                AccountMeta::new(ctx.escrow.0, false),
                AccountMeta::new(ctx.vault, false),
                AccountMeta::new(ctx.maker_ata_a, false),
                AccountMeta::new_readonly(token_program, false),
            ],
            data: vec![2u8],
        };

        let message = Message::new(&[cancel_ix], Some(&ctx.maker.pubkey()));
        let recent_blockhash = ctx.svm.latest_blockhash();
        let transaction = Transaction::new(&[&ctx.maker], message, recent_blockhash);

        let tx = ctx.svm.send_transaction(transaction).unwrap();
        println!("\nCancel transaction successful");
        println!("CUs Consumed: {}", tx.compute_units_consumed);

        // Maker receives full 1,000 A back
        let maker_ata_a_acc = ctx.svm.get_account(&ctx.maker_ata_a).unwrap();
        let maker_state = spl_token_2022::state::Account::unpack(&maker_ata_a_acc.data).unwrap();
        assert_eq!(maker_state.amount, 1000000000);

        assert!(ctx.svm.get_account(&ctx.vault).map_or(true, |acc| acc.lamports == 0), "vault must be closed");
        assert!(ctx.svm.get_account(&ctx.escrow.0).map_or(true, |acc| acc.lamports == 0), "escrow must be closed");

        let maker_balance_after = ctx.svm.get_account(&ctx.maker.pubkey()).unwrap().lamports;
        assert!(
            maker_balance_after > maker_balance_before,
            "maker balance must increase from refunded rent"
        );
    }

    #[test]
    pub fn test_take_insufficient_balance() {
        let mut ctx = setup_make();
        let program_id = program_id();

        let taker = Keypair::new();
        ctx.svm
            .airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL)
            .expect("Airdrop to taker failed");

        // Taker mints only 50 B (needs 100 B)
        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut ctx.svm, &taker, &ctx.mint_b)
            .owner(&taker.pubkey())
            .send()
            .unwrap();

        let half_amount = ctx.amount_to_receive / 2;
        MintTo::new(&mut ctx.svm, &ctx.maker, &ctx.mint_b, &taker_ata_b, half_amount)
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

        let associated_token_program = ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();
        let token_program = TOKEN_PROGRAM_ID;
        let system_program = solana_sdk_ids::system_program::ID;

        let take_ix = Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new(taker.pubkey(), true),
                AccountMeta::new(ctx.maker.pubkey(), false),
                AccountMeta::new_readonly(ctx.mint_a, false),
                AccountMeta::new_readonly(ctx.mint_b, false),
                AccountMeta::new(ctx.escrow.0, false),
                AccountMeta::new(ctx.vault, false),
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
        let recent_blockhash = ctx.svm.latest_blockhash();
        let transaction = Transaction::new(&[&taker], message, recent_blockhash);

        let result = ctx.svm.send_transaction(transaction);
        assert!(result.is_err(), "underfunded take transaction must fail");

        // Confirm vault balance is completely untouched
        let vault_acc = ctx.svm.get_account(&ctx.vault).unwrap();
        let vault_state = spl_token_2022::state::Account::unpack(&vault_acc.data).unwrap();
        assert_eq!(vault_state.amount, ctx.amount_to_give);
    }

    #[test]
    pub fn test_cancel_unauthorized_signer() {
        let mut ctx = setup_make();
        let program_id = program_id();

        let stranger = Keypair::new();
        ctx.svm
            .airdrop(&stranger.pubkey(), 10 * LAMPORTS_PER_SOL)
            .expect("Airdrop to stranger failed");

        let stranger_ata_a = spl_associated_token_account::get_associated_token_address(
            &stranger.pubkey(),
            &ctx.mint_a,
        );

        let token_program = TOKEN_PROGRAM_ID;

        // Stranger tries to cancel with stranger as signer and recipient
        let cancel_ix = Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new(stranger.pubkey(), true),
                AccountMeta::new_readonly(ctx.mint_a, false),
                AccountMeta::new(ctx.escrow.0, false),
                AccountMeta::new(ctx.vault, false),
                AccountMeta::new(stranger_ata_a, false),
                AccountMeta::new_readonly(token_program, false),
            ],
            data: vec![2u8],
        };

        let message = Message::new(&[cancel_ix], Some(&stranger.pubkey()));
        let recent_blockhash = ctx.svm.latest_blockhash();
        let transaction = Transaction::new(&[&stranger], message, recent_blockhash);

        let result = ctx.svm.send_transaction(transaction);
        assert!(result.is_err(), "a stranger must not be able to cancel");

        // Verify tokens are still in vault
        let vault_acc = ctx.svm.get_account(&ctx.vault).unwrap();
        let vault_state = spl_token_2022::state::Account::unpack(&vault_acc.data).unwrap();
        assert_eq!(vault_state.amount, ctx.amount_to_give);
    }

    #[test]
    pub fn test_take_mismatched_maker() {
        let mut ctx = setup_make();
        let program_id = program_id();

        let taker = Keypair::new();
        let stranger = Keypair::new();
        ctx.svm
            .airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL)
            .expect("Airdrop to taker failed");

        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut ctx.svm, &taker, &ctx.mint_b)
            .owner(&taker.pubkey())
            .send()
            .unwrap();

        MintTo::new(&mut ctx.svm, &ctx.maker, &ctx.mint_b, &taker_ata_b, ctx.amount_to_receive)
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

        let associated_token_program = ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();
        let token_program = TOKEN_PROGRAM_ID;
        let system_program = solana_sdk_ids::system_program::ID;

        // Send Take with stranger as maker
        let take_ix = Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new(taker.pubkey(), true),
                AccountMeta::new(stranger.pubkey(), false), // Wrong maker!
                AccountMeta::new_readonly(ctx.mint_a, false),
                AccountMeta::new_readonly(ctx.mint_b, false),
                AccountMeta::new(ctx.escrow.0, false),
                AccountMeta::new(ctx.vault, false),
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
        let recent_blockhash = ctx.svm.latest_blockhash();
        let transaction = Transaction::new(&[&taker], message, recent_blockhash);

        let result = ctx.svm.send_transaction(transaction);
        assert!(result.is_err(), "take with mismatched maker must fail");

        // Verify tokens are still in vault
        let vault_acc = ctx.svm.get_account(&ctx.vault).unwrap();
        let vault_state = spl_token_2022::state::Account::unpack(&vault_acc.data).unwrap();
        assert_eq!(vault_state.amount, ctx.amount_to_give);
    }
}