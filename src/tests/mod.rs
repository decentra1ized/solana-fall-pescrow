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

    fn setup_make() -> (
        LiteSVM,
        Keypair,
        Pubkey,
        Pubkey,
        Pubkey,
        u8,
        Pubkey,
    ) {
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
    
        // Maker's Token A account
        let maker_ata_a =
            CreateAssociatedTokenAccount::new(&mut svm, &maker, &mint_a)
                .owner(&maker.pubkey())
                .send()
                .unwrap();
    
        // Escrow PDA
        let (escrow_pda, bump) = Pubkey::find_program_address(
            &[b"escrow".as_ref(), maker.pubkey().as_ref()],
            &program_id,
        );
    
        // Token A vault controlled by escrow PDA
        let vault =
            spl_associated_token_account::get_associated_token_address(
                &escrow_pda,
                &mint_a,
            );
    
        let associated_token_program =
            ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();
    
        let token_program = TOKEN_PROGRAM_ID;
        let system_program = solana_sdk_ids::system_program::ID;
    
        // Give maker 1000 Token A
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
                AccountMeta::new(mint_a, false),
                AccountMeta::new(mint_b, false),
                AccountMeta::new(escrow_pda, false),
                AccountMeta::new(maker_ata_a, false),
                AccountMeta::new(vault, false),
                AccountMeta::new(system_program, false),
                AccountMeta::new(token_program, false),
                AccountMeta::new(associated_token_program, false),
            ],
            data: make_data,
        };
    
        let message = Message::new(
            &[make_ix],
            Some(&maker.pubkey()),
        );
    
        let recent_blockhash = svm.latest_blockhash();
    
        let transaction =
            Transaction::new(&[&maker], message, recent_blockhash);
    
        let tx = svm.send_transaction(transaction).unwrap();
    
        println!("Make transaction successful");
        println!("CUs Consumed: {}", tx.compute_units_consumed);
    
        (
            svm,
            maker,
            mint_a,
            mint_b,
            escrow_pda,
            bump,
            vault,
        )
    }

   
    

    #[test]
pub fn test_make_instruction() {
    let (
        svm,
        maker,
        mint_a,
        mint_b,
        escrow_pda,
        bump,
        vault,
    ) = setup_make();

    let program_id = program_id();

    let amount_to_receive: u64 = 100_000_000;
    let amount_to_give: u64 = 500_000_000;

    // We don't need setup_make() to return this because
    // it's deterministic from maker + mint A.
    let maker_ata_a =
        spl_associated_token_account::get_associated_token_address(
            &maker.pubkey(),
            &mint_a,
        );

    // Verify vault
    let vault_acc = svm.get_account(&vault).unwrap();

    let vault_state =
        spl_token_2022::state::Account::unpack(&vault_acc.data).unwrap();

    println!(
        "Vault owner: {} (escrow PDA? {})",
        vault_state.owner,
        vault_state.owner == escrow_pda
    );

    println!("Vault balance: {}", vault_state.amount);

    assert_eq!(vault_state.amount, amount_to_give);

    // Verify maker's remaining Token A
    let maker_acc = svm.get_account(&maker_ata_a).unwrap();

    let maker_state =
        spl_token_2022::state::Account::unpack(&maker_acc.data).unwrap();

    println!("Maker ATA balance: {}", maker_state.amount);

    assert_eq!(
        maker_state.amount,
        1_000_000_000 - amount_to_give
    );

    // Verify escrow state
    let esc = svm.get_account(&escrow_pda).unwrap();

    println!(
        "Escrow account owner: {} (program? {})",
        esc.owner,
        esc.owner == program_id
    );

    println!("Escrow data len: {}", esc.data.len());

    let d = &esc.data;

    println!(
        "  maker   = {}",
        Pubkey::new_from_array(d[0..32].try_into().unwrap())
    );

    println!(
        "  mint_a  = {}",
        Pubkey::new_from_array(d[32..64].try_into().unwrap())
    );

    println!(
        "  mint_b  = {}",
        Pubkey::new_from_array(d[64..96].try_into().unwrap())
    );

    println!(
        "  receive = {}",
        u64::from_le_bytes(d[96..104].try_into().unwrap())
    );

    println!(
        "  give    = {}",
        u64::from_le_bytes(d[104..112].try_into().unwrap())
    );

    println!("  bump    = {}", d[112]);

    assert_eq!(&d[0..32], maker.pubkey().as_ref());

    assert_eq!(
        u64::from_le_bytes(d[96..104].try_into().unwrap()),
        amount_to_receive
    );

    assert_eq!(
        u64::from_le_bytes(d[104..112].try_into().unwrap()),
        amount_to_give
    );

    assert_eq!(d[112], bump);

    // Prevent unused-variable warning for now.
    assert_ne!(mint_b, Pubkey::default());
    }


    #[test]
