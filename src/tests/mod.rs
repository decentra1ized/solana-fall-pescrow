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

    struct MakeFixture {
        svm: LiteSVM,
        maker: Keypair,
        mint_a: Pubkey,
        mint_b: Pubkey,
        maker_ata_a: Pubkey,
        escrow: Pubkey,
        bump: u8,
        vault: Pubkey,
    }

    fn setup_make() -> MakeFixture {
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

        let maker_ata_a =
            CreateAssociatedTokenAccount::new(&mut svm, &maker, &mint_a)
                .owner(&maker.pubkey())
                .send()
                .unwrap();

        let (escrow, bump) = Pubkey::find_program_address(
            &[b"escrow".as_ref(), maker.pubkey().as_ref()],
            &program_id,
        );

        let vault =
            spl_associated_token_account::get_associated_token_address(
                &escrow,
                &mint_a,
            );

        let associated_token_program =
            ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();
        let token_program = TOKEN_PROGRAM_ID;
        let system_program = solana_sdk_ids::system_program::ID;

        MintTo::new(
            &mut svm,
            &maker,
            &mint_a,
            &maker_ata_a,
            1_000_000_000,
        )
        .send()
        .unwrap();

        let amount_to_receive: u64 = 100_000_000;
        let amount_to_give: u64 = 500_000_000;

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
                AccountMeta::new_readonly(associated_token_program, false),
            ],
            data: make_data,
        };

        let message = Message::new(&[make_ix], Some(&maker.pubkey()));
        let recent_blockhash = svm.latest_blockhash();
        let transaction =
            Transaction::new(&[&maker], message, recent_blockhash);

        let tx = svm.send_transaction(transaction).unwrap();

        println!("\nMake transaction successful");
        println!("CUs Consumed: {}", tx.compute_units_consumed);

        MakeFixture {
            svm,
            maker,
            mint_a,
            mint_b,
            maker_ata_a,
            escrow,
            bump,
            vault,
        }
    }

    #[test]
    pub fn test_make_instruction() {
        let fixture = setup_make();

        let vault_acc = fixture.svm.get_account(&fixture.vault).unwrap();
        let vault_state =
            spl_token_2022::state::Account::unpack(&vault_acc.data).unwrap();

        println!("Vault balance: {}", vault_state.amount);
        assert_eq!(vault_state.amount, 500_000_000);

        let maker_acc = fixture
            .svm
            .get_account(&fixture.maker_ata_a)
            .unwrap();
        let maker_state =
            spl_token_2022::state::Account::unpack(&maker_acc.data).unwrap();

        println!("Maker ATA balance: {}", maker_state.amount);
        assert_eq!(maker_state.amount, 500_000_000);

        let escrow_acc = fixture.svm.get_account(&fixture.escrow).unwrap();

        println!("Escrow data len: {}", escrow_acc.data.len());

        let data = &escrow_acc.data;

        assert_eq!(&data[0..32], fixture.maker.pubkey().as_ref());
        assert_eq!(&data[32..64], fixture.mint_a.as_ref());
        assert_eq!(&data[64..96], fixture.mint_b.as_ref());
        assert_eq!(
            u64::from_le_bytes(data[96..104].try_into().unwrap()),
            100_000_000
        );
        assert_eq!(
            u64::from_le_bytes(data[104..112].try_into().unwrap()),
            500_000_000
        );
        assert_eq!(data[112], fixture.bump);
    }

    #[test]
    fn test_take_instruction() {
        let mut fixture = setup_make();

        // Bob / taker
        let taker = Keypair::new();

        fixture
            .svm
            .airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL)
            .expect("Taker airdrop failed");

        // Bob needs an ATA for token B, because B is what he pays Alice.
        let taker_ata_b =
            CreateAssociatedTokenAccount::new(&mut fixture.svm, &taker, &fixture.mint_b)
                .owner(&taker.pubkey())
                .send()
                .unwrap();

        // Give Bob exactly 100 B.
        MintTo::new(
            &mut fixture.svm,
            &fixture.maker,
            &fixture.mint_b,
            &taker_ata_b,
            100_000_000,
        )
        .send()
        .unwrap();

        // These two destination ATAs do NOT exist yet.
        // Take itself should create them with CreateIdempotent.
        let taker_ata_a =
            spl_associated_token_account::get_associated_token_address(
                &taker.pubkey(),
                &fixture.mint_a,
            );

        let maker_ata_b =
            spl_associated_token_account::get_associated_token_address(
                &fixture.maker.pubkey(),
                &fixture.mint_b,
            );

        println!("Taker: {}", taker.pubkey());
        println!("Taker ATA B: {}", taker_ata_b);
        println!("Taker ATA A: {}", taker_ata_a);
        println!("Maker ATA B: {}", maker_ata_b);

        let associated_token_program =
            ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();

        let system_program = solana_sdk_ids::system_program::ID;
        let token_program = TOKEN_PROGRAM_ID;

        // Take has no payload beyond its 1-byte discriminator.
        let take_data = vec![1u8];

        let take_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(taker.pubkey(), true),                  // 0 taker
                AccountMeta::new(fixture.maker.pubkey(), false),         // 1 maker
                AccountMeta::new_readonly(fixture.mint_a, false),        // 2 mint_a
                AccountMeta::new_readonly(fixture.mint_b, false),        // 3 mint_b
                AccountMeta::new(fixture.escrow, false),                 // 4 escrow_account
                AccountMeta::new(fixture.vault, false),                  // 5 vault
                AccountMeta::new(taker_ata_a, false),                    // 6 taker_ata_a
                AccountMeta::new(taker_ata_b, false),                    // 7 taker_ata_b
                AccountMeta::new(maker_ata_b, false),                    // 8 maker_ata_b
                AccountMeta::new_readonly(system_program, false),        // 9 system_program
                AccountMeta::new_readonly(token_program, false),         // 10 token_program
                AccountMeta::new_readonly(associated_token_program, false), // 11 associated_token_program
            ],
            data: take_data,
        };

        let message = Message::new(&[take_ix], Some(&taker.pubkey()));
        let recent_blockhash = fixture.svm.latest_blockhash();

        let transaction = Transaction::new(
            &[&taker],
            message,
            recent_blockhash,
        );

        let tx = fixture
            .svm
            .send_transaction(transaction)
            .expect("Take transaction failed");

        println!("\nTake transaction successful");
        println!("CUs Consumed: {}", tx.compute_units_consumed);

        // Prove the taker received all 500 A.
        let taker_ata_a_acc = fixture
            .svm
            .get_account(&taker_ata_a)
            .expect("taker ATA A should exist");

        let taker_ata_a_state =
            spl_token_2022::state::Account::unpack(&taker_ata_a_acc.data).unwrap();

        assert_eq!(
            taker_ata_a_state.amount,
            500_000_000,
            "taker should receive 500 A"
        );

        // Prove the maker received the 100 B asking price.
        let maker_ata_b_acc = fixture
            .svm
            .get_account(&maker_ata_b)
            .expect("maker ATA B should exist");

        let maker_ata_b_state =
            spl_token_2022::state::Account::unpack(&maker_ata_b_acc.data).unwrap();

        assert_eq!(
            maker_ata_b_state.amount,
            100_000_000,
            "maker should receive 100 B"
        );

        // The vault must be closed.
        match fixture.svm.get_account(&fixture.vault) {
            None => {}
            Some(account) => {
                assert_eq!(account.lamports, 0, "closed vault should have 0 lamports");
                assert_eq!(
                    account.owner,
                    solana_sdk_ids::system_program::ID,
                    "closed vault should no longer be token-program owned"
                );
            }
        }

        // The escrow state account must also be closed.
        match fixture.svm.get_account(&fixture.escrow) {
            None => {}
            Some(account) => {
                assert_eq!(
                    account.lamports, 0,
                    "closed escrow should have 0 lamports"
                );
                assert_eq!(
                    account.owner,
                    solana_sdk_ids::system_program::ID,
                    "closed escrow should no longer be program-owned"
                );
            }
        }
    }

    #[test]
    fn test_take_insufficient_funds() {
        let mut fixture = setup_make();

        // Bob / taker
        let taker = Keypair::new();

        fixture
            .svm
            .airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL)
            .expect("Taker airdrop failed");

        // Bob needs an ATA for token B, because B is what he pays Alice.
        let taker_ata_b =
            CreateAssociatedTokenAccount::new(&mut fixture.svm, &taker, &fixture.mint_b)
                .owner(&taker.pubkey())
                .send()
                .unwrap();

        // Give Bob only 50 B, while the escrow requires 100 B.
        MintTo::new(
            &mut fixture.svm,
            &fixture.maker,
            &fixture.mint_b,
            &taker_ata_b,
            50_000_000,
        )
        .send()
        .unwrap();

        // These destination ATAs do not exist yet.
        let taker_ata_a =
            spl_associated_token_account::get_associated_token_address(
                &taker.pubkey(),
                &fixture.mint_a,
            );

        let maker_ata_b =
            spl_associated_token_account::get_associated_token_address(
                &fixture.maker.pubkey(),
                &fixture.mint_b,
            );

        let associated_token_program =
            ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();

        let system_program = solana_sdk_ids::system_program::ID;
        let token_program = TOKEN_PROGRAM_ID;

        let take_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(taker.pubkey(), true),                          // 0 taker
                AccountMeta::new(fixture.maker.pubkey(), false),                // 1 maker
                AccountMeta::new_readonly(fixture.mint_a, false),               // 2 mint_a
                AccountMeta::new_readonly(fixture.mint_b, false),               // 3 mint_b
                AccountMeta::new(fixture.escrow, false),                        // 4 escrow_account
                AccountMeta::new(fixture.vault, false),                         // 5 vault
                AccountMeta::new(taker_ata_a, false),                           // 6 taker_ata_a
                AccountMeta::new(taker_ata_b, false),                           // 7 taker_ata_b
                AccountMeta::new(maker_ata_b, false),                           // 8 maker_ata_b
                AccountMeta::new_readonly(system_program, false),               // 9 system_program
                AccountMeta::new_readonly(token_program, false),                // 10 token_program
                AccountMeta::new_readonly(associated_token_program, false),     // 11 associated_token_program
            ],
            data: vec![1u8],
        };

        let message = Message::new(&[take_ix], Some(&taker.pubkey()));
        let recent_blockhash = fixture.svm.latest_blockhash();

        let transaction = Transaction::new(
            &[&taker],
            message,
            recent_blockhash,
        );

        // This MUST fail: Bob has only 50 B, but needs to pay 100 B.
        let result = fixture.svm.send_transaction(transaction);

        assert!(
            result.is_err(),
            "Take must fail when the taker cannot afford the asking price"
        );

        // Prove the failed transaction was atomic:
        // all 500 A must still be sitting in the vault.
        let vault_acc = fixture
            .svm
            .get_account(&fixture.vault)
            .expect("vault must still exist after failed Take");

        let vault_state =
            spl_token_2022::state::Account::unpack(&vault_acc.data).unwrap();

        assert_eq!(
            vault_state.amount,
            500_000_000,
            "failed Take must leave all 500 A in the vault"
        );

        // Bob must still have his original 50 B.
        let taker_b_acc = fixture
            .svm
            .get_account(&taker_ata_b)
            .expect("taker ATA B should still exist");

        let taker_b_state =
            spl_token_2022::state::Account::unpack(&taker_b_acc.data).unwrap();

        assert_eq!(
            taker_b_state.amount,
            50_000_000,
            "failed Take must not move Bob's 50 B"
        );

        // The escrow must also still exist.
        assert!(
            fixture.svm.get_account(&fixture.escrow).is_some(),
            "escrow must remain open after failed Take"
        );

        println!("\nInsufficient-funds Take correctly failed");
    }

    #[test]
    fn test_stranger_cannot_cancel() {
        let mut fixture = setup_make();

        // Someone who is NOT the maker.
        let stranger = Keypair::new();

        fixture
            .svm
            .airdrop(&stranger.pubkey(), 10 * LAMPORTS_PER_SOL)
            .expect("Stranger airdrop failed");

        let token_program = TOKEN_PROGRAM_ID;

        // Cancel discriminator = 2.
        let cancel_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                // Pretend the stranger is the maker.
                // They really do sign, so is_signer() alone is NOT enough protection.
                AccountMeta::new(stranger.pubkey(), true),             // 0 maker
                AccountMeta::new_readonly(fixture.mint_a, false),      // 1 mint_a
                AccountMeta::new(fixture.escrow, false),               // 2 escrow_account
                AccountMeta::new(fixture.vault, false),                // 3 vault
                AccountMeta::new(fixture.maker_ata_a, false),          // 4 maker_ata_a
                AccountMeta::new_readonly(token_program, false),       // 5 token_program
            ],
            data: vec![2u8],
        };

        let message = Message::new(
            &[cancel_ix],
            Some(&stranger.pubkey()),
        );

        let recent_blockhash = fixture.svm.latest_blockhash();

        let transaction = Transaction::new(
            &[&stranger],
            message,
            recent_blockhash,
        );

        let result = fixture.svm.send_transaction(transaction);

        assert!(
            result.is_err(),
            "a stranger must not be able to cancel another maker's escrow"
        );

        // The failed Cancel must leave all 500 A in the vault.
        let vault_acc = fixture
            .svm
            .get_account(&fixture.vault)
            .expect("vault must still exist after stranger's failed Cancel");

        let vault_state =
            spl_token_2022::state::Account::unpack(&vault_acc.data).unwrap();

        assert_eq!(
            vault_state.amount,
            500_000_000,
            "stranger's failed Cancel must leave all 500 A in the vault"
        );

        // The escrow must still exist too.
        assert!(
            fixture.svm.get_account(&fixture.escrow).is_some(),
            "escrow must remain open after stranger's failed Cancel"
        );

        println!("\nStranger Cancel correctly failed");
    }

    #[test]
    fn test_cancel_instruction() {
        let mut fixture = setup_make();

        let program_id = program_id();
        let token_program = TOKEN_PROGRAM_ID;

        let cancel_ix = Instruction {
            program_id,
            accounts: vec![
                // Must match cancel.rs exactly:
                // 0 maker
                // 1 mint_a
                // 2 escrow_account
                // 3 vault
                // 4 maker_ata_a
                // 5 token_program
                AccountMeta::new(fixture.maker.pubkey(), true),
                AccountMeta::new_readonly(fixture.mint_a, false),
                AccountMeta::new(fixture.escrow, false),
                AccountMeta::new(fixture.vault, false),
                AccountMeta::new(fixture.maker_ata_a, false),
                AccountMeta::new_readonly(token_program, false),
            ],
            data: vec![2u8], // Cancel discriminator
        };

        let message = Message::new(
            &[cancel_ix],
            Some(&fixture.maker.pubkey()),
        );

        let recent_blockhash = fixture.svm.latest_blockhash();

        let transaction = Transaction::new(
            &[&fixture.maker],
            message,
            recent_blockhash,
        );

        let tx = fixture.svm
            .send_transaction(transaction)
            .expect("Cancel transaction failed");

        println!("\nCancel transaction successful");
        println!("CUs Consumed: {}", tx.compute_units_consumed);

        // Maker started with 1,000 A and deposited 500 A into the vault.
        // Cancel must return all 500 A.
        let maker_ata_acc = fixture
            .svm
            .get_account(&fixture.maker_ata_a)
            .expect("Maker ATA A should still exist");

        let maker_ata_state =
            spl_token_2022::state::Account::unpack(&maker_ata_acc.data).unwrap();

        assert_eq!(
            maker_ata_state.amount,
            1_000_000_000,
            "maker should have all 1,000 A back after Cancel"
        );

        // Vault must be closed.
        match fixture.svm.get_account(&fixture.vault) {
            None => {}
            Some(account) => {
                assert_eq!(
                    account.lamports, 0,
                    "closed vault should have zero lamports"
                );
            }
        }

        // Escrow state account must also be closed.
        match fixture.svm.get_account(&fixture.escrow) {
            None => {}
            Some(account) => {
                assert_eq!(
                    account.lamports, 0,
                    "closed escrow should have zero lamports"
                );
            }
        }
    }

}