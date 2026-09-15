use litesvm_token::{CreateAssociatedTokenAccount, CreateMint, MintTo};

use solana_instruction::{AccountMeta, Instruction};
use solana_message::Message;
use solana_program_pack::Pack;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use solana_transaction::Transaction;

use crate::tests::test_cases::*;

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
        .owner(&payer.pubkey())
        .send()
        .unwrap();
    println!("Maker ATA A: {}\n", maker_ata_a);

    // Derive the PDA for the escrow account using the maker's public key and a seed value
    let escrow = Pubkey::find_program_address(
        &[b"escrow".as_ref(), payer.pubkey().as_ref()],
        &PROGRAM_ID.parse().unwrap(),
    );
    println!("Escrow PDA: {}\n", escrow.0);

    // Derive the PDA for the vault associated token account using the escrow PDA and Mint A
    let vault = spl_associated_token_account::get_associated_token_address(
        &escrow.0, // owner will be the escrow PDA
        &mint_a,   // mint
    );
    println!("Vault PDA: {}\n", vault);

    // Define program IDs for associated token program, token program, and system program
    let associated_token_program = ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();
    let token_program = TOKEN_PROGRAM_ID;
    let system_program = solana_sdk_ids::system_program::ID;

    // Mint 1,000 tokens (with 6 decimal places) of Mint A to the maker's associated token account
    MintTo::new(&mut svm, &payer, &mint_a, &maker_ata_a, 1_000_000_000)
        .send()
        .unwrap();

    let amount_to_receive: u64 = 100_000_000; // 100 tokens with 6 decimal places
    let amount_to_give: u64 = 500_000_000; // 500 tokens with 6 decimal places
    let bump: u8 = escrow.1; // canonical bump; the program derives the same one on-chain

    println!("Bump: {}", bump);

    // Create the "Make" instruction to deposit tokens into the escrow
    let make_data = [
        vec![0u8], // Discriminator for "Make" instruction
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
    println!(
        "Vault owner: {} (escrow PDA? {})",
        vault_state.owner,
        vault_state.owner == escrow.0
    );
    println!("Vault balance: {}", vault_state.amount);
    assert_eq!(vault_state.amount, amount_to_give);

    let maker_acc = svm.get_account(&maker_ata_a).unwrap();
    let maker_state = spl_token_2022::state::Account::unpack(&maker_acc.data).unwrap();
    println!("Maker ATA balance: {}", maker_state.amount);
    assert_eq!(maker_state.amount, 1000000000 - amount_to_give);

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
    assert_eq!(d[112], bump);
}
