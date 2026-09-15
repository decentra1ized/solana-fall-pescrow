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
    (Pubkey, u8),
    Pubkey,
    u64,
    u64,
) {
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

        return (
    svm,
    payer,
    mint_a,
    mint_b,
    escrow,
    vault,
    amount_to_receive,
    amount_to_give,
);
}



#[test]
pub fn test_take_instruction() {
    let (
        mut svm,
        payer,
        mint_a,
        mint_b,
        escrow,
        vault,
        _amount_to_receive,
        _amount_to_give,
    ) = setup_make();

    // Create a second user who will take the escrow.
    let taker = Keypair::new();

    svm.airdrop(&taker.pubkey(), 2 * LAMPORTS_PER_SOL)
        .unwrap();

    let taker_ata_b =
        spl_associated_token_account::get_associated_token_address(
            &taker.pubkey(),
            &mint_b,
        );

    // Create taker's B ATA and give the taker 100 B.
    CreateAssociatedTokenAccount::new(&mut svm, &taker, &mint_b)
        .owner(&taker.pubkey())
        .send()
        .unwrap();

    MintTo::new(&mut svm, &payer, &mint_b, &taker_ata_b, 100_000_000)
        .send()
        .unwrap();

    // These two ATAs should NOT exist yet.
    // Take should create them with CreateIdempotent.
    let taker_ata_a =
        spl_associated_token_account::get_associated_token_address(
            &taker.pubkey(),
            &mint_a,
        );

    let maker_ata_b =
        spl_associated_token_account::get_associated_token_address(
            &payer.pubkey(),
            &mint_b,
        );

    assert!(svm.get_account(&taker_ata_a).is_none());
    assert!(svm.get_account(&maker_ata_b).is_none());

    let program_id = program_id();

    let instruction = Instruction {
        program_id,
        accounts: vec![
            // 0. taker
            AccountMeta::new(taker.pubkey(), true),

            // 1. maker
            AccountMeta::new(payer.pubkey(), false),

            // 2. mint_a
            AccountMeta::new_readonly(mint_a, false),

            // 3. mint_b
            AccountMeta::new_readonly(mint_b, false),

            // 4. escrow
            AccountMeta::new(escrow.0, false),

            // 5. vault
            AccountMeta::new(vault, false),

            // 6. taker ATA A
            AccountMeta::new(taker_ata_a, false),

            // 7. taker ATA B
            AccountMeta::new(taker_ata_b, false),

            // 8. maker ATA B
            AccountMeta::new(maker_ata_b, false),

            // 9. system program
            AccountMeta::new_readonly(solana_sdk_ids::system_program::ID, false),

            // 10. token program
            AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),

            // 11. associated token program
            AccountMeta::new_readonly(
                spl_associated_token_account::ID,
                false,
            ),
        ],
        data: vec![1u8],
    };

    let tx = Transaction::new_signed_with_payer(
        &[instruction],
        Some(&taker.pubkey()),
        &[&taker],
        svm.latest_blockhash(),
    );

    let result = svm.send_transaction(tx);
    println!("Take result: {:?}", result);

    assert!(result.is_ok());

    // Taker should receive 500 A.
    let taker_a_acc = svm.get_account(&taker_ata_a).unwrap();
    let taker_a_state =
        spl_token::state::Account::unpack(&taker_a_acc.data).unwrap();

    println!("Taker ATA A balance: {}", taker_a_state.amount);
    assert_eq!(taker_a_state.amount, 500_000_000);

    // Maker should receive 100 B.
    let maker_b_acc = svm.get_account(&maker_ata_b).unwrap();
    let maker_b_state =
        spl_token::state::Account::unpack(&maker_b_acc.data).unwrap();

    println!("Maker ATA B balance: {}", maker_b_state.amount);
    assert_eq!(maker_b_state.amount, 100_000_000);

    // The vault and escrow should both be closed.
    assert!(svm.get_account(&vault).is_none());
    assert!(svm.get_account(&escrow.0).is_none());
}


    

