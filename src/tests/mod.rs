#[cfg(test)]
mod tests {

    use std::path::PathBuf;

    use litesvm::LiteSVM;
    use litesvm_token::{
        spl_token::{self},
        CreateAssociatedTokenAccount,
        CreateMint,
        MintTo,
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
    const ASSOCIATED_TOKEN_PROGRAM_ID: &str =
        "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL";

    fn program_id() -> Pubkey {
        Pubkey::from(crate::ID)
    }

    fn setup() -> (LiteSVM, Keypair) {
        let mut svm = LiteSVM::new();
        let payer = Keypair::new();

        // LiteSVM 0.9 still ships the pre-SIMD-0194 Rent sysvar.
        // Set the sysvar to match the live cluster / Pinocchio 0.11.
        #[allow(deprecated)]
        svm.set_sysvar(&solana_rent::Rent {
            lamports_per_byte_year: 6960,
            exemption_threshold: 1.0,
            burn_percent: 50,
        });

        svm.airdrop(&payer.pubkey(), 10 * LAMPORTS_PER_SOL)
            .expect("Airdrop failed");

        // Load program SO file.
        let so_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/deploy/escrow.so");

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

    // ------------------------------------------------------------
    // Reusable helper:
    // Creates Mint A, Mint B, maker ATA A, escrow PDA, vault,
    // mints 1,000 Token A to maker, and executes Make.
    // ------------------------------------------------------------
    fn make_escrow(
        svm: &mut LiteSVM,
        payer: &Keypair,
    ) -> (Pubkey, Pubkey, Pubkey, Pubkey, Pubkey, u8, u64, u64) {
        let program_id = program_id();

        // Create Mint A
        let mint_a = CreateMint::new(svm, payer)
            .decimals(6)
            .authority(&payer.pubkey())
            .send()
            .unwrap();

        // Create Mint B
        let mint_b = CreateMint::new(svm, payer)
            .decimals(6)
            .authority(&payer.pubkey())
            .send()
            .unwrap();

        // Create maker ATA for Mint A
        let maker_ata_a = CreateAssociatedTokenAccount::new(svm, payer, &mint_a)
            .owner(&payer.pubkey())
            .send()
            .unwrap();

        // Derive escrow PDA
        let escrow = Pubkey::find_program_address(
            &[b"escrow", payer.pubkey().as_ref()],
            &program_id,
        );

        // Derive vault ATA.
        // The escrow PDA owns the vault and Mint A is the vault's mint.
        let vault = spl_associated_token_account::get_associated_token_address(
            &escrow.0,
            &mint_a,
        );

        let associated_token_program =
            ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();

        let token_program = TOKEN_PROGRAM_ID;
        let system_program = solana_sdk_ids::system_program::ID;

        // Mint 1,000 Token A to maker.
        MintTo::new(
            svm,
            payer,
            &mint_a,
            &maker_ata_a,
            1_000_000_000,
        )
        .send()
        .unwrap();

        let amount_to_receive: u64 = 100_000_000;
        let amount_to_give: u64 = 500_000_000;

        // Build Make instruction.
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

        // Send Make transaction.
        let message = Message::new(&[make_ix], Some(&payer.pubkey()));
        let recent_blockhash = svm.latest_blockhash();

        let transaction =
            Transaction::new(&[payer], message, recent_blockhash);

        svm.send_transaction(transaction)
            .expect("Make transaction failed");

        (
            mint_a,
            mint_b,
            maker_ata_a,
            escrow.0,
            vault,
            escrow.1,
            amount_to_receive,
            amount_to_give,
        )
    }

    // ------------------------------------------------------------
    // Existing Make test
    // ------------------------------------------------------------
    #[test]
    pub fn test_make_instruction() {
        let (mut svm, payer) = setup();

        let program_id = program_id();

        assert_eq!(program_id.to_string(), PROGRAM_ID);

        let (
            mint_a,
            mint_b,
            maker_ata_a,
            escrow,
            vault,
            bump,
            amount_to_receive,
            amount_to_give,
        ) = make_escrow(&mut svm, &payer);

        println!("Mint A: {}", mint_a);
        println!("Mint B: {}", mint_b);
        println!("Maker ATA A: {}\n", maker_ata_a);
        println!("Escrow PDA: {}\n", escrow);
        println!("Vault PDA: {}\n", vault);
        println!("Bump: {}\n", bump);

        println!("Make transaction successful");

        // Verify vault.
        let vault_acc = svm.get_account(&vault).unwrap();

        let vault_state =
            spl_token_2022::state::Account::unpack(&vault_acc.data).unwrap();

        println!(
            "Vault owner: {} (escrow PDA? {})",
            vault_state.owner,
            vault_state.owner == escrow
        );

        println!("Vault balance: {}", vault_state.amount);

        assert_eq!(vault_state.amount, amount_to_give);
        assert_eq!(vault_state.owner, escrow);

        // Verify maker ATA.
        let maker_acc = svm.get_account(&maker_ata_a).unwrap();

        let maker_state =
            spl_token_2022::state::Account::unpack(&maker_acc.data).unwrap();

        println!("Maker ATA balance: {}", maker_state.amount);

        assert_eq!(
            maker_state.amount,
            1_000_000_000 - amount_to_give
        );

        // Verify escrow account.
        let esc = svm.get_account(&escrow).unwrap();

        println!(
            "Escrow account owner: {} (program? {})",
            esc.owner,
            esc.owner == program_id
        );

        println!("Escrow data len: {}", esc.data.len());

        assert_eq!(esc.owner, program_id);
        assert_eq!(esc.data.len(), 113);

        let d = &esc.data;

        println!(
            "  maker   = {}",
            Pubkey::new_from_array(
                d[0..32].try_into().unwrap()
            )
        );

        println!(
            "  mint_a  = {}",
            Pubkey::new_from_array(
                d[32..64].try_into().unwrap()
            )
        );

        println!(
            "  mint_b  = {}",
            Pubkey::new_from_array(
                d[64..96].try_into().unwrap()
            )
        );

        println!(
            "  receive = {}",
            u64::from_le_bytes(
                d[96..104].try_into().unwrap()
            )
        );

        println!(
            "  give    = {}",
            u64::from_le_bytes(
                d[104..112].try_into().unwrap()
            )
        );

        println!("  bump    = {}", d[112]);

        assert_eq!(
            &d[0..32],
            payer.pubkey().as_ref()
        );

        assert_eq!(
            &d[32..64],
            mint_a.as_ref()
        );

        assert_eq!(
            &d[64..96],
            mint_b.as_ref()
        );

        assert_eq!(
            u64::from_le_bytes(
                d[96..104].try_into().unwrap()
            ),
            amount_to_receive
        );

        assert_eq!(
            u64::from_le_bytes(
                d[104..112].try_into().unwrap()
            ),
            amount_to_give
        );

        assert_eq!(d[112], bump);
    }

    #[test]
    pub fn test_take_instruction() {
        let (mut svm, maker) = setup();
    
        let (
            mint_a,
            mint_b,
            maker_ata_a,
            escrow,
            vault,
            _bump,
            amount_to_receive,
            amount_to_give,
        ) = make_escrow(&mut svm, &maker);
    
        // Create the taker.
        let taker = Keypair::new();
    
        svm.airdrop(
            &taker.pubkey(),
            2 * LAMPORTS_PER_SOL,
        )
        .unwrap();
    
        // Create taker's Token B ATA.
        let taker_ata_b =
            CreateAssociatedTokenAccount::new(
                &mut svm,
                &maker,
                &mint_b,
            )
            .owner(&taker.pubkey())
            .send()
            .unwrap();
    
        // Give the taker 100 Token B.
        MintTo::new(
            &mut svm,
            &maker,
            &mint_b,
            &taker_ata_b,
            amount_to_receive,
        )
        .send()
        .unwrap();
    
        // We intentionally DO NOT create:
        // - taker_ata_a
        // - maker_ata_b
        //
        // Take should create those using CreateIdempotent.
    
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
    
        let associated_token_program =
            ASSOCIATED_TOKEN_PROGRAM_ID
                .parse::<Pubkey>()
                .unwrap();
    
        let token_program = TOKEN_PROGRAM_ID;
        let system_program =
            solana_sdk_ids::system_program::ID;
    
        // ---------------------------------------------------------
        // Take instruction
        // ---------------------------------------------------------
    
        let take_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                // 0 - taker
                AccountMeta::new(
                    taker.pubkey(),
                    true,
                ),
    
                // 1 - maker
                AccountMeta::new(
                    maker.pubkey(),
                    false,
                ),
    
                // 2 - mint A
                AccountMeta::new(
                    mint_a,
                    false,
                ),
    
                // 3 - mint B
                AccountMeta::new(
                    mint_b,
                    false,
                ),
    
                // 4 - escrow
                AccountMeta::new(
                    escrow,
                    false,
                ),
    
                // 5 - vault
                AccountMeta::new(
                    vault,
                    false,
                ),
    
                // 6 - taker ATA A
                AccountMeta::new(
                    taker_ata_a,
                    false,
                ),
    
                // 7 - taker ATA B
                AccountMeta::new(
                    taker_ata_b,
                    false,
                ),
    
                // 8 - maker ATA B
                AccountMeta::new(
                    maker_ata_b,
                    false,
                ),
    
                // 9 - system program
                AccountMeta::new(
                    system_program,
                    false,
                ),
    
                // 10 - token program
                AccountMeta::new(
                    token_program,
                    false,
                ),
    
                // 11 - associated token program
                AccountMeta::new(
                    associated_token_program,
                    false,
                ),
            ],
    
            // Take discriminator = 1
            data: vec![1u8],
        };
    
        let message =
            Message::new(
                &[take_ix],
                Some(&taker.pubkey()),
            );
    
        let recent_blockhash =
            svm.latest_blockhash();
    
        let transaction =
            Transaction::new(
                &[&taker],
                message,
                recent_blockhash,
            );
    
        svm.send_transaction(transaction)
            .expect("Take transaction failed");
    
        // ---------------------------------------------------------
        // Verify Take
        // ---------------------------------------------------------
    
        // Taker should receive all 500 Token A.
        let taker_a_account =
            svm.get_account(&taker_ata_a)
                .expect("Taker ATA A should exist");
    
        let taker_a_state =
            spl_token_2022::state::Account::unpack(
                &taker_a_account.data,
            )
            .unwrap();
    
        assert_eq!(
            taker_a_state.amount,
            amount_to_give
        );
    
        // Maker should receive 100 Token B.
        let maker_b_account =
            svm.get_account(&maker_ata_b)
                .expect("Maker ATA B should exist");
    
        let maker_b_state =
            spl_token_2022::state::Account::unpack(
                &maker_b_account.data,
            )
            .unwrap();
    
        assert_eq!(
            maker_b_state.amount,
            amount_to_receive
        );
    
        // Vault should have been closed.
        let vault_account =
            svm.get_account(&vault);
    
        assert!(
            vault_account.is_none()
                || vault_account.unwrap().lamports == 0
        );
    
        // Escrow should have been closed.
        let escrow_account =
            svm.get_account(&escrow);
    
        assert!(
            escrow_account.is_none()
                || escrow_account.unwrap().lamports == 0
        );
    }

