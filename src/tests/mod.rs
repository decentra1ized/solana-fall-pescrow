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
    /// Proves that the original maker can cancel an open escrow.
    ///
    /// Expected result:
    ///
    /// 1. Make deposits 500 Token A into the vault.
    /// 2. Cancel returns all 500 Token A to the maker.
    /// 3. The vault closes.
    /// 4. The escrow state account closes.
    #[test]
    pub fn test_cancel_instruction() {
        let (mut svm, payer) = setup();
        let program_id = program_id();

        // Create the two token mints used by the escrow agreement.
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

        // Create the maker's Token A account.
        let maker_ata_a =
            CreateAssociatedTokenAccount::new(&mut svm, &payer, &mint_a)
                .owner(&payer.pubkey())
                .send()
                .unwrap();

        // Give the maker 1,000 Token A.
        MintTo::new(
            &mut svm,
            &payer,
            &mint_a,
            &maker_ata_a,
            1_000_000_000,
        )
        .send()
        .unwrap();

        // Derive the escrow PDA from ["escrow", maker].
        let (escrow, _bump) = Pubkey::find_program_address(
            &[b"escrow", payer.pubkey().as_ref()],
            &program_id,
        );

        // The vault is the escrow PDA's associated Token A account.
        let vault =
            spl_associated_token_account::get_associated_token_address(
                &escrow,
                &mint_a,
            );

        let system_program = solana_sdk_ids::system_program::ID;
        let token_program = TOKEN_PROGRAM_ID;
        let associated_token_program =
            ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();

        let amount_to_receive: u64 = 100_000_000;
        let amount_to_give: u64= 500_000_000;

        // Build discriminator 0: Make.
        let make_data = [
            vec![0u8],
            amount_to_receive.to_le_bytes().to_vec(),
            amount_to_give.to_le_bytes().to_vec(),
        ]
        .concat();

        let make_ix = Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new(payer.pubkey(), true),
                AccountMeta::new_readonly(mint_a, false),
                AccountMeta::new_readonly(mint_b, false),
                AccountMeta::new(escrow, false),
                AccountMeta::new(maker_ata_a, false),
                AccountMeta::new(vault, false),
                AccountMeta::new_readonly(system_program, false),
                AccountMeta::new_readonly(token_program, false),
                AccountMeta::new_readonly(
                    associated_token_program,
                    false,
                ),
            ],
            data: make_data,
        };

        // Send Make so the vault begins with 500 Token A.
        let make_message =
            Message::new(&[make_ix], Some(&payer.pubkey()));

        let make_transaction = Transaction::new(
            &[&payer],
            make_message,
            svm.latest_blockhash(),
        );

        svm.send_transaction(make_transaction)
            .expect("Make should succeed before Cancel");

        // Confirm the test's starting condition.
        let vault_before = svm
            .get_account(&vault)
            .expect("Vault should exist after Make");

        let vault_before_state =
            spl_token_2022::state::Account::unpack(
                &vault_before.data,
            )
            .unwrap();

        assert_eq!(
            vault_before_state.amount,
            amount_to_give,
            "Vault should contain the maker's deposit"
        );

        // Build discriminator 2: Cancel.
        //
        // Account order must match process_cancel_instruction:
        // maker, mint_a, escrow, vault, maker_ata_a, token_program.
        let cancel_ix = Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new(payer.pubkey(), true),
                AccountMeta::new_readonly(mint_a, false),
                AccountMeta::new(escrow, false),
                AccountMeta::new(vault, false),
                AccountMeta::new(maker_ata_a, false),
                AccountMeta::new_readonly(token_program, false),
            ],
            data: vec![2u8],
        };

        let cancel_message =
            Message::new(&[cancel_ix], Some(&payer.pubkey()));

        let cancel_transaction = Transaction::new(
            &[&payer],
            cancel_message,
            svm.latest_blockhash(),
        );

        let cancel_result = svm
            .send_transaction(cancel_transaction)
            .expect("The original maker should be able to cancel");

        println!("\nCancel transaction successful");
        println!(
            "CUs Consumed: {}",
            cancel_result.compute_units_consumed
        );

        // The maker started with 1,000 Token A, deposited 500, and
        // received those 500 back. The final balance must be 1,000.
        let maker_account = svm
            .get_account(&maker_ata_a)
            .expect("Maker Token A account should still exist");

        let maker_state =
            spl_token_2022::state::Account::unpack(
                &maker_account.data,
            )
            .unwrap();

        assert_eq!(
            maker_state.amount,
            1_000_000_000,
            "Cancel should return every Token A to the maker"
        );

        // Closing an account returns its rent and removes the account.
        assert!(
            svm.get_account(&vault).is_none(),
            "The empty token vault should be closed"
        );

        assert!(
            svm.get_account(&escrow).is_none(),
            "The escrow state account should be closed"
        );
    }
        /// Proves that a taker can accept an escrow and complete the swap.
    ///
    /// Expected result:
    ///
    /// 1. The maker deposits 500 Token A into the vault.
    /// 2. The taker pays 100 Token B to the maker.
    /// 3. The taker receives all 500 Token A from the vault.
    /// 4. The vault and escrow accounts close.
    #[test]
    pub fn test_take_instruction() {
        let (mut svm, maker) = setup();
        let program_id = program_id();

        // Create a second wallet that will accept the escrow.
        let taker = Keypair::new();

        // The taker needs SOL to pay transaction fees and to create
        // the two missing associated token accounts.
        svm.airdrop(&taker.pubkey(), 2 * LAMPORTS_PER_SOL)
            .expect("Taker airdrop should succeed");

        // Create Token A, which the maker is offering.
        let mint_a = CreateMint::new(&mut svm, &maker)
            .decimals(6)
            .authority(&maker.pubkey())
            .send()
            .unwrap();

        // Create Token B, which the maker wants to receive.
        let mint_b = CreateMint::new(&mut svm, &maker)
            .decimals(6)
            .authority(&maker.pubkey())
            .send()
            .unwrap();

        // Create the maker's Token A account.
        let maker_ata_a =
            CreateAssociatedTokenAccount::new(
                &mut svm,
                &maker,
                &mint_a,
            )
            .owner(&maker.pubkey())
            .send()
            .unwrap();

        // Give the maker 1,000 Token A.
        MintTo::new(
            &mut svm,
            &maker,
            &mint_a,
            &maker_ata_a,
            1_000_000_000,
        )
        .send()
        .unwrap();

        // Create the taker's Token B account.
        let taker_ata_b =
            CreateAssociatedTokenAccount::new(
                &mut svm,
                &maker,
                &mint_b,
            )
            .owner(&taker.pubkey())
            .send()
            .unwrap();

        // Give the taker 200 Token B.
        //
        // The escrow requires only 100, so the taker should have
        // 100 Token B remaining after the successful trade.
        MintTo::new(
            &mut svm,
            &maker,
            &mint_b,
            &taker_ata_b,
            200_000_000,
        )
        .send()
        .unwrap();

        // Derive the escrow state PDA.
        let (escrow, _bump) = Pubkey::find_program_address(
            &[b"escrow", maker.pubkey().as_ref()],
            &program_id,
        );

        // Derive the vault that will hold the maker's Token A.
        let vault =
            spl_associated_token_account::get_associated_token_address(
                &escrow,
                &mint_a,
            );

        // These two accounts do not exist yet.
        //
        // The Take instruction should create them with
        // CreateIdempotent:
        //
        // - taker_ata_a receives Token A
        // - maker_ata_b receives Token B
        let taker_ata_a =
            spl_associated_token_account::get_associated_token_address(
                &taker.pubkey(),
                &mint_a,
            );

        let maker_ata_b =
            spl_associated_token_account::get_associated_token_address(
                &maker.pubkey(),
                &mint_b,
            );

        let system_program = solana_sdk_ids::system_program::ID;
        let token_program = TOKEN_PROGRAM_ID;
        let associated_token_program =
            ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();

        let amount_to_receive: u64 = 100_000_000;
        let amount_to_give: u64 = 500_000_000;

        // Build and send discriminator 0: Make.
        let make_data = [
            vec![0u8],
            amount_to_receive.to_le_bytes().to_vec(),
            amount_to_give.to_le_bytes().to_vec(),
        ]
        .concat();

        let make_ix = Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new(maker.pubkey(), true),
                AccountMeta::new_readonly(mint_a, false),
                AccountMeta::new_readonly(mint_b, false),
                AccountMeta::new(escrow, false),
                AccountMeta::new(maker_ata_a, false),
                AccountMeta::new(vault, false),
                AccountMeta::new_readonly(system_program, false),
                AccountMeta::new_readonly(token_program, false),
                AccountMeta::new_readonly(
                    associated_token_program,
                    false,
                ),
            ],
            data: make_data,
        };

        let make_message =
            Message::new(&[make_ix], Some(&maker.pubkey()));

        let make_transaction = Transaction::new(
            &[&maker],
            make_message,
            svm.latest_blockhash(),
        );

        svm.send_transaction(make_transaction)
            .expect("Make should succeed before Take");

        // Confirm that Make deposited the offered Token A.
        let vault_before = svm
            .get_account(&vault)
            .expect("Vault should exist after Make");

        let vault_before_state =
            spl_token_2022::state::Account::unpack(
                &vault_before.data,
            )
            .unwrap();

        assert_eq!(
            vault_before_state.amount,
            amount_to_give,
            "Vault should hold 500 Token A before Take"
        );

        // Build discriminator 1: Take.
        //
        // Account order must match process_take_instruction:
        //
        // taker, maker, mint_a, mint_b, escrow, vault,
        // taker_ata_a, taker_ata_b, maker_ata_b,
        // system_program, token_program, associated_token_program.
        let take_ix = Instruction {
            program_id,
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
                AccountMeta::new_readonly(system_program, false),
                AccountMeta::new_readonly(token_program, false),
                AccountMeta::new_readonly(
                    associated_token_program,
                    false,
                ),
            ],
            data: vec![1u8],
        };

        let take_message =
            Message::new(&[take_ix], Some(&taker.pubkey()));

        let take_transaction = Transaction::new(
            &[&taker],
            take_message,
            svm.latest_blockhash(),
        );

        let take_result = svm
            .send_transaction(take_transaction)
            .expect("A properly funded taker should complete the trade");

        println!("\nTake transaction successful");
        println!(
            "CUs Consumed: {}",
            take_result.compute_units_consumed
        );

        // The taker receives all 500 Token A from the vault.
        let taker_a_account = svm
            .get_account(&taker_ata_a)
            .expect("Taker Token A account should exist");

        let taker_a_state =
            spl_token_2022::state::Account::unpack(
                &taker_a_account.data,
            )
            .unwrap();

        assert_eq!(
            taker_a_state.amount,
            amount_to_give,
            "Taker should receive 500 Token A"
        );

        // The maker receives the requested 100 Token B.
        let maker_b_account = svm
            .get_account(&maker_ata_b)
            .expect("Maker Token B account should exist");

        let maker_b_state =
            spl_token_2022::state::Account::unpack(
                &maker_b_account.data,
            )
            .unwrap();

        assert_eq!(
            maker_b_state.amount,
            amount_to_receive,
            "Maker should receive 100 Token B"
        );

        // The taker began with 200 Token B and paid 100.
        let taker_b_account = svm
            .get_account(&taker_ata_b)
            .expect("Taker Token B account should still exist");

        let taker_b_state =
            spl_token_2022::state::Account::unpack(
                &taker_b_account.data,
            )
            .unwrap();

        assert_eq!(
            taker_b_state.amount,
            100_000_000,
            "Taker should have 100 Token B remaining"
        );

        // A completed escrow must close both temporary accounts.
        assert!(
            svm.get_account(&vault).is_none(),
            "The empty vault should be closed"
        );

        assert!(
            svm.get_account(&escrow).is_none(),
            "The completed escrow account should be closed"
        );
    }
        /// Proves that a stranger cannot cancel another person's escrow.
    ///
    /// The transaction must fail, and the failure must not move any
    /// tokens or close either escrow account.
    #[test]
    pub fn test_stranger_cannot_cancel() {
        let (mut svm, maker) = setup();
        let program_id = program_id();

        // Create an unrelated wallet that will try to cancel the escrow.
        let stranger = Keypair::new();

        svm.airdrop(&stranger.pubkey(), LAMPORTS_PER_SOL)
            .expect("Stranger airdrop should succeed");

        // Create the two mints used by the maker's escrow.
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

        // Create and fund the maker's Token A account.
        let maker_ata_a =
            CreateAssociatedTokenAccount::new(
                &mut svm,
                &maker,
                &mint_a,
            )
            .owner(&maker.pubkey())
            .send()
            .unwrap();

        MintTo::new(
            &mut svm,
            &maker,
            &mint_a,
            &maker_ata_a,
            1_000_000_000,
        )
        .send()
        .unwrap();

        // Derive the maker's escrow PDA and Token A vault.
        let (escrow, _bump) = Pubkey::find_program_address(
            &[b"escrow", maker.pubkey().as_ref()],
            &program_id,
        );

        let vault =
            spl_associated_token_account::get_associated_token_address(
                &escrow,
                &mint_a,
            );

        let system_program = solana_sdk_ids::system_program::ID;
        let token_program = TOKEN_PROGRAM_ID;
        let associated_token_program =
            ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();

        let amount_to_receive: u64 = 100_000_000;
        let amount_to_give: u64 = 500_000_000;

        // The real maker creates and funds the escrow normally.
        let make_data = [
            vec![0u8],
            amount_to_receive.to_le_bytes().to_vec(),
            amount_to_give.to_le_bytes().to_vec(),
        ]
        .concat();

        let make_ix = Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new(maker.pubkey(), true),
                AccountMeta::new_readonly(mint_a, false),
                AccountMeta::new_readonly(mint_b, false),
                AccountMeta::new(escrow, false),
                AccountMeta::new(maker_ata_a, false),
                AccountMeta::new(vault, false),
                AccountMeta::new_readonly(system_program, false),
                AccountMeta::new_readonly(token_program, false),
                AccountMeta::new_readonly(
                    associated_token_program,
                    false,
                ),
            ],
            data: make_data,
        };

        let make_message =
            Message::new(&[make_ix], Some(&maker.pubkey()));

        let make_transaction = Transaction::new(
            &[&maker],
            make_message,
            svm.latest_blockhash(),
        );

        svm.send_transaction(make_transaction)
            .expect("The maker should create the escrow");

        // Confirm the escrow begins with 500 Token A.
        let vault_before = svm
            .get_account(&vault)
            .expect("Vault should exist before the attack");

        let vault_before_state =
            spl_token_2022::state::Account::unpack(
                &vault_before.data,
            )
            .unwrap();

        assert_eq!(
            vault_before_state.amount,
            amount_to_give,
            "Vault should contain 500 Token A"
        );

        // The stranger supplies their own wallet as the supposed maker
        // and signs discriminator 2: Cancel.
        //
        // The program must compare this wallet against the maker stored
        // inside the escrow and reject the transaction.
        let fake_cancel_ix = Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new(stranger.pubkey(), true),
                AccountMeta::new_readonly(mint_a, false),
                AccountMeta::new(escrow, false),
                AccountMeta::new(vault, false),
                AccountMeta::new(maker_ata_a, false),
                AccountMeta::new_readonly(token_program, false),
            ],
            data: vec![2u8],
        };

        let fake_cancel_message = Message::new(
            &[fake_cancel_ix],
            Some(&stranger.pubkey()),
        );

        let fake_cancel_transaction = Transaction::new(
            &[&stranger],
            fake_cancel_message,
            svm.latest_blockhash(),
        );

        let result = svm.send_transaction(fake_cancel_transaction);

        assert!(
            result.is_err(),
            "A stranger must not be allowed to cancel the escrow"
        );

        println!("\nStranger cancellation correctly rejected");

        // The vault must still exist and still hold all 500 Token A.
        let vault_after = svm
            .get_account(&vault)
            .expect("Failed cancellation must not close the vault");

        let vault_after_state =
            spl_token_2022::state::Account::unpack(
                &vault_after.data,
            )
            .unwrap();

        assert_eq!(
            vault_after_state.amount,
            amount_to_give,
            "Failed cancellation must not move the vault tokens"
        );

        // The maker deposited 500 of their original 1,000 Token A.
        // A failed attack must leave the maker's balance at 500.
        let maker_after = svm
            .get_account(&maker_ata_a)
            .expect("Maker Token A account should remain");

        let maker_after_state =
            spl_token_2022::state::Account::unpack(
                &maker_after.data,
            )
            .unwrap();

        assert_eq!(
            maker_after_state.amount,
            500_000_000,
            "Failed cancellation must not change the maker's balance"
        );

        // The escrow must remain open after the rejected transaction.
        assert!(
            svm.get_account(&escrow).is_some(),
            "Failed cancellation must not close the escrow"
        );
    }
        /// Proves that an underfunded taker cannot complete the escrow.
    ///
    /// The escrow asks for 100 Token B, but this taker owns only 50.
    /// The entire transaction must fail atomically:
    ///
    /// - no Token A leaves the vault;
    /// - no Token B leaves the taker;
    /// - no new token accounts remain;
    /// - the escrow stays open.
    #[test]
    pub fn test_underfunded_taker_fails() {
        let (mut svm, maker) = setup();
        let program_id = program_id();

        // Create the wallet that will attempt to take the escrow.
        let taker = Keypair::new();

        svm.airdrop(&taker.pubkey(), 2 * LAMPORTS_PER_SOL)
            .expect("Taker airdrop should succeed");

        // Create the two mints used in the proposed swap.
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

        // Create and fund the maker's Token A account.
        let maker_ata_a =
            CreateAssociatedTokenAccount::new(
                &mut svm,
                &maker,
                &mint_a,
            )
            .owner(&maker.pubkey())
            .send()
            .unwrap();

        MintTo::new(
            &mut svm,
            &maker,
            &mint_a,
            &maker_ata_a,
            1_000_000_000,
        )
        .send()
        .unwrap();

        // Create the taker's Token B account.
        let taker_ata_b =
            CreateAssociatedTokenAccount::new(
                &mut svm,
                &maker,
                &mint_b,
            )
            .owner(&taker.pubkey())
            .send()
            .unwrap();

        // Give the taker only 50 Token B.
        //
        // The escrow requires 100 Token B, so Take must fail.
        let taker_starting_balance: u64 = 50_000_000;

        MintTo::new(
            &mut svm,
            &maker,
            &mint_b,
            &taker_ata_b,
            taker_starting_balance,
        )
        .send()
        .unwrap();

        // Derive the escrow PDA and its Token A vault.
        let (escrow, _bump) = Pubkey::find_program_address(
            &[b"escrow", maker.pubkey().as_ref()],
            &program_id,
        );

        let vault =
            spl_associated_token_account::get_associated_token_address(
                &escrow,
                &mint_a,
            );

        // Take will attempt to create these accounts.
        //
        // Because the later token transfer fails, Solana should roll
        // back their creation with the rest of the transaction.
        let taker_ata_a =
            spl_associated_token_account::get_associated_token_address(
                &taker.pubkey(),
                &mint_a,
            );

        let maker_ata_b =
            spl_associated_token_account::get_associated_token_address(
                &maker.pubkey(),
                &mint_b,
            );

        let system_program = solana_sdk_ids::system_program::ID;
        let token_program = TOKEN_PROGRAM_ID;
        let associated_token_program =
            ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();

        let amount_to_receive: u64 = 100_000_000;
        let amount_to_give: u64 = 500_000_000;

        // The real maker creates the escrow and deposits 500 Token A.
        let make_data = [
            vec![0u8],
            amount_to_receive.to_le_bytes().to_vec(),
            amount_to_give.to_le_bytes().to_vec(),
        ]
        .concat();

        let make_ix = Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new(maker.pubkey(), true),
                AccountMeta::new_readonly(mint_a, false),
                AccountMeta::new_readonly(mint_b, false),
                AccountMeta::new(escrow, false),
                AccountMeta::new(maker_ata_a, false),
                AccountMeta::new(vault, false),
                AccountMeta::new_readonly(system_program, false),
                AccountMeta::new_readonly(token_program, false),
                AccountMeta::new_readonly(
                    associated_token_program,
                    false,
                ),
            ],
            data: make_data,
        };

        let make_message =
            Message::new(&[make_ix], Some(&maker.pubkey()));

        let make_transaction = Transaction::new(
            &[&maker],
            make_message,
            svm.latest_blockhash(),
        );

        svm.send_transaction(make_transaction)
            .expect("Make should succeed before the failed Take");

        // Confirm the vault begins with the promised 500 Token A.
        let vault_before = svm
            .get_account(&vault)
            .expect("Vault should exist after Make");

        let vault_before_state =
            spl_token_2022::state::Account::unpack(
                &vault_before.data,
            )
            .unwrap();

        assert_eq!(
            vault_before_state.amount,
            amount_to_give,
            "Vault should hold 500 Token A before Take"
        );

        // Build discriminator 1: Take.
        let take_ix = Instruction {
            program_id,
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
                AccountMeta::new_readonly(system_program, false),
                AccountMeta::new_readonly(token_program, false),
                AccountMeta::new_readonly(
                    associated_token_program,
                    false,
                ),
            ],
            data: vec![1u8],
        };

        let take_message =
            Message::new(&[take_ix], Some(&taker.pubkey()));

        let take_transaction = Transaction::new(
            &[&taker],
            take_message,
            svm.latest_blockhash(),
        );

        // The Token Program should reject the attempt because the
        // taker cannot pay the requested 100 Token B.
        let take_result = svm.send_transaction(take_transaction);

        assert!(
            take_result.is_err(),
            "A taker with only 50 Token B must not complete the trade"
        );

        println!("\nUnderfunded Take correctly rejected");

        // Solana transactions are atomic. Even though Take attempted
        // earlier operations, the failure must roll everything back.
        let vault_after = svm
            .get_account(&vault)
            .expect("Failed Take must not close the vault");

        let vault_after_state =
            spl_token_2022::state::Account::unpack(
                &vault_after.data,
            )
            .unwrap();

        assert_eq!(
            vault_after_state.amount,
            amount_to_give,
            "Failed Take must leave all 500 Token A in the vault"
        );

        // The taker must keep all 50 Token B.
        let taker_b_after = svm
            .get_account(&taker_ata_b)
            .expect("Taker Token B account should still exist");

        let taker_b_after_state =
            spl_token_2022::state::Account::unpack(
                &taker_b_after.data,
            )
            .unwrap();

        assert_eq!(
            taker_b_after_state.amount,
            taker_starting_balance,
            "Failed Take must not remove Token B from the taker"
        );

        // The two ATA creations happened inside the failed transaction.
        // Atomic rollback means neither account should remain.
        assert!(
            svm.get_account(&taker_ata_a).is_none(),
            "Failed Take must roll back the taker's Token A account"
        );

        assert!(
            svm.get_account(&maker_ata_b).is_none(),
            "Failed Take must roll back the maker's Token B account"
        );

        // The escrow must remain available for another properly funded
        // taker or for the maker to cancel later.
        assert!(
            svm.get_account(&escrow).is_some(),
            "Failed Take must leave the escrow open"
        );
    }
}