#[test]
pub fn test_make_instruction() {
    let (
        svm,
        payer,
        mint_a,
        mint_b,
        escrow,
        vault,
        amount_to_receive,
        amount_to_give,
    ) = setup_make();

    let program_id = program_id();

    // Verify vault
    let vault_acc = svm.get_account(&vault).unwrap();
    let vault_state =
        spl_token::state::Account::unpack(&vault_acc.data).unwrap();

    println!(
        "Vault owner: {} (escrow PDA? {})",
        vault_state.owner,
        vault_state.owner == escrow.0
    );
    println!("Vault balance: {}", vault_state.amount);

    assert_eq!(vault_state.amount, amount_to_give);

    // Verify maker's token balance
    let maker_ata_a =
        spl_associated_token_account::get_associated_token_address(
            &payer.pubkey(),
            &mint_a,
        );

    let maker_acc = svm.get_account(&maker_ata_a).unwrap();
    let maker_state =
        spl_token_2022::state::Account::unpack(&maker_acc.data).unwrap();

    println!("Maker ATA balance: {}", maker_state.amount);

    assert_eq!(
        maker_state.amount,
        1000000000 - amount_to_give
    );

    // Verify escrow state
    let esc = svm.get_account(&escrow.0).unwrap();

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

    assert_eq!(&d[0..32], payer.pubkey().as_ref());
    assert_eq!(
        u64::from_le_bytes(d[96..104].try_into().unwrap()),
        amount_to_receive
    );
    assert_eq!(
        u64::from_le_bytes(d[104..112].try_into().unwrap()),
        amount_to_give
    );
}

#[test]
pub fn test_cancel_instruction() {
    let (
        mut svm,
        payer,
        mint_a,
        _mint_b,
        escrow,
        vault,
        _amount_to_receive,
        _amount_to_give,
    ) = setup_make();

    let maker_ata_a =
        spl_associated_token_account::get_associated_token_address(
            &payer.pubkey(),
            &mint_a,
        );

    let program_id = program_id();

    // Cancel instruction
    let instruction = Instruction {
        program_id,
        accounts: vec![
            AccountMeta::new(payer.pubkey(), true),       // maker
            AccountMeta::new_readonly(mint_a, false),     // mint_a
            AccountMeta::new(escrow.0, false),            // escrow
            AccountMeta::new(vault, false),               // vault
            AccountMeta::new(maker_ata_a, false),          // maker ATA A
            AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),   // token program
        ],
        data: vec![2u8],
    };

    let tx = Transaction::new_signed_with_payer(
        &[instruction],
        Some(&payer.pubkey()),
        &[&payer],
        svm.latest_blockhash(),
    );

    let result = svm.send_transaction(tx);

    println!("Cancel result: {:?}", result);

    assert!(result.is_ok());

    // Maker should get the 500 A back
    let maker_acc = svm.get_account(&maker_ata_a).unwrap();
    let maker_state =
        spl_token_2022::state::Account::unpack(&maker_acc.data).unwrap();

    println!("Maker ATA balance after cancel: {}", maker_state.amount);

    assert_eq!(maker_state.amount, 1_000_000_000);

    // Vault should be closed
    assert!(svm.get_account(&vault).is_none());

    // Escrow should be closed
    assert!(svm.get_account(&escrow.0).is_none());
}


