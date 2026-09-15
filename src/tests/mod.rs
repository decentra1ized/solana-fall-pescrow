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

    const PROGRAM_ID: &str = "29eCn3KRBeWL5fYUHPKVHJzKW6Uc8VFbJsR9f7RyTKBp";
    const TOKEN_PROGRAM_ID: Pubkey = spl_token::ID;
    const ASSOCIATED_TOKEN_PROGRAM_ID: &str = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL";

    // 1000 / 500 / 100 tokens at 6 decimals.
    const MINT_AMOUNT: u64 = 1_000_000_000;
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

    /// Everything a test needs after a successful `Make`.
    struct EscrowCtx {
        mint_a: Pubkey,
        mint_b: Pubkey,
        maker_ata_a: Pubkey,
        escrow: Pubkey,
        bump: u8,
        vault: Pubkey,
    }

    /// Factor the Make setup out of `test_make_instruction`: two mints, a funded
    /// maker ATA, a Make transaction, then the next steps (Take, Cancel) are
    /// handed the addresses they need. Everything the original test set up by
    /// hand lives here.
    fn make_escrow(svm: &mut LiteSVM, maker: &Keypair) -> (EscrowCtx, u64) {
        let program_id = program_id();
        let associated_token_program = ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();
        let token_program = TOKEN_PROGRAM_ID;
        let system_program = solana_sdk_ids::system_program::ID;

        // Create Mint A: the token the maker deposits.
        let mint_a = CreateMint::new(svm, maker)
            .decimals(6)
            .authority(&maker.pubkey())
            .send()
            .unwrap();
        println!("Mint A: {}", mint_a);

        // Create Mint B: the token the maker wants back.
        let mint_b = CreateMint::new(svm, maker)
            .decimals(6)
            .authority(&maker.pubkey())
            .send()
            .unwrap();
        println!("Mint B: {}", mint_b);

        // Create the maker's associated token account for Mint A
        let maker_ata_a = CreateAssociatedTokenAccount::new(svm, maker, &mint_a)
            .owner(&maker.pubkey()).send().unwrap();
        println!("Maker ATA A: {}\n", maker_ata_a);

        // Derive the PDA for the escrow account using the maker's public key and a seed value
        let escrow = Pubkey::find_program_address(
            &[b"escrow".as_ref(), maker.pubkey().as_ref()],
            &PROGRAM_ID.parse().unwrap(),
        );
        println!("Escrow PDA: {}\n", escrow.0);

        // Derive the PDA for the vault associated token account using the escrow PDA and Mint A
        let vault = spl_associated_token_account::get_associated_token_address(
            &escrow.0,  // owner will be the escrow PDA
            &mint_a     // mint
        );
        println!("Vault PDA: {}\n", vault);

        // Mint 1,000 tokens (with 6 decimal places) of Mint A to the maker's associated token account
        MintTo::new(svm, maker, &mint_a, &maker_ata_a, MINT_AMOUNT)
            .send()
            .unwrap();

        // 100 tokens with 6 decimal places / 500 tokens with 6 decimal places
        let amount_to_receive = AMOUNT_TO_RECEIVE;
        let amount_to_give = AMOUNT_TO_GIVE;
        let bump: u8 = escrow.1;   // canonical bump; the program derives the same one on-chain
        println!("Bump: {}", bump);

        // Create the "Make" instruction to deposit tokens into the escrow
        let make_data = [
            vec![0u8],              // Discriminator for "Make" instruction
            amount_to_receive.to_le_bytes().to_vec(),
            amount_to_give.to_le_bytes().to_vec(),
        ].concat();
        let make_ix = Instruction {
            program_id: program_id,
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

        // Create and send the transaction containing the "Make" instruction
        let message = Message::new(&[make_ix], Some(&maker.pubkey()));
        let recent_blockhash = svm.latest_blockhash();
        let transaction = Transaction::new(&[maker], message, recent_blockhash);

        // Send the transaction and capture the result
        let tx = svm.send_transaction(transaction).unwrap();

        // Log transaction details
        println!("\n\nMake transaction successful");
        println!("CUs Consumed: {}", tx.compute_units_consumed);

        let ctx = EscrowCtx {
            mint_a,
            mint_b,
            maker_ata_a,
            escrow: escrow.0,
            bump,
            vault,
        };
        (ctx, tx.compute_units_consumed)
    }

    fn send_signed(
        svm: &mut LiteSVM,
        signer: &Keypair,
        ix: Instruction,
    ) -> litesvm::types::TransactionResult {
        let message = Message::new(&[ix], Some(&signer.pubkey()));
        let recent_blockhash = svm.latest_blockhash();
        let transaction = Transaction::new(&[signer], message, recent_blockhash);
        svm.send_transaction(transaction)
    }

    /// A closed account either no longer exists, or sits at 0 lamports with the
    /// system program as owner (runtime close semantics).
    fn assert_closed(svm: &LiteSVM, address: &Pubkey) {
        match svm.get_account(address) {
            None => {}
            Some(acc) => {
                assert_eq!(acc.lamports, 0, "account {address} should be closed");
                assert_eq!(acc.owner, solana_sdk_ids::system_program::ID);
            }
        }
    }

    fn token_balance(svm: &LiteSVM, account: &Pubkey) -> u64 {
        let acc = svm.get_account(account).expect("token account should exist");
        spl_token_2022::state::Account::unpack(&acc.data).unwrap().amount
    }

    fn sol_balance(svm: &LiteSVM, account: &Pubkey) -> u64 {
        svm.get_account(account).map(|a| a.lamports).unwrap_or(0)
    }

    #[test]
    pub fn test_make_instruction() {
        let (mut svm, payer) = setup();
        assert_eq!(program_id().to_string(), PROGRAM_ID);

        // The Make setup itself (mints, maker ATA, Make transaction) now runs in
        // the shared `make_escrow` helper; the rest of this test is the
        // extra verification step from the guide: reading the state back.
        let (ctx, _cus) = make_escrow(&mut svm, &payer);

        // --- extra verification ---
        let vault_acc = svm.get_account(&ctx.vault).unwrap();
        let vault_state = spl_token_2022::state::Account::unpack(&vault_acc.data).unwrap();
        println!("Vault owner: {} (escrow PDA? {})", vault_state.owner, vault_state.owner == ctx.escrow);
        println!("Vault balance: {}", vault_state.amount);
        assert_eq!(vault_state.amount, AMOUNT_TO_GIVE);

        let maker_acc = svm.get_account(&ctx.maker_ata_a).unwrap();
        let maker_state = spl_token_2022::state::Account::unpack(&maker_acc.data).unwrap();
        println!("Maker ATA balance: {}", maker_state.amount);
        assert_eq!(maker_state.amount, MINT_AMOUNT - AMOUNT_TO_GIVE);

        let esc = svm.get_account(&ctx.escrow).unwrap();
        println!("Escrow account owner: {} (program? {})", esc.owner, esc.owner == program_id());
        println!("Escrow data len: {}", esc.data.len());
        let d = &esc.data;
        println!("  maker   = {}", Pubkey::new_from_array(d[0..32].try_into().unwrap()));
        println!("  mint_a  = {}", Pubkey::new_from_array(d[32..64].try_into().unwrap()));
        println!("  mint_b  = {}", Pubkey::new_from_array(d[64..96].try_into().unwrap()));
        println!("  receive = {}", u64::from_le_bytes(d[96..104].try_into().unwrap()));
        println!("  give    = {}", u64::from_le_bytes(d[104..112].try_into().unwrap()));
        println!("  bump    = {}", d[112]);
        assert_eq!(&d[0..32], payer.pubkey().as_ref());
        assert_eq!(d[32..64], ctx.mint_a.to_bytes());
        assert_eq!(d[64..96], ctx.mint_b.to_bytes());
        assert_eq!(u64::from_le_bytes(d[96..104].try_into().unwrap()), AMOUNT_TO_RECEIVE);
        assert_eq!(u64::from_le_bytes(d[104..112].try_into().unwrap()), AMOUNT_TO_GIVE);
        assert_eq!(d[112], ctx.bump);
    }

    #[test]
    pub fn test_take_instruction() {
        let (mut svm, maker) = setup();
        let (ctx, _) = make_escrow(&mut svm, &maker);

        // Second keypair: the taker (not the maker).
        let taker = Keypair::new();
        svm.airdrop(&taker.pubkey(), 5 * LAMPORTS_PER_SOL).unwrap();

        // The taker has B to offer (100 of mint B at 6 decimals).
        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut svm, &taker, &ctx.mint_b)
            .owner(&taker.pubkey()).send().unwrap();
        MintTo::new(&mut svm, &maker, &ctx.mint_b, &taker_ata_b, AMOUNT_TO_RECEIVE)
            .send().unwrap();

        // Destination ATAs do NOT exist yet: the program must create them via
        // CreateIdempotent.
        let taker_ata_a = spl_associated_token_account::get_associated_token_address(
            &taker.pubkey(), &ctx.mint_a,
        );
        let maker_ata_b = spl_associated_token_account::get_associated_token_address(
            &maker.pubkey(), &ctx.mint_b,
        );

        let system_program = solana_sdk_ids::system_program::ID;

        // Create the "Take" instruction, signed by the taker: the 12 accounts in
        // the order documented at the top of `take.rs`.
        let take_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(taker.pubkey(), true),
                AccountMeta::new(maker.pubkey(), false),
                AccountMeta::new(ctx.mint_a, false),
                AccountMeta::new(ctx.mint_b, false),
                AccountMeta::new(ctx.escrow, false),
                AccountMeta::new(ctx.vault, false),
                AccountMeta::new(taker_ata_a, false),
                AccountMeta::new(taker_ata_b, false),
                AccountMeta::new(maker_ata_b, false),
                AccountMeta::new(system_program, false),
                AccountMeta::new(TOKEN_PROGRAM_ID, false),
                AccountMeta::new(ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap(), false),
            ],
            data: vec![1u8],    // Take discriminator
        };

        // Create and send the transaction containing the "Take" instruction
        let message = Message::new(&[take_ix], Some(&taker.pubkey()));
        let recent_blockhash = svm.latest_blockhash();
        let transaction = Transaction::new(&[&taker], message, recent_blockhash);

        let maker_sol_before = sol_balance(&svm, &maker.pubkey());

        // Send the transaction and capture the result
        let tx = svm.send_transaction(transaction).unwrap();

        // Log transaction details
        println!("\n\nTake transaction successful");
        println!("CUs Consumed: {}", tx.compute_units_consumed);

        // --- extra verification ---
        // Swap happened: taker got all A, maker got B.
        assert_eq!(token_balance(&svm, &taker_ata_a), AMOUNT_TO_GIVE);
        assert_eq!(token_balance(&svm, &maker_ata_b), AMOUNT_TO_RECEIVE);

        // Vault and escrow both closed.
        assert_closed(&svm, &ctx.vault);
        assert_closed(&svm, &ctx.escrow);

        // Maker's SOL went up by the rent of the two closed accounts.
        let maker_sol_after = sol_balance(&svm, &maker.pubkey());
        println!("Maker SOL before: {maker_sol_before}, after: {maker_sol_after}");
        assert!(maker_sol_after > maker_sol_before, "maker should receive the rent refunds");
    }

    #[test]
    pub fn test_cancel_instruction() {
        let (mut svm, maker) = setup();
        let (ctx, _) = make_escrow(&mut svm, &maker);

        let maker_ata_a = ctx.maker_ata_a;

        // Create the "Cancel" instruction, signed by the maker: the 6 accounts
        // in the order documented at the top of `cancel.rs`.
        let cancel_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(maker.pubkey(), true),
                AccountMeta::new(ctx.mint_a, false),
                AccountMeta::new(ctx.escrow, false),
                AccountMeta::new(ctx.vault, false),
                AccountMeta::new(maker_ata_a, false),
                AccountMeta::new(TOKEN_PROGRAM_ID, false),
            ],
            data: vec![2u8],    // Cancel discriminator
        };

        // Send the transaction and capture the result
        let tx = send_signed(&mut svm, &maker, cancel_ix).unwrap();
        println!("\nCancel transaction successful");
        println!("CUs Consumed: {}", tx.compute_units_consumed);

        // --- extra verification ---
        // Maker has all A back.
        assert_eq!(token_balance(&svm, &maker_ata_a), MINT_AMOUNT);
        assert_closed(&svm, &ctx.vault);
        assert_closed(&svm, &ctx.escrow);
    }

    #[test]
    pub fn test_take_underfunded_fails() {
        let (mut svm, maker) = setup();
        let (ctx, _) = make_escrow(&mut svm, &maker);

        let taker = Keypair::new();
        svm.airdrop(&taker.pubkey(), 5 * LAMPORTS_PER_SOL).unwrap();

        // Taker only has 50 B, but must pay 100 B. Transfer must be rejected by
        // the token program and nothing may move.
        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut svm, &taker, &ctx.mint_b)
            .owner(&taker.pubkey()).send().unwrap();
        MintTo::new(&mut svm, &maker, &ctx.mint_b, &taker_ata_b, AMOUNT_TO_RECEIVE / 2)
            .send().unwrap();

        let taker_ata_a = spl_associated_token_account::get_associated_token_address(
            &taker.pubkey(), &ctx.mint_a,
        );
        let maker_ata_b = spl_associated_token_account::get_associated_token_address(
            &maker.pubkey(), &ctx.mint_b,
        );

        let take_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(taker.pubkey(), true),
                AccountMeta::new(maker.pubkey(), false),
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

        // A failed transaction returns an `Err`; assert it, then prove nothing moved.
        let result = send_signed(&mut svm, &taker, take_ix);
        assert!(result.is_err(), "an underfunded taker must not complete a Take");

        // Vault untouched; escrow untouched; no swap happened.
        assert_eq!(token_balance(&svm, &ctx.vault), AMOUNT_TO_GIVE);
        assert!(svm.get_account(&ctx.escrow).is_some());
    }

    #[test]
    pub fn test_cancel_by_stranger_fails() {
        let (mut svm, maker) = setup();
        let (ctx, _) = make_escrow(&mut svm, &maker);

        // A stranger signs the Cancel. It must fail: the maker check and the
        // signer check both hold.
        let stranger = Keypair::new();
        svm.airdrop(&stranger.pubkey(), 5 * LAMPORTS_PER_SOL).unwrap();

        let cancel_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(stranger.pubkey(), true),
                AccountMeta::new(ctx.mint_a, false),
                AccountMeta::new(ctx.escrow, false),
                AccountMeta::new(ctx.vault, false),
                AccountMeta::new(ctx.maker_ata_a, false),
                AccountMeta::new(TOKEN_PROGRAM_ID, false),
            ],
            data: vec![2u8],
        };

        let result = send_signed(&mut svm, &stranger, cancel_ix);
        assert!(result.is_err(), "a stranger must not be able to cancel");

        // Nothing moved: the vault still holds all of A, escrow untouched.
        assert_eq!(token_balance(&svm, &ctx.vault), AMOUNT_TO_GIVE);
        assert!(svm.get_account(&ctx.escrow).is_some());
    }

    #[test]
    pub fn test_take_mismatched_maker_fails() {
        let (mut svm, maker) = setup();
        let (ctx, _) = make_escrow(&mut svm, &maker);

        let taker = Keypair::new();
        svm.airdrop(&taker.pubkey(), 5 * LAMPORTS_PER_SOL).unwrap();

        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut svm, &taker, &ctx.mint_b)
            .owner(&taker.pubkey()).send().unwrap();
        MintTo::new(&mut svm, &maker, &ctx.mint_b, &taker_ata_b, AMOUNT_TO_RECEIVE)
            .send().unwrap();

        let taker_ata_a = spl_associated_token_account::get_associated_token_address(
            &taker.pubkey(), &ctx.mint_a,
        );
        let maker_ata_b = spl_associated_token_account::get_associated_token_address(
            &maker.pubkey(), &ctx.mint_b,
        );

        // Pass a wrong "maker" (the taker, lying about who made the escrow) as
        // account 1. Step 3 of Take cross-checks it against escrow.maker() and
        // must fail before any tokens move.
        let take_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(taker.pubkey(), true),
                AccountMeta::new(taker.pubkey(), false),
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

        let result = send_signed(&mut svm, &taker, take_ix);
        assert!(result.is_err(), "Take must reject a maker that does not match the escrow");
        assert_eq!(token_balance(&svm, &ctx.vault), AMOUNT_TO_GIVE);
    }
}