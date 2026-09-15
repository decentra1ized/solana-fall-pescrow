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

    const AMOUNT_TO_GIVE: u64 = 500_000_000;    // 500 tokens, 6 decimals
    const AMOUNT_TO_RECEIVE: u64 = 100_000_000; // 100 tokens, 6 decimals
    const MAKER_MINTED: u64 = 1_000_000_000;    // 1000 tokens, 6 decimals

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
        vault: Pubkey,
        escrow: Pubkey,
        bump: u8,
    }

    /// Create the two mints, the maker's funded ATA for A, and run Make.
    /// Returns everything the Take and Cancel tests need.
    fn make_escrow(mut svm: LiteSVM, maker: Keypair) -> MakeContext {
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

        MintTo::new(&mut svm, &maker, &mint_a, &maker_ata_a, MAKER_MINTED)
            .send()
            .unwrap();

        let (escrow, bump) = Pubkey::find_program_address(
            &[b"escrow".as_ref(), maker.pubkey().as_ref()],
            &program_id(),
        );

        let vault = spl_associated_token_account::get_associated_token_address(
            &escrow,
            &mint_a,
        );

        let make_data = [
            vec![0u8],
            AMOUNT_TO_RECEIVE.to_le_bytes().to_vec(),
            AMOUNT_TO_GIVE.to_le_bytes().to_vec(),
        ].concat();
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
                AccountMeta::new(ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap(), false),
            ],
            data: make_data,
        };

        let message = Message::new(&[make_ix], Some(&maker.pubkey()));
        let recent_blockhash = svm.latest_blockhash();
        let transaction = Transaction::new(&[&maker], message, recent_blockhash);
        let tx = svm.send_transaction(transaction).unwrap();
        println!("Make CUs Consumed: {}", tx.compute_units_consumed);

        MakeContext {
            svm,
            maker,
            mint_a,
            mint_b,
            maker_ata_a,
            vault,
            escrow,
            bump,
        }
    }

    fn token_balance(svm: &LiteSVM, address: &Pubkey) -> u64 {
        let acc = svm.get_account(address).expect("token account not found");
        spl_token_2022::state::Account::unpack(&acc.data)
            .expect("not a valid token account")
            .amount
    }

    fn assert_closed(svm: &LiteSVM, address: &Pubkey) {
        match svm.get_account(address) {
            None => {}
            Some(acc) => {
                assert_eq!(acc.lamports, 0, "{address} should be closed");
                assert_eq!(acc.owner, solana_sdk_ids::system_program::ID);
            }
        }
    }

    #[test]
    pub fn test_make_instruction() {
        let (svm, maker) = setup();
        let ctx = make_escrow(svm, maker);

        let vault_state_acc = ctx.svm.get_account(&ctx.vault).unwrap();
        let vault_state = spl_token_2022::state::Account::unpack(&vault_state_acc.data).unwrap();
        println!("Vault owner: {} (escrow PDA? {})", vault_state.owner, vault_state.owner == ctx.escrow);
        println!("Vault balance: {}", vault_state.amount);
        assert_eq!(vault_state.amount, AMOUNT_TO_GIVE);

        let maker_balance = token_balance(&ctx.svm, &ctx.maker_ata_a);
        println!("Maker ATA balance: {}", maker_balance);
        assert_eq!(maker_balance, MAKER_MINTED - AMOUNT_TO_GIVE);

        let esc = ctx.svm.get_account(&ctx.escrow).unwrap();
        assert_eq!(esc.owner, program_id());
        assert_eq!(esc.data.len(), 113);
        let d = &esc.data;
        assert_eq!(&d[0..32], ctx.maker.pubkey().as_ref());
        assert_eq!(u64::from_le_bytes(d[96..104].try_into().unwrap()), AMOUNT_TO_RECEIVE);
        assert_eq!(u64::from_le_bytes(d[104..112].try_into().unwrap()), AMOUNT_TO_GIVE);
        assert_eq!(d[112], ctx.bump);
    }

    #[test]
    fn test_take_instruction() {
        let (svm, maker) = setup();
        let mut ctx = make_escrow(svm, maker);

        let taker = Keypair::new();
        ctx.svm.airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

        // Taker holds 100 B; will pay that for the 500 A.
        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut ctx.svm, &taker, &ctx.mint_b)
            .owner(&taker.pubkey())
            .send()
            .unwrap();
        MintTo::new(&mut ctx.svm, &ctx.maker, &ctx.mint_b, &taker_ata_b, AMOUNT_TO_RECEIVE)
            .send()
            .unwrap();

        // Destination ATAs are deliberately NOT created; the program must create them.
        let taker_ata_a = spl_associated_token_account::get_associated_token_address(
            &taker.pubkey(),
            &ctx.mint_a,
        );
        let maker_ata_b = spl_associated_token_account::get_associated_token_address(
            &ctx.maker.pubkey(),
            &ctx.mint_b,
        );

        let maker_sol_before = ctx.svm.get_account(&ctx.maker.pubkey()).unwrap().lamports;

        let take_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(taker.pubkey(), true),
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
                AccountMeta::new(ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap(), false),
            ],
            data: vec![1u8],
        };

        let message = Message::new(&[take_ix], Some(&taker.pubkey()));
        let recent_blockhash = ctx.svm.latest_blockhash();
        let transaction = Transaction::new(&[&taker], message, recent_blockhash);
        let tx = ctx.svm.send_transaction(transaction).unwrap();
        println!("Take CUs Consumed: {}", tx.compute_units_consumed);

        assert_eq!(token_balance(&ctx.svm, &taker_ata_a), AMOUNT_TO_GIVE);
        assert_eq!(token_balance(&ctx.svm, &maker_ata_b), AMOUNT_TO_RECEIVE);

        // The vault and escrow must be closed.
        assert_closed(&ctx.svm, &ctx.vault);
        assert_closed(&ctx.svm, &ctx.escrow);

        // Maker's SOL went up by (roughly) the rent of the closed vault + escrow.
        let maker_sol_after = ctx.svm.get_account(&ctx.maker.pubkey()).unwrap().lamports;
        println!("Maker SOL delta: {}", maker_sol_after - maker_sol_before);
        assert!(maker_sol_after > maker_sol_before);
    }

    #[test]
    fn test_cancel_instruction() {
        let (svm, maker) = setup();
        let mut ctx = make_escrow(svm, maker);

        let cancel_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(ctx.maker.pubkey(), true),
                AccountMeta::new(ctx.mint_a, false),
                AccountMeta::new(ctx.escrow, false),
                AccountMeta::new(ctx.vault, false),
                AccountMeta::new(ctx.maker_ata_a, false),
                AccountMeta::new(TOKEN_PROGRAM_ID, false),
            ],
            data: vec![2u8],
        };

        let message = Message::new(&[cancel_ix], Some(&ctx.maker.pubkey()));
        let recent_blockhash = ctx.svm.latest_blockhash();
        let transaction = Transaction::new(&[&ctx.maker], message, recent_blockhash);
        let tx = ctx.svm.send_transaction(transaction).unwrap();
        println!("Cancel CUs Consumed: {}", tx.compute_units_consumed);

        // All the A is back with the maker.
        assert_eq!(token_balance(&ctx.svm, &ctx.maker_ata_a), MAKER_MINTED);

        assert_closed(&ctx.svm, &ctx.vault);
        assert_closed(&ctx.svm, &ctx.escrow);
    }

    #[test]
    fn test_take_underfunded_taker_fails() {
        let (svm, maker) = setup();
        let mut ctx = make_escrow(svm, maker);

        let taker = Keypair::new();
        ctx.svm.airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

        // Taker only has 50 B, but the escrow asks for 100 B.
        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut ctx.svm, &taker, &ctx.mint_b)
            .owner(&taker.pubkey())
            .send()
            .unwrap();
        MintTo::new(&mut ctx.svm, &ctx.maker, &ctx.mint_b, &taker_ata_b, AMOUNT_TO_RECEIVE / 2)
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

        let take_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(taker.pubkey(), true),
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
                AccountMeta::new(ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap(), false),
            ],
            data: vec![1u8],
        };

        let message = Message::new(&[take_ix], Some(&taker.pubkey()));
        let recent_blockhash = ctx.svm.latest_blockhash();
        let transaction = Transaction::new(&[&taker], message, recent_blockhash);
        let result = ctx.svm.send_transaction(transaction);

        assert!(result.is_err(), "an underfunded taker must not be able to take");

        // Nothing moved: the vault still holds all of the A.
        assert_eq!(token_balance(&ctx.svm, &ctx.vault), AMOUNT_TO_GIVE);
        assert_eq!(token_balance(&ctx.svm, &ctx.maker_ata_a), MAKER_MINTED - AMOUNT_TO_GIVE);
    }

    #[test]
    fn test_cancel_by_stranger_fails() {
        let (svm, maker) = setup();
        let mut ctx = make_escrow(svm, maker);

        let stranger = Keypair::new();
        ctx.svm.airdrop(&stranger.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

        let cancel_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                // The stranger signs, but is NOT the stored maker.
                AccountMeta::new(stranger.pubkey(), true),
                AccountMeta::new(ctx.mint_a, false),
                AccountMeta::new(ctx.escrow, false),
                AccountMeta::new(ctx.vault, false),
                AccountMeta::new(ctx.maker_ata_a, false),
                AccountMeta::new(TOKEN_PROGRAM_ID, false),
            ],
            data: vec![2u8],
        };

        let message = Message::new(&[cancel_ix], Some(&stranger.pubkey()));
        let recent_blockhash = ctx.svm.latest_blockhash();
        let transaction = Transaction::new(&[&stranger], message, recent_blockhash);
        let result = ctx.svm.send_transaction(transaction);

        assert!(result.is_err(), "a stranger must not be able to cancel");

        // The 500 A are still in the vault.
        assert_eq!(token_balance(&ctx.svm, &ctx.vault), AMOUNT_TO_GIVE);
        assert_eq!(token_balance(&ctx.svm, &ctx.maker_ata_a), MAKER_MINTED - AMOUNT_TO_GIVE);
    }

    #[test]
    fn test_take_mismatched_maker_fails() {
        let (svm, maker) = setup();
        let mut ctx = make_escrow(svm, maker);

        let taker = Keypair::new();
        ctx.svm.airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

        // The taker is funded, but passes a wrong "maker" account (someone else).
        let fake_maker = Keypair::new();
        ctx.svm.airdrop(&fake_maker.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut ctx.svm, &taker, &ctx.mint_b)
            .owner(&taker.pubkey())
            .send()
            .unwrap();
        MintTo::new(&mut ctx.svm, &ctx.maker, &ctx.mint_b, &taker_ata_b, AMOUNT_TO_RECEIVE)
            .send()
            .unwrap();

        let taker_ata_a = spl_associated_token_account::get_associated_token_address(
            &taker.pubkey(),
            &ctx.mint_a,
        );
        let maker_ata_b = spl_associated_token_account::get_associated_token_address(
            &fake_maker.pubkey(),
            &ctx.mint_b,
        );

        let take_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(taker.pubkey(), true),
                AccountMeta::new(fake_maker.pubkey(), false),
                AccountMeta::new(ctx.mint_a, false),
                AccountMeta::new(ctx.mint_b, false),
                AccountMeta::new(ctx.escrow, false),
                AccountMeta::new(ctx.vault, false),
                AccountMeta::new(taker_ata_a, false),
                AccountMeta::new(taker_ata_b, false),
                AccountMeta::new(maker_ata_b, false),
                AccountMeta::new(solana_sdk_ids::system_program::ID, false),
                AccountMeta::new(TOKEN_PROGRAM_ID, false),
                AccountMeta::new(ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap(), false),
            ],
            data: vec![1u8],
        };

        let message = Message::new(&[take_ix], Some(&taker.pubkey()));
        let recent_blockhash = ctx.svm.latest_blockhash();
        let transaction = Transaction::new(&[&taker], message, recent_blockhash);
        let result = ctx.svm.send_transaction(transaction);

        assert!(result.is_err(), "Take must reject a maker that is not the stored one");

        // Escrow is untouched.
        assert_eq!(token_balance(&ctx.svm, &ctx.vault), AMOUNT_TO_GIVE);
    }
}