pub fn test_take_instruction() {
    // 1. Run Make first.
    let (
        mut svm,
        maker,
        mint_a,
        mint_b,
        escrow_pda,
        _bump,
        vault,
    ) = setup_make();

    let program_id = program_id();

    // 2. Create Bob / taker and give him SOL.
    let taker = Keypair::new();

    svm.airdrop(
        &taker.pubkey(),
        10_000_000_000,
    )
    .unwrap();

    // Create Bob's Token B account.
    let taker_ata_b =
        CreateAssociatedTokenAccount::new(
            &mut svm,
            &taker,
            &mint_b,
        )
        .owner(&taker.pubkey())
        .send()
        .unwrap();

    // Give Bob 100 Token B.
    //
    // Maker is still the mint authority for mint_b,
    // because setup_make() created both mints using maker.
    MintTo::new(
        &mut svm,
        &maker,
        &mint_b,
        &taker_ata_b,
        100_000_000,
    )
    .send()
    .unwrap();

    // 3. Derive these ATAs, but DO NOT create them.
    //
    // Take will create them with CreateIdempotent.
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
    let system_program = solana_sdk_ids::system_program::ID;

    // 4. Take discriminator = 1.
    let take_ix = Instruction {
        program_id,
        accounts: vec![
            // 0 taker
            AccountMeta::new(taker.pubkey(), true),

            // 1 maker
            AccountMeta::new(maker.pubkey(), false),

            // 2 mint_a
            AccountMeta::new_readonly(mint_a, false),

            // 3 mint_b
            AccountMeta::new_readonly(mint_b, false),

            // 4 escrow_account
            AccountMeta::new(escrow_pda, false),

            // 5 vault
            AccountMeta::new(vault, false),

            // 6 taker_ata_a
            AccountMeta::new(taker_ata_a, false),

            // 7 taker_ata_b
            AccountMeta::new(taker_ata_b, false),

            // 8 maker_ata_b
            AccountMeta::new(maker_ata_b, false),

            // 9 system_program
            AccountMeta::new_readonly(system_program, false),

            // 10 token_program
            AccountMeta::new_readonly(token_program, false),

            // 11 associated_token_program
            AccountMeta::new_readonly(
                associated_token_program,
                false,
            ),
        ],

        data: vec![1u8],
    };

    // Bob is the fee payer and signer for Take.
    let message =
        Message::new(&[take_ix], Some(&taker.pubkey()));

    let recent_blockhash = svm.latest_blockhash();

    let transaction =
        Transaction::new(
            &[&taker],
            message,
            recent_blockhash,
        );

    let maker_sol_before = svm
        .get_account(&maker.pubkey())
        .unwrap()
        .lamports;

    // 5. Send Take.
    let tx = svm
        .send_transaction(transaction)
        .unwrap();

    println!("\nTake transaction successful");
    println!(
        "CUs Consumed: {}",
        tx.compute_units_consumed
    );// -----------------------------------------------------
    // Verify Bob received 500 Token A
    // -----------------------------------------------------
    
    let taker_ata_a_account = svm
        .get_account(&taker_ata_a)
        .unwrap();
    
    let taker_ata_a_state =
        spl_token_2022::state::Account::unpack(
            &taker_ata_a_account.data
        )
        .unwrap();
    
    println!(
        "Taker Token A balance: {}",
        taker_ata_a_state.amount
    );
    
    assert_eq!(
        taker_ata_a_state.amount,
        500_000_000
    );
    
    
    // -----------------------------------------------------
    // Verify Alice received 100 Token B
    // -----------------------------------------------------
    
    let maker_ata_b_account = svm
        .get_account(&maker_ata_b)
        .unwrap();
    
    let maker_ata_b_state =
        spl_token_2022::state::Account::unpack(
            &maker_ata_b_account.data
        )
        .unwrap();
    
    println!(
        "Maker Token B balance: {}",
        maker_ata_b_state.amount
    );
    
    assert_eq!(
        maker_ata_b_state.amount,
        100_000_000
    );
    
    
    // -----------------------------------------------------
    // Verify vault was closed
    // -----------------------------------------------------
    
    let vault_closed = match svm.get_account(&vault) {
        None => true,
    
        Some(account) => {
            account.lamports == 0
                && account.owner
                    == solana_sdk_ids::system_program::ID
        }
    };
    
    assert!(
        vault_closed,
        "Vault should be closed"
    );
    
    
    // -----------------------------------------------------
    // Verify escrow state account was closed
    // -----------------------------------------------------
    
    let escrow_closed = match svm.get_account(&escrow_pda) {
        None => true,
    
        Some(account) => {
            account.lamports == 0
                && account.owner
                    == solana_sdk_ids::system_program::ID
        }
    };
    
    assert!(
        escrow_closed,
        "Escrow account should be closed"
    );
    
    
    // -----------------------------------------------------
    // Verify Alice got the rent back
    // -----------------------------------------------------
    
    let maker_sol_after = svm
        .get_account(&maker.pubkey())
        .unwrap()
        .lamports;
    
        let maker_sol_before_sol =
        maker_sol_before as f64 / 1_000_000_000.0;
    
    let maker_sol_after_sol =
        maker_sol_after as f64 / 1_000_000_000.0;
    
    let rent_refunded_sol =
        (maker_sol_after - maker_sol_before) as f64 / 1_000_000_000.0;
    
    println!(
        "Maker SOL before: {:.9} SOL",
        maker_sol_before_sol
    );
    
    println!(
        "Maker SOL after:  {:.9} SOL",
        maker_sol_after_sol
    );
    
    println!(
        "Rent refunded:    {:.9} SOL",
        rent_refunded_sol
    );
    
    assert!(
        maker_sol_after > maker_sol_before,
        "Maker should receive rent from the closed vault and escrow accounts"
    );
}
#[test]
pub fn test_cancel_instruction() {
    // Make the escrow first.
    let (
        mut svm,
        maker,
        mint_a,
        _mint_b,
        escrow_pda,
        _bump,
        vault,
    ) = setup_make();

    let program_id = program_id();

    // Maker's Token A ATA already exists from Make.
    let maker_ata_a =
        spl_associated_token_account::get_associated_token_address(
            &maker.pubkey(),
            &mint_a,
        );

    let associated_token_program =
        ASSOCIATED_TOKEN_PROGRAM_ID
            .parse::<Pubkey>()
            .unwrap();

    let token_program = TOKEN_PROGRAM_ID;
    let system_program = solana_sdk_ids::system_program::ID;

    // Cancel discriminator = 2.
    let cancel_ix = Instruction {
        program_id,
        accounts: vec![
            // 0 maker
            AccountMeta::new(maker.pubkey(), true),

            // 1 mint_a
            AccountMeta::new_readonly(mint_a, false),

            // 2 escrow_account
            AccountMeta::new(escrow_pda, false),

            // 3 vault
            AccountMeta::new(vault, false),

            // 4 maker_ata_a
            AccountMeta::new(maker_ata_a, false),

            // 5 system_program
            AccountMeta::new_readonly(system_program, false),

            // 6 token_program
            AccountMeta::new_readonly(token_program, false),

            // 7 associated_token_program
            AccountMeta::new_readonly(
                associated_token_program,
                false,
            ),
        ],

        data: vec![2u8],
    };

    // Maker signs Cancel.
    let message =
        Message::new(&[cancel_ix], Some(&maker.pubkey()));

    let recent_blockhash = svm.latest_blockhash();

    let transaction =
        Transaction::new(
            &[&maker],
            message,
            recent_blockhash,
        );

    let tx = svm
        .send_transaction(transaction)
        .unwrap();

    println!("\nCancel transaction successful");
    println!("CUs Consumed: {}", tx.compute_units_consumed);

    // --------------------------------------------------
    // Maker should have all 1,000 Token A back
    // --------------------------------------------------

    let maker_ata_account =
        svm.get_account(&maker_ata_a).unwrap();

    let maker_ata_state =
        spl_token_2022::state::Account::unpack(
            &maker_ata_account.data,
        )
        .unwrap();

    println!(
        "Maker Token A balance after Cancel: {}",
        maker_ata_state.amount
    );

    assert_eq!(
        maker_ata_state.amount,
        1_000_000_000
    );

    // --------------------------------------------------
    // Vault should be closed
    // --------------------------------------------------

    let vault_closed = match svm.get_account(&vault) {
        None => true,
        Some(account) => {
            account.lamports == 0
                && account.owner
                    == solana_sdk_ids::system_program::ID
        }
    };

    assert!(
        vault_closed,
        "Vault should be closed after Cancel"
    );

    // --------------------------------------------------
    // Escrow state account should be closed
    // --------------------------------------------------

    let escrow_closed =
        match svm.get_account(&escrow_pda) {
            None => true,
            Some(account) => {
                account.lamports == 0
                    && account.owner
                        == solana_sdk_ids::system_program::ID
            }
        };

    assert!(
        escrow_closed,
        "Escrow account should be closed after Cancel"
    );
}