#[test]
pub fn test_take_insufficient_balance() {
    let (
        mut svm,
        payer,
        mint_a,
        mint_b,
        escrow,
        vault,
        _amount_to_receive,
        _amount_to_give,
    ) = setup_make();

    let taker = Keypair::new();

    svm.airdrop(&taker.pubkey(), 2 * LAMPORTS_PER_SOL)
        .unwrap();

    // Create taker's B ATA and give the taker only 50 B.
    let taker_ata_b =
        spl_associated_token_account::get_associated_token_address(
            &taker.pubkey(),
            &mint_b,
        );

    CreateAssociatedTokenAccount::new(&mut svm, &taker, &mint_b)
        .owner(&taker.pubkey())
        .send()
        .unwrap();

    MintTo::new(&mut svm, &payer, &mint_b, &taker_ata_b, 50_000_000)
        .send()
        .unwrap();

    let taker_ata_a =
        spl_associated_token_account::get_associated_token_address(
            &taker.pubkey(),
            &mint_a,
        );

    let maker_ata_b =
        spl_associated_token_account::get_associated_token_address(
            &payer.pubkey(),
            &mint_b,
        );

    // These don't exist yet. Take would normally create them.
    assert!(svm.get_account(&taker_ata_a).is_none());
    assert!(svm.get_account(&maker_ata_b).is_none());

    let instruction = Instruction {
        program_id: program_id(),
        accounts: vec![
            AccountMeta::new(taker.pubkey(), true),
            AccountMeta::new(payer.pubkey(), false),
            AccountMeta::new_readonly(mint_a, false),
            AccountMeta::new_readonly(mint_b, false),
            AccountMeta::new(escrow.0, false),
            AccountMeta::new(vault, false),
            AccountMeta::new(taker_ata_a, false),
            AccountMeta::new(taker_ata_b, false),
            AccountMeta::new(maker_ata_b, false),
            AccountMeta::new_readonly(
                solana_sdk_ids::system_program::ID,
                false,
            ),
            AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
            AccountMeta::new_readonly(
                spl_associated_token_account::ID,
                false,
            ),
        ],
        data: vec![1u8],
    };

    let tx = Transaction::new_signed_with_payer(
        &[instruction],
        Some(&taker.pubkey()),
        &[&taker],
        svm.latest_blockhash(),
    );

    let result = svm.send_transaction(tx);

    println!("Take with insufficient balance: {:?}", result);

    assert!(result.is_err());

    // Vault should still contain the original 500 A.
    let vault_acc = svm.get_account(&vault).unwrap();
    let vault_state =
        spl_token::state::Account::unpack(&vault_acc.data).unwrap();

    assert_eq!(vault_state.amount, 500_000_000);

    // Escrow should still exist.
    assert!(svm.get_account(&escrow.0).is_some());

    // Taker should not have received A.
    assert!(svm.get_account(&taker_ata_a).is_none());

    // Maker should not have received B.
    assert!(svm.get_account(&maker_ata_b).is_none());

    // Taker should still have only 50 B.
    let taker_b_acc = svm.get_account(&taker_ata_b).unwrap();
    let taker_b_state =
        spl_token::state::Account::unpack(&taker_b_acc.data).unwrap();

    assert_eq!(taker_b_state.amount, 50_000_000);
}

#[test]
pub fn test_cancel_unauthorized() {
    let (
        mut svm,
        payer,
        mint_a,
        _mint_b,
        escrow,
        vault,
        _amount_to_receive,
        _amount_to_give,
    ) = setup_make();

    let maker_ata_a =
        spl_associated_token_account::get_associated_token_address(
            &payer.pubkey(),
            &mint_a,
        );

    // A different user tries to cancel the maker's escrow.
    let stranger = Keypair::new();

    svm.airdrop(&stranger.pubkey(), 2 * LAMPORTS_PER_SOL)
        .unwrap();

    let instruction = Instruction {
        program_id: program_id(),
        accounts: vec![
            // The program expects this account to be the maker.
            // The stranger signs instead.
            AccountMeta::new(stranger.pubkey(), true),

            AccountMeta::new_readonly(mint_a, false),
            AccountMeta::new(escrow.0, false),
            AccountMeta::new(vault, false),
            AccountMeta::new(maker_ata_a, false),
            AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
        ],
        data: vec![2u8],
    };

    let tx = Transaction::new_signed_with_payer(
        &[instruction],
        Some(&stranger.pubkey()),
        &[&stranger],
        svm.latest_blockhash(),
    );

    let result = svm.send_transaction(tx);

    println!("Unauthorized cancel result: {:?}", result);

    assert!(result.is_err());

    // The vault must still contain the original 500 A.
    let vault_acc = svm.get_account(&vault).unwrap();
    let vault_state =
        spl_token::state::Account::unpack(&vault_acc.data).unwrap();

    println!("Vault balance after unauthorized cancel: {}", vault_state.amount);

    assert_eq!(vault_state.amount, 500_000_000);

    // Escrow must still exist.
    assert!(svm.get_account(&escrow.0).is_some());
}


}


