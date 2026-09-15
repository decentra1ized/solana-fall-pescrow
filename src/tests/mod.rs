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


    /// Everything the Take and Cancel tests need: a live escrow holding 500 A.
    ///
    /// Reuses `setup()` rather than building a LiteSVM by hand, so the Rent
    /// sysvar override comes along — without it every Make fails with
    /// InsufficientFundsForRent.
    struct Deal {
        svm: LiteSVM,
        maker: Keypair,
        mint_a: Pubkey,
        mint_b: Pubkey,
        maker_ata_a: Pubkey,
        escrow: Pubkey,
        vault: Pubkey,
    }

    const AMOUNT_TO_GIVE: u64 = 500_000_000;    // 500 A
    const AMOUNT_TO_RECEIVE: u64 = 100_000_000; // 100 B
    const MINTED_A: u64 = 1_000_000_000;        // 1000 A

    fn make_deal() -> Deal {
        let (mut svm, maker) = setup();

        let mint_a = CreateMint::new(&mut svm, &maker).decimals(6)
            .authority(&maker.pubkey()).send().unwrap();
        let mint_b = CreateMint::new(&mut svm, &maker).decimals(6)
            .authority(&maker.pubkey()).send().unwrap();

        let maker_ata_a = CreateAssociatedTokenAccount::new(&mut svm, &maker, &mint_a)
            .owner(&maker.pubkey()).send().unwrap();
        MintTo::new(&mut svm, &maker, &mint_a, &maker_ata_a, MINTED_A).send().unwrap();

        let (escrow, _bump) = Pubkey::find_program_address(
            &[b"escrow".as_ref(), maker.pubkey().as_ref()],
            &program_id(),
        );
        let vault = spl_associated_token_account::get_associated_token_address(&escrow, &mint_a);

        let data = [
            vec![0u8],
            AMOUNT_TO_RECEIVE.to_le_bytes().to_vec(),
            AMOUNT_TO_GIVE.to_le_bytes().to_vec(),
        ].concat();

        let ix = Instruction {
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
            data,
        };

        send(&mut svm, &maker, ix, &[&maker]).expect("make should succeed");

        Deal { svm, maker, mint_a, mint_b, maker_ata_a, escrow, vault }
    }

    fn send(
        svm: &mut LiteSVM,
        payer: &Keypair,
        ix: Instruction,
        signers: &[&Keypair],
    ) -> Result<litesvm::types::TransactionMetadata, litesvm::types::FailedTransactionMetadata> {
        let message = Message::new(&[ix], Some(&payer.pubkey()));
        let blockhash = svm.latest_blockhash();
        svm.send_transaction(Transaction::new(signers, message, blockhash))
    }

    fn token_amount(svm: &LiteSVM, address: &Pubkey) -> u64 {
        let acc = svm.get_account(address).expect("token account should exist");
        spl_token_2022::state::Account::unpack(&acc.data).unwrap().amount
    }

    /// A closed account is either gone or an empty system-owned husk.
    fn is_closed(svm: &LiteSVM, address: &Pubkey) -> bool {
        match svm.get_account(address) {
            None => true,
            Some(acc) => acc.lamports == 0 || acc.data.is_empty(),
        }
    }

    #[test]
    pub fn test_make_instruction() {
        let (mut svm, payer) = setup();

        let program_id = program_id();

        assert_eq!(program_id.to_string(), PROGRAM_ID);

        let mint_a = CreateMint::new(&mut svm, &payer)
            .decimals(6)
            .authority(&payer.pubkey())
            .send()
            .unwrap();
        println!("Mint A: {}", mint_a);

        let mint_b = CreateMint::new(&mut svm, &payer)
            .decimals(6)
            .authority(&payer.pubkey())
            .send()
            .unwrap();
        println!("Mint B: {}", mint_b);

        // Create the maker's associated token account for Mint A
        let maker_ata_a = CreateAssociatedTokenAccount::new(&mut svm, &payer, &mint_a)
            .owner(&payer.pubkey()).send().unwrap();
        println!("Maker ATA A: {}\n", maker_ata_a);

        // Derive the PDA for the escrow account using the maker's public key and a seed value
        let escrow = Pubkey::find_program_address(
            &[b"escrow".as_ref(), payer.pubkey().as_ref()],
            &PROGRAM_ID.parse().unwrap(),
        );
        println!("Escrow PDA: {}\n", escrow.0);

        // Derive the PDA for the vault associated token account using the escrow PDA and Mint A
        let vault = spl_associated_token_account::get_associated_token_address(
            &escrow.0,  // owner will be the escrow PDA
            &mint_a     // mint
        );
        println!("Vault PDA: {}\n", vault);

        // Define program IDs for associated token program, token program, and system program
        let associated_token_program = ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();
        let token_program = TOKEN_PROGRAM_ID;
        let system_program = solana_sdk_ids::system_program::ID;

        // Mint 1,000 tokens (with 6 decimal places) of Mint A to the maker's associated token account
        MintTo::new(&mut svm, &payer, &mint_a, &maker_ata_a, 1000000000)
            .send()
            .unwrap();

        let amount_to_receive: u64 = 100000000; // 100 tokens with 6 decimal places
        let amount_to_give: u64 = 500000000;    // 500 tokens with 6 decimal places
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
                AccountMeta::new(payer.pubkey(), true),
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
        let message = Message::new(&[make_ix], Some(&payer.pubkey()));
        let recent_blockhash = svm.latest_blockhash();

        let transaction = Transaction::new(&[&payer], message, recent_blockhash);

        // Send the transaction and capture the result
        let tx = svm.send_transaction(transaction).unwrap();

        // Log transaction details
        println!("\n\nMake transaction successful");
        println!("CUs Consumed: {}", tx.compute_units_consumed);

        // --- extra verification ---
        let vault_acc = svm.get_account(&vault).unwrap();
        let vault_state = spl_token_2022::state::Account::unpack(&vault_acc.data).unwrap();
        println!("Vault owner: {} (escrow PDA? {})", vault_state.owner, vault_state.owner == escrow.0);
        println!("Vault balance: {}", vault_state.amount);
        assert_eq!(vault_state.amount, amount_to_give);

        let maker_acc = svm.get_account(&maker_ata_a).unwrap();
        let maker_state = spl_token_2022::state::Account::unpack(&maker_acc.data).unwrap();
        println!("Maker ATA balance: {}", maker_state.amount);
        assert_eq!(maker_state.amount, 1000000000 - amount_to_give);

        let esc = svm.get_account(&escrow.0).unwrap();
        println!("Escrow account owner: {} (program? {})", esc.owner, esc.owner == program_id);
        println!("Escrow data len: {}", esc.data.len());
        let d = &esc.data;
        println!("  maker   = {}", Pubkey::new_from_array(d[0..32].try_into().unwrap()));
        println!("  mint_a  = {}", Pubkey::new_from_array(d[32..64].try_into().unwrap()));
        println!("  mint_b  = {}", Pubkey::new_from_array(d[64..96].try_into().unwrap()));
        println!("  receive = {}", u64::from_le_bytes(d[96..104].try_into().unwrap()));
        println!("  give    = {}", u64::from_le_bytes(d[104..112].try_into().unwrap()));
        println!("  bump    = {}", d[112]);
        assert_eq!(&d[0..32], payer.pubkey().as_ref());
        assert_eq!(u64::from_le_bytes(d[96..104].try_into().unwrap()), amount_to_receive);
        assert_eq!(u64::from_le_bytes(d[104..112].try_into().unwrap()), amount_to_give);
        assert_eq!(d[112], bump);
    }

    // ---------- Take ----------

    #[test]
    pub fn test_take_instruction() {
        let Deal { mut svm, maker, mint_a, mint_b, escrow, vault, .. } = make_deal();

        let taker = Keypair::new();
        svm.airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut svm, &taker, &mint_b)
            .owner(&taker.pubkey()).send().unwrap();
        MintTo::new(&mut svm, &maker, &mint_b, &taker_ata_b, AMOUNT_TO_RECEIVE).send().unwrap();

        // Derived, not created: the program makes them with CreateIdempotent.
        let taker_ata_a = spl_associated_token_account::get_associated_token_address(&taker.pubkey(), &mint_a);
        let maker_ata_b = spl_associated_token_account::get_associated_token_address(&maker.pubkey(), &mint_b);

        let maker_sol_before = svm.get_account(&maker.pubkey()).unwrap().lamports;

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
                AccountMeta::new_readonly(ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap(), false),
            ],
            data: vec![1u8],
        };

        let tx = send(&mut svm, &taker, ix, &[&taker]).expect("take should succeed");
        println!("Take CUs Consumed: {}", tx.compute_units_consumed);

        // The trade happened, both ways.
        assert_eq!(token_amount(&svm, &taker_ata_a), AMOUNT_TO_GIVE, "taker should hold the A");
        assert_eq!(token_amount(&svm, &maker_ata_b), AMOUNT_TO_RECEIVE, "maker should hold the B");

        // And nothing was left behind holding rent.
        assert!(is_closed(&svm, &vault), "vault should be closed");
        assert!(is_closed(&svm, &escrow), "escrow should be closed");

        let maker_sol_after = svm.get_account(&maker.pubkey()).unwrap().lamports;
        assert!(maker_sol_after > maker_sol_before, "maker should have been refunded the rent");
    }

    // ---------- Cancel ----------

    #[test]
    pub fn test_cancel_instruction() {
        let Deal { mut svm, maker, mint_a, maker_ata_a, escrow, vault, .. } = make_deal();

        assert_eq!(token_amount(&svm, &maker_ata_a), MINTED_A - AMOUNT_TO_GIVE);

        let ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(maker.pubkey(), true),
                AccountMeta::new_readonly(mint_a, false),
                AccountMeta::new(escrow, false),
                AccountMeta::new(vault, false),
                AccountMeta::new(maker_ata_a, false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
            ],
            data: vec![2u8],
        };

        let tx = send(&mut svm, &maker, ix, &[&maker]).expect("cancel should succeed");
        println!("Cancel CUs Consumed: {}", tx.compute_units_consumed);

        assert_eq!(token_amount(&svm, &maker_ata_a), MINTED_A, "every token should be back");
        assert!(is_closed(&svm, &vault), "vault should be closed");
        assert!(is_closed(&svm, &escrow), "escrow should be closed");
    }

    // ---------- Negative tests ----------

    /// A taker who cannot pay must not walk away with the vault.
    #[test]
    pub fn test_take_with_insufficient_funds_fails() {
        let Deal { mut svm, maker, mint_a, mint_b, escrow, vault, .. } = make_deal();

        let taker = Keypair::new();
        svm.airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut svm, &taker, &mint_b)
            .owner(&taker.pubkey()).send().unwrap();
        // Half of what the escrow asks for.
        MintTo::new(&mut svm, &maker, &mint_b, &taker_ata_b, AMOUNT_TO_RECEIVE / 2).send().unwrap();

        let taker_ata_a = spl_associated_token_account::get_associated_token_address(&taker.pubkey(), &mint_a);
        let maker_ata_b = spl_associated_token_account::get_associated_token_address(&maker.pubkey(), &mint_b);

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
                AccountMeta::new_readonly(ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap(), false),
            ],
            data: vec![1u8],
        };

        assert!(send(&mut svm, &taker, ix, &[&taker]).is_err(), "an underfunded take must fail");

        // Atomic: the transfer that did succeed was rolled back with it.
        assert_eq!(token_amount(&svm, &vault), AMOUNT_TO_GIVE, "the A never left the vault");
    }

    /// The test that matters most: a stranger must not be able to cancel
    /// someone else's escrow.
    #[test]
    pub fn test_cancel_by_stranger_fails() {
        let Deal { mut svm, maker, mint_a, maker_ata_a, escrow, vault, .. } = make_deal();

        let stranger = Keypair::new();
        svm.airdrop(&stranger.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

        // The stranger signs, but the escrow's stored maker is someone else.
        let ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(stranger.pubkey(), true),
                AccountMeta::new_readonly(mint_a, false),
                AccountMeta::new(escrow, false),
                AccountMeta::new(vault, false),
                AccountMeta::new(maker_ata_a, false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
            ],
            data: vec![2u8],
        };

        assert!(
            send(&mut svm, &stranger, ix, &[&stranger]).is_err(),
            "a stranger must not be able to cancel someone else's escrow",
        );

        assert_eq!(token_amount(&svm, &vault), AMOUNT_TO_GIVE, "the A is still in the vault");
        assert_eq!(token_amount(&svm, &maker_ata_a), MINTED_A - AMOUNT_TO_GIVE, "and the maker got nothing back");
        let _ = maker;
    }
}