    #[test]
    pub fn test_cancel_instruction() {
        let (mut svm, maker) = setup();
    
        let (
            mint_a,
            _mint_b,
            maker_ata_a,
            escrow,
            vault,
            _bump,
            _amount_to_receive,
            amount_to_give,
        ) = make_escrow(&mut svm, &maker);
    
        let token_program = TOKEN_PROGRAM_ID;
    
        // Cancel instruction
        //
        // Account order:
        // 0 maker
        // 1 mint_a
        // 2 escrow
        // 3 vault
        // 4 maker_ata_a
        // 5 token_program
    
        let cancel_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(maker.pubkey(), true),
                AccountMeta::new(mint_a, false),
                AccountMeta::new(escrow, false),
                AccountMeta::new(vault, false),
                AccountMeta::new(maker_ata_a, false),
                AccountMeta::new(token_program, false),
            ],
            data: vec![2u8],
        };
    
        let message =
            Message::new(&[cancel_ix], Some(&maker.pubkey()));
    
        let recent_blockhash = svm.latest_blockhash();
    
        let transaction =
            Transaction::new(
                &[&maker],
                message,
                recent_blockhash,
            );
    
        svm.send_transaction(transaction)
            .expect("Cancel transaction failed");
    
        // Maker originally had 1,000 Token A.
        // Make deposited 500.
        // Cancel should return those 500.
        let maker_account =
            svm.get_account(&maker_ata_a)
                .expect("Maker ATA A should exist");
    
        let maker_state =
            spl_token_2022::state::Account::unpack(
                &maker_account.data,
            )
            .unwrap();
    
        assert_eq!(
            maker_state.amount,
            1_000_000_000
        );
    
        assert_eq!(
            amount_to_give,
            500_000_000
        );
    
        // Vault should be closed.
        assert!(
            svm.get_account(&vault)
                .map(|account| account.lamports == 0)
                .unwrap_or(true)
        );
    
        // Escrow should be closed.
        assert!(
            svm.get_account(&escrow)
                .map(|account| account.lamports == 0)
                .unwrap_or(true)
        );
    }

    #[test]
    pub fn test_take_insufficient_token_b() {
        let (mut svm, maker) = setup();
    
        let (
            mint_a,
            mint_b,
            _maker_ata_a,
            escrow,
            vault,
            _bump,
            amount_to_receive,
            _amount_to_give,
        ) = make_escrow(&mut svm, &maker);
    
        let taker = Keypair::new();
    
        svm.airdrop(
            &taker.pubkey(),
            2 * LAMPORTS_PER_SOL,
        )
        .unwrap();
    
        // Create taker's Token B ATA.
        let taker_ata_b =
            CreateAssociatedTokenAccount::new(
                &mut svm,
                &maker,
                &mint_b,
            )
            .owner(&taker.pubkey())
            .send()
            .unwrap();
    
        // Give taker ONLY 50 Token B.
        MintTo::new(
            &mut svm,
            &maker,
            &mint_b,
            &taker_ata_b,
            50_000_000,
        )
        .send()
        .unwrap();
    
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
    
        let associated_token_program =
            ASSOCIATED_TOKEN_PROGRAM_ID
                .parse::<Pubkey>()
                .unwrap();
    
        let token_program = TOKEN_PROGRAM_ID;
        let system_program =
            solana_sdk_ids::system_program::ID;
    
        let take_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(taker.pubkey(), true),
                AccountMeta::new(maker.pubkey(), false),
                AccountMeta::new(mint_a, false),
                AccountMeta::new(mint_b, false),
                AccountMeta::new(escrow, false),
                AccountMeta::new(vault, false),
                AccountMeta::new(taker_ata_a, false),
                AccountMeta::new(taker_ata_b, false),
                AccountMeta::new(maker_ata_b, false),
                AccountMeta::new(system_program, false),
                AccountMeta::new(token_program, false),
                AccountMeta::new(associated_token_program, false),
            ],
            data: vec![1u8],
        };
    
        let message =
            Message::new(
                &[take_ix],
                Some(&taker.pubkey()),
            );
    
        let recent_blockhash =
            svm.latest_blockhash();
    
        let transaction =
            Transaction::new(
                &[&taker],
                message,
                recent_blockhash,
            );
    
        // Take MUST fail because taker has only 50,
        // while escrow requires 100.
        let result = svm.send_transaction(transaction);
    
        assert!(
            result.is_err(),
            "Take should fail with insufficient Token B"
        );
    
        // Vault should STILL contain the 500 Token A.
        let vault_account =
            svm.get_account(&vault)
                .expect("Vault should still exist");
    
        let vault_state =
            spl_token_2022::state::Account::unpack(
                &vault_account.data,
            )
            .unwrap();
    
        assert_eq!(
            vault_state.amount,
            500_000_000
        );
    
        // Taker should still have 50 Token B.
        let taker_b_account =
            svm.get_account(&taker_ata_b)
                .expect("Taker ATA B should exist");
    
        let taker_b_state =
            spl_token_2022::state::Account::unpack(
                &taker_b_account.data,
            )
            .unwrap();
    
        assert_eq!(
            taker_b_state.amount,
            50_000_000
        );
    
        // Escrow should still exist.
        assert!(
            svm.get_account(&escrow).is_some()
        );
    
        // Nothing should have been transferred to maker.
        assert!(
            svm.get_account(&maker_ata_b).is_none()
        );
    
        // `amount_to_receive` is 100 Token B.
        assert_eq!(
            amount_to_receive,
            100_000_000
        );
    }

    #[test]
    pub fn test_cancel_wrong_maker() {
        let (mut svm, maker) = setup();
    
        let (
            mint_a,
            _mint_b,
            _maker_ata_a,
            escrow,
            vault,
            _bump,
            _amount_to_receive,
            _amount_to_give,
        ) = make_escrow(&mut svm, &maker);
    
        // Create a completely different user.
        let stranger = Keypair::new();
    
        svm.airdrop(
            &stranger.pubkey(),
            2 * LAMPORTS_PER_SOL,
        )
        .unwrap();
    
        // The stranger does not own the maker's ATA,
        // but we still construct the Cancel instruction
        // using the real maker account.
        let cancel_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                // Wrong signer!
                AccountMeta::new(stranger.pubkey(), true),
    
                AccountMeta::new(mint_a, false),
                AccountMeta::new(escrow, false),
                AccountMeta::new(vault, false),
    
                // Real maker's ATA.
                // The program should reject before touching it.
                AccountMeta::new(
                    spl_associated_token_account::get_associated_token_address(
                        &maker.pubkey(),
                        &mint_a,
                    ),
                    false,
                ),
    
                AccountMeta::new(
                    TOKEN_PROGRAM_ID,
                    false,
                ),
            ],
            data: vec![2u8],
        };
    
        let message =
            Message::new(
                &[cancel_ix],
                Some(&stranger.pubkey()),
            );
    
        let recent_blockhash =
            svm.latest_blockhash();
    
        let transaction =
            Transaction::new(
                &[&stranger],
                message,
                recent_blockhash,
            );
    
        // Cancel MUST fail.
        let result =
            svm.send_transaction(transaction);
    
        assert!(
            result.is_err(),
            "Cancel by stranger should fail"
        );
    
        // Vault must still contain the 500 Token A.
        let vault_account =
            svm.get_account(&vault)
                .expect("Vault should still exist");
    
        let vault_state =
            spl_token_2022::state::Account::unpack(
                &vault_account.data,
            )
            .unwrap();
    
        assert_eq!(
            vault_state.amount,
            500_000_000
        );
    
        // Escrow must still exist.
        assert!(
            svm.get_account(&escrow).is_some()
        );
    }
}