#[test]
pub fn test_take_insufficient_funds() {
    let (
        mut svm,
        maker,
        mint_a,
        mint_b,
        escrow_pda,
        _bump,
        vault,
    ) = setup_make();

    let program_id = program_id();

    let taker = Keypair::new();

    svm.airdrop(
        &taker.pubkey(),
        10_000_000_000,
    )
    .unwrap();

    let taker_ata_b =
        CreateAssociatedTokenAccount::new(
            &mut svm,
            &taker,
            &mint_b,
        )
        .owner(&taker.pubkey())
        .send()
        .unwrap();

    // Only give Bob 50 B.
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
    let system_program = solana_sdk_ids::system_program::ID;

    let take_ix = Instruction {
        program_id,
        accounts: vec![
            AccountMeta::new(taker.pubkey(), true),
            AccountMeta::new(maker.pubkey(), false),
            AccountMeta::new_readonly(mint_a, false),
            AccountMeta::new_readonly(mint_b, false),
            AccountMeta::new(escrow_pda, false),
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

    let message =
        Message::new(&[take_ix], Some(&taker.pubkey()));

    let transaction = Transaction::new(
        &[&taker],
        message,
        svm.latest_blockhash(),
    );

    let result = svm.send_transaction(transaction);

    assert!(
        result.is_err(),
        "Take should fail when taker only has 50 B"
    );

    println!("Underfunded Take correctly failed");

    // Bob should still have his 50 B.
    let taker_b_account =
        svm.get_account(&taker_ata_b).unwrap();

    let taker_b_state =
        spl_token_2022::state::Account::unpack(
            &taker_b_account.data,
        )
        .unwrap();

    assert_eq!(taker_b_state.amount, 50_000_000);

    // Vault should still contain all 500 A.
    let vault_account =
        svm.get_account(&vault).unwrap();

    let vault_state =
        spl_token_2022::state::Account::unpack(
            &vault_account.data,
        )
        .unwrap();

    assert_eq!(vault_state.amount, 500_000_000);

    // Escrow should still exist.
    assert!(
        svm.get_account(&escrow_pda).is_some(),
        "Escrow should remain after failed Take"
    );

    // These were supposed to be created inside Take.
    // Because the transaction failed atomically, they should not persist.
    assert!(
        svm.get_account(&taker_ata_a).is_none(),
        "Taker ATA A should not persist after failed transaction"
    );

    assert!(
        svm.get_account(&maker_ata_b).is_none(),
        "Maker ATA B should not persist after failed transaction"
    );
}

#[test]
pub fn test_cancel_by_stranger_fails() {
    let (
        mut svm,
        maker,
        mint_a,
        _mint_b,
        escrow_pda,
        _bump,
        vault,
    ) = setup_make();

    let program_id = program_id();

    let stranger = Keypair::new();

    svm.airdrop(
        &stranger.pubkey(),
        10_000_000_000,
    )
    .unwrap();

    let maker_ata_a =
        spl_associated_token_account::get_associated_token_address(
            &maker.pubkey(),
            &mint_a,
        );

    let associated_token_program =
        ASSOCIATED_TOKEN_PROGRAM_ID
            .parse::<Pubkey>()
            .unwrap();

    let token_program = TOKEN_PROGRAM_ID;
    let system_program = solana_sdk_ids::system_program::ID;

    let cancel_ix = Instruction {
        program_id,
        accounts: vec![
            // Stranger tries to present himself as maker.
            AccountMeta::new(stranger.pubkey(), true),

            AccountMeta::new_readonly(mint_a, false),
            AccountMeta::new(escrow_pda, false),
            AccountMeta::new(vault, false),
            AccountMeta::new(maker_ata_a, false),
            AccountMeta::new_readonly(system_program, false),
            AccountMeta::new_readonly(token_program, false),
            AccountMeta::new_readonly(
                associated_token_program,
                false,
            ),
        ],
        data: vec![2u8],
    };

    let message =
        Message::new(&[cancel_ix], Some(&stranger.pubkey()));

    let transaction = Transaction::new(
        &[&stranger],
        message,
        svm.latest_blockhash(),
    );

    let result = svm.send_transaction(transaction);

    assert!(
        result.is_err(),
        "A stranger must not be able to cancel another maker's escrow"
    );

    println!("Stranger Cancel correctly failed");

    // Vault should still contain Alice's 500 A.
    let vault_account =
        svm.get_account(&vault).unwrap();

    let vault_state =
        spl_token_2022::state::Account::unpack(
            &vault_account.data,
        )
        .unwrap();

    assert_eq!(vault_state.amount, 500_000_000);

    // Escrow must still exist.
    assert!(
        svm.get_account(&escrow_pda).is_some(),
        "Escrow should remain after unauthorized Cancel"
    );

    // Alice should still have only the 500 A left after Make.
    let maker_account =
        svm.get_account(&maker_ata_a).unwrap();

    let maker_state =
        spl_token_2022::state::Account::unpack(
            &maker_account.data,
        )
        .unwrap();

    assert_eq!(maker_state.amount, 500_000_000);
}

#[test]
pub fn test_take_with_wrong_maker_fails() {
    let (
        mut svm,
        real_maker,
        mint_a,
        mint_b,
        escrow_pda,
        _bump,
        vault,
    ) = setup_make();

    let program_id = program_id();

    // Bob / taker
    let taker = Keypair::new();

    svm.airdrop(
        &taker.pubkey(),
        10_000_000_000,
    )
    .unwrap();

    // Give Bob a Token B account with the FULL 100 B.
    // We want the test to fail because of the wrong maker,
    // not because Bob is underfunded.
    let taker_ata_b =
        CreateAssociatedTokenAccount::new(
            &mut svm,
            &taker,
            &mint_b,
        )
        .owner(&taker.pubkey())
        .send()
        .unwrap();

    MintTo::new(
        &mut svm,
        &real_maker,
        &mint_b,
        &taker_ata_b,
        100_000_000,
    )
    .send()
    .unwrap();

    // Create a different account and pretend it is the maker.
    let wrong_maker = Keypair::new();

    svm.airdrop(
        &wrong_maker.pubkey(),
        1_000_000_000,
    )
    .unwrap();

    // These destination ATAs should NOT exist yet.
    let taker_ata_a =
        spl_associated_token_account::get_associated_token_address(
            &taker.pubkey(),
            &mint_a,
        );

    let wrong_maker_ata_b =
        spl_associated_token_account::get_associated_token_address(
            &wrong_maker.pubkey(),
            &mint_b,
        );

    let associated_token_program =
        ASSOCIATED_TOKEN_PROGRAM_ID
            .parse::<Pubkey>()
            .unwrap();

    let token_program = TOKEN_PROGRAM_ID;
    let system_program = solana_sdk_ids::system_program::ID;

    let take_ix = Instruction {
        program_id,
        accounts: vec![
            // 0 taker
            AccountMeta::new(taker.pubkey(), true),

            // 1 WRONG maker
            AccountMeta::new(wrong_maker.pubkey(), false),

            // 2 mint_a
            AccountMeta::new_readonly(mint_a, false),

            // 3 mint_b
            AccountMeta::new_readonly(mint_b, false),

            // 4 escrow
            AccountMeta::new(escrow_pda, false),

            // 5 vault
            AccountMeta::new(vault, false),

            // 6 taker ATA A
            AccountMeta::new(taker_ata_a, false),

            // 7 taker ATA B
            AccountMeta::new(taker_ata_b, false),

            // 8 maker ATA B — corresponding to the fake maker
            AccountMeta::new(wrong_maker_ata_b, false),

            // 9 system program
            AccountMeta::new_readonly(system_program, false),

            // 10 token program
            AccountMeta::new_readonly(token_program, false),

            // 11 associated token program
            AccountMeta::new_readonly(
                associated_token_program,
                false,
            ),
        ],

        data: vec![1u8],
    };

    let message =
        Message::new(&[take_ix], Some(&taker.pubkey()));

    let transaction = Transaction::new(
        &[&taker],
        message,
        svm.latest_blockhash(),
    );

    let result = svm.send_transaction(transaction);

    assert!(
        result.is_err(),
        "Take must fail when passed maker does not match escrow.maker()"
    );

    println!("Take with wrong maker correctly failed");

    // --------------------------------------------------
    // Nothing should have moved
    // --------------------------------------------------

    // Bob still has all 100 B.
    let taker_b_account =
        svm.get_account(&taker_ata_b).unwrap();

    let taker_b_state =
        spl_token_2022::state::Account::unpack(
            &taker_b_account.data,
        )
        .unwrap();

    assert_eq!(taker_b_state.amount, 100_000_000);

    // Vault still contains all 500 A.
    let vault_account =
        svm.get_account(&vault).unwrap();

    let vault_state =
        spl_token_2022::state::Account::unpack(
            &vault_account.data,
        )
        .unwrap();

    assert_eq!(vault_state.amount, 500_000_000);

    // Escrow still exists.
    assert!(
        svm.get_account(&escrow_pda).is_some(),
        "Escrow should remain after wrong-maker Take"
    );

    // Program should have failed before creating destination ATAs.
    assert!(
        svm.get_account(&taker_ata_a).is_none(),
        "Taker ATA A should not have been created"
    );

    assert!(
        svm.get_account(&wrong_maker_ata_b).is_none(),
        "Wrong maker ATA B should not have been created"
    );
}
}