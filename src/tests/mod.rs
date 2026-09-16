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

    struct EscrowTestContext {
        svm: LiteSVM,
        maker: Keypair,
        mint_a: Pubkey,
        mint_b: Pubkey,
        maker_ata_a: Pubkey,
        escrow: Pubkey,
        bump: u8,
        vault: Pubkey,
        amount_to_receive: u64,
        amount_to_give: u64,
    }

    fn setup_escrow() -> EscrowTestContext {
        let (mut svm, maker) = setup();
        let program_id = program_id();

        assert_eq!(program_id.to_string(), PROGRAM_ID);

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

        // Create the maker's associated token account for Mint A
        let maker_ata_a = CreateAssociatedTokenAccount::new(&mut svm, &maker, &mint_a)
            .owner(&maker.pubkey())
            .send()
            .unwrap();

        // Derive the PDA for the escrow account using the maker's public key and a seed value
        let escrow = Pubkey::find_program_address(
            &[b"escrow".as_ref(), maker.pubkey().as_ref()],
            &program_id,
        );

        // Derive the PDA for the vault associated token account using the escrow PDA and Mint A
        let vault = spl_associated_token_account::get_associated_token_address(
            &escrow.0,
            &mint_a,
        );

        let associated_token_program = ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();
        let token_program = TOKEN_PROGRAM_ID;
        let system_program = solana_sdk_ids::system_program::ID;

        // Mint 1,000 tokens (with 6 decimal places) of Mint A to the maker's associated token account
        MintTo::new(&mut svm, &maker, &mint_a, &maker_ata_a, 1000000000)
            .send()
            .unwrap();

        let amount_to_receive: u64 = 100000000; // 100 tokens with 6 decimal places
        let amount_to_give: u64 = 500000000;    // 500 tokens with 6 decimal places
        let bump: u8 = escrow.1;

        // Create the "Make" instruction to deposit tokens into the escrow
        let make_data = [
            vec![0u8],
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

        println!("\n\nMake transaction successful");
        println!("CUs Consumed: {}", tx.compute_units_consumed);

        EscrowTestContext {
            svm,
            maker,
            mint_a,
            mint_b,
            maker_ata_a,
            escrow: escrow.0,
            bump,
            vault,
            amount_to_receive,
            amount_to_give,
        }
    }

    #[test]
    pub fn test_make_instruction() {
        let ctx = setup_escrow();
        let program_id = program_id();

        let vault_acc = ctx.svm.get_account(&ctx.vault).unwrap();
        let vault_state = spl_token_2022::state::Account::unpack(&vault_acc.data).unwrap();
        println!("Vault owner: {} (escrow PDA? {})", vault_state.owner, vault_state.owner == ctx.escrow);
        println!("Vault balance: {}", vault_state.amount);
        assert_eq!(vault_state.amount, ctx.amount_to_give);

        let maker_acc = ctx.svm.get_account(&ctx.maker_ata_a).unwrap();
        let maker_state = spl_token_2022::state::Account::unpack(&maker_acc.data).unwrap();
        println!("Maker ATA balance: {}", maker_state.amount);
        assert_eq!(maker_state.amount, 1000000000 - ctx.amount_to_give);

        let esc = ctx.svm.get_account(&ctx.escrow).unwrap();
        println!("Escrow account owner: {} (program? {})", esc.owner, esc.owner == program_id);
        println!("Escrow data len: {}", esc.data.len());
        let d = &esc.data;
        println!("  maker   = {}", Pubkey::new_from_array(d[0..32].try_into().unwrap()));
        println!("  mint_a  = {}", Pubkey::new_from_array(d[32..64].try_into().unwrap()));
        println!("  mint_b  = {}", Pubkey::new_from_array(d[64..96].try_into().unwrap()));
        println!("  receive = {}", u64::from_le_bytes(d[96..104].try_into().unwrap()));
        println!("  give    = {}", u64::from_le_bytes(d[104..112].try_into().unwrap()));
        println!("  bump    = {}", d[112]);
        assert_eq!(&d[0..32], ctx.maker.pubkey().as_ref());
        assert_eq!(&d[32..64], ctx.mint_a.as_ref());
        assert_eq!(&d[64..96], ctx.mint_b.as_ref());
        assert_eq!(u64::from_le_bytes(d[96..104].try_into().unwrap()), ctx.amount_to_receive);
        assert_eq!(u64::from_le_bytes(d[104..112].try_into().unwrap()), ctx.amount_to_give);
        assert_eq!(d[112], ctx.bump);
    }

    #[test]
    pub fn test_take_instruction() {
        let mut ctx = setup_escrow();

        let taker = Keypair::new();
        ctx.svm
            .airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL)
            .expect("Taker airdrop failed");

        // Create taker_ata_b and mint 100 B into it
        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut ctx.svm, &taker, &ctx.mint_b)
            .owner(&taker.pubkey())
            .send()
            .unwrap();

        MintTo::new(&mut ctx.svm, &ctx.maker, &ctx.mint_b, &taker_ata_b, ctx.amount_to_receive)
            .send()
            .unwrap();

        // Derive taker_ata_a and maker_ata_b with get_associated_token_address (not created yet)
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

        let maker_lamports_before = ctx.svm.get_account(&ctx.maker.pubkey()).unwrap().lamports;

        // Take instruction with 12 accounts in the designated order
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
                AccountMeta::new(system_program, false),
                AccountMeta::new(token_program, false),
                AccountMeta::new(associated_token_program, false),
            ],
            data: vec![1u8],
        };

        let message = Message::new(&[take_ix], Some(&taker.pubkey()));
        let recent_blockhash = ctx.svm.latest_blockhash();
        let tx = ctx
            .svm
            .send_transaction(Transaction::new(&[&taker], message, recent_blockhash))
            .unwrap();

        println!("\n\nTake transaction successful");
        println!("CUs Consumed: {}", tx.compute_units_consumed);

        // Assert taker_ata_a.amount: 500 A
        let taker_a_acc = ctx.svm.get_account(&taker_ata_a).unwrap();
        let taker_a_state = spl_token_2022::state::Account::unpack(&taker_a_acc.data).unwrap();
        println!("Taker ATA A balance: {}", taker_a_state.amount);
        assert_eq!(taker_a_state.amount, ctx.amount_to_give);

        // Assert maker_ata_b.amount: 100 B
        let maker_b_acc = ctx.svm.get_account(&maker_ata_b).unwrap();
        let maker_b_state = spl_token_2022::state::Account::unpack(&maker_b_acc.data).unwrap();
        println!("Maker ATA B balance: {}", maker_b_state.amount);
        assert_eq!(maker_b_state.amount, ctx.amount_to_receive);

        // Assert vault and escrow accounts are closed (None or 0 lamports and system owner)
        if let Some(vault_acc) = ctx.svm.get_account(&ctx.vault) {
            assert_eq!(vault_acc.lamports, 0, "Closed vault should have 0 lamports");
            assert_eq!(vault_acc.owner, solana_sdk_ids::system_program::ID, "Closed vault should be system owned");
        }
        if let Some(escrow_acc) = ctx.svm.get_account(&ctx.escrow) {
            assert_eq!(escrow_acc.lamports, 0, "Closed escrow should have 0 lamports");
            assert_eq!(escrow_acc.owner, solana_sdk_ids::system_program::ID, "Closed escrow should be system owned");
        }

        // Assert maker SOL balance went up by roughly the rent of both closed accounts
        let maker_lamports_after = ctx.svm.get_account(&ctx.maker.pubkey()).unwrap().lamports;
        println!("Maker lamports before: {}, after: {}", maker_lamports_before, maker_lamports_after);
        assert!(
            maker_lamports_after > maker_lamports_before,
            "Maker lamports should increase after closing vault and escrow"
        );
    }

    #[test]
    pub fn test_cancel_instruction() {
        let mut ctx = setup_escrow();

        let token_program = TOKEN_PROGRAM_ID;
        let maker_lamports_before = ctx.svm.get_account(&ctx.maker.pubkey()).unwrap().lamports;

        // Cancel instruction with 6 accounts
        let cancel_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(ctx.maker.pubkey(), true),
                AccountMeta::new(ctx.mint_a, false),
                AccountMeta::new(ctx.escrow, false),
                AccountMeta::new(ctx.vault, false),
                AccountMeta::new(ctx.maker_ata_a, false),
                AccountMeta::new(token_program, false),
            ],
            data: vec![2u8],
        };

        let message = Message::new(&[cancel_ix], Some(&ctx.maker.pubkey()));
        let recent_blockhash = ctx.svm.latest_blockhash();
        let tx = ctx
            .svm
            .send_transaction(Transaction::new(&[&ctx.maker], message, recent_blockhash))
            .unwrap();

        println!("\n\nCancel transaction successful");
        println!("CUs Consumed: {}", tx.compute_units_consumed);

        // Assert the maker's ATA is back to 1,000 A
        let maker_acc = ctx.svm.get_account(&ctx.maker_ata_a).unwrap();
        let maker_state = spl_token_2022::state::Account::unpack(&maker_acc.data).unwrap();
        println!("Maker ATA A balance after cancel: {}", maker_state.amount);
        assert_eq!(maker_state.amount, 1000000000);

        // Assert both PDA accounts are gone (None or 0 lamports and system owner)
        if let Some(vault_acc) = ctx.svm.get_account(&ctx.vault) {
            assert_eq!(vault_acc.lamports, 0, "Closed vault should have 0 lamports");
            assert_eq!(vault_acc.owner, solana_sdk_ids::system_program::ID, "Closed vault should be system owned");
        }
        if let Some(escrow_acc) = ctx.svm.get_account(&ctx.escrow) {
            assert_eq!(escrow_acc.lamports, 0, "Closed escrow should have 0 lamports");
            assert_eq!(escrow_acc.owner, solana_sdk_ids::system_program::ID, "Closed escrow should be system owned");
        }

        // Assert maker SOL balance went up
        let maker_lamports_after = ctx.svm.get_account(&ctx.maker.pubkey()).unwrap().lamports;
        println!("Maker lamports before: {}, after: {}", maker_lamports_before, maker_lamports_after);
        assert!(
            maker_lamports_after > maker_lamports_before,
            "Maker lamports should increase after closing vault and escrow"
        );
    }

    #[test]
    pub fn test_take_with_insufficient_funds_fails() {
        let mut ctx = setup_escrow();

        let taker = Keypair::new();
        ctx.svm
            .airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL)
            .expect("Taker airdrop failed");

        // Mint only 50 B (50_000_000) instead of required 100 B
        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut ctx.svm, &taker, &ctx.mint_b)
            .owner(&taker.pubkey())
            .send()
            .unwrap();

        MintTo::new(&mut ctx.svm, &ctx.maker, &ctx.mint_b, &taker_ata_b, 50000000)
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
                AccountMeta::new(system_program, false),
                AccountMeta::new(token_program, false),
                AccountMeta::new(associated_token_program, false),
            ],
            data: vec![1u8],
        };

        let message = Message::new(&[take_ix], Some(&taker.pubkey()));
        let result = ctx
            .svm
            .send_transaction(Transaction::new(&[&taker], message, ctx.svm.latest_blockhash()));

        assert!(result.is_err(), "Take by underfunded taker must fail");

        // Confirm vault still holds 500 A
        let vault_acc = ctx.svm.get_account(&ctx.vault).unwrap();
        let vault_state = spl_token_2022::state::Account::unpack(&vault_acc.data).unwrap();
        assert_eq!(vault_state.amount, ctx.amount_to_give);

        // Confirm escrow account is still intact
        let esc = ctx.svm.get_account(&ctx.escrow).unwrap();
        assert_eq!(esc.owner, program_id());
    }

    #[test]
    pub fn test_stranger_cannot_cancel() {
        let mut ctx = setup_escrow();

        let stranger = Keypair::new();
        ctx.svm
            .airdrop(&stranger.pubkey(), 10 * LAMPORTS_PER_SOL)
            .expect("Stranger airdrop failed");

        let stranger_ata_a = spl_associated_token_account::get_associated_token_address(
            &stranger.pubkey(),
            &ctx.mint_a,
        );

        let token_program = TOKEN_PROGRAM_ID;

        // Stranger attempts to cancel maker's escrow by passing stranger as maker
        let cancel_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(stranger.pubkey(), true),
                AccountMeta::new(ctx.mint_a, false),
                AccountMeta::new(ctx.escrow, false),
                AccountMeta::new(ctx.vault, false),
                AccountMeta::new(stranger_ata_a, false),
                AccountMeta::new(token_program, false),
            ],
            data: vec![2u8],
        };

        let message = Message::new(&[cancel_ix], Some(&stranger.pubkey()));
        let result = ctx
            .svm
            .send_transaction(Transaction::new(&[&stranger], message, ctx.svm.latest_blockhash()));

        assert!(result.is_err(), "a stranger must not be able to cancel someone else's escrow");

        // Read the vault back and confirm the 500 A are still there
        let vault_acc = ctx.svm.get_account(&ctx.vault).unwrap();
        let vault_state = spl_token_2022::state::Account::unpack(&vault_acc.data).unwrap();
        assert_eq!(vault_state.amount, ctx.amount_to_give);

        // Escrow account is still open
        let esc = ctx.svm.get_account(&ctx.escrow).unwrap();
        assert_eq!(esc.owner, program_id());
    }
}