use litesvm_token::{CreateAssociatedTokenAccount, CreateMint, MintTo};

use solana_instruction::{AccountMeta, Instruction};
use solana_keypair::Keypair;
use solana_message::Message;
use solana_native_token::LAMPORTS_PER_SOL;
use solana_program_pack::Pack;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use solana_transaction::Transaction;

use crate::tests::test_cases::*;

#[test]
pub fn test_take_instruction() {
    let (mut svm, maker) = setup();
    let taker = Keypair::new();
    svm.airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL)
        .expect("Airdrop failed");

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

    let (escrow, vault, _bump, amount_to_receive, amount_to_give) =
        run_make_helper(&mut svm, &maker, mint_a, mint_b);

    let taker_ata_a =
        spl_associated_token_account::get_associated_token_address(&taker.pubkey(), &mint_a);
    let maker_ata_b =
        spl_associated_token_account::get_associated_token_address(&maker.pubkey(), &mint_b);

    let taker_ata_b = CreateAssociatedTokenAccount::new(&mut svm, &taker, &mint_b)
        .owner(&taker.pubkey())
        .send()
        .unwrap();
    MintTo::new(&mut svm, &maker, &mint_b, &taker_ata_b, 100_000_000)
        .send()
        .unwrap();

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
            AccountMeta::new(solana_sdk_ids::system_program::ID, false),
            AccountMeta::new(TOKEN_PROGRAM_ID, false),
            AccountMeta::new(
                ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap(),
                false,
            ),
        ],
        data: vec![1u8],
    };

    let maker_balance_before = svm.get_balance(&maker.pubkey());
    println!("before take escrow account: {:?}", svm.get_account(&escrow));
    println!("before take vault account: {:?}", svm.get_account(&vault));
    let message = Message::new(&[take_ix], Some(&taker.pubkey()));
    let tx = Transaction::new(&[&taker], message, svm.latest_blockhash());
    let tx = svm.send_transaction(tx).unwrap();

    println!("Take transaction successful");
    println!("CUs Consumed: {}", tx.compute_units_consumed);

    let taker_ata_a_account = svm.get_account(&taker_ata_a).unwrap();
    let taker_ata_a_state =
        spl_token_2022::state::Account::unpack(&taker_ata_a_account.data).unwrap();
    assert_eq!(taker_ata_a_state.amount, amount_to_give);

    let maker_ata_b_account = svm.get_account(&maker_ata_b).unwrap();
    let maker_ata_b_state =
        spl_token_2022::state::Account::unpack(&maker_ata_b_account.data).unwrap();
    assert_eq!(maker_ata_b_state.amount, amount_to_receive);

    let vault_account = svm.get_account(&vault);
    assert!(
        vault_account.is_none() || {
            let account = vault_account.unwrap();
            account.lamports == 0 && account.owner == solana_sdk_ids::system_program::ID
        }
    );

    let escrow_account = svm.get_account(&escrow);
    assert!(
        escrow_account.is_none() || {
            let account = escrow_account.unwrap();
            account.lamports == 0 && account.owner == solana_sdk_ids::system_program::ID
        }
    );

    let maker_balance_after = svm.get_balance(&maker.pubkey());
    assert!(maker_balance_after > maker_balance_before);
}

#[test]
pub fn test_take_instruction_fails_with_underfunded_taker_ata_b() {
    let (mut svm, maker) = setup();
    let taker = Keypair::new();
    svm.airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL)
        .expect("Airdrop failed");

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

    let (escrow, vault, _bump, _amount_to_receive, amount_to_give) =
        run_make_helper(&mut svm, &maker, mint_a, mint_b);

    let taker_ata_a =
        spl_associated_token_account::get_associated_token_address(&taker.pubkey(), &mint_a);
    let maker_ata_b =
        spl_associated_token_account::get_associated_token_address(&maker.pubkey(), &mint_b);

    let taker_ata_b = CreateAssociatedTokenAccount::new(&mut svm, &taker, &mint_b)
        .owner(&taker.pubkey())
        .send()
        .unwrap();
    MintTo::new(&mut svm, &maker, &mint_b, &taker_ata_b, 50_000_000)
        .send()
        .unwrap();

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
            AccountMeta::new(solana_sdk_ids::system_program::ID, false),
            AccountMeta::new(TOKEN_PROGRAM_ID, false),
            AccountMeta::new(
                ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap(),
                false,
            ),
        ],
        data: vec![1u8],
    };

    let tx = Transaction::new(
        &[&taker],
        Message::new(&[take_ix], Some(&taker.pubkey())),
        svm.latest_blockhash(),
    );
    let result = svm.send_transaction(tx);
    assert!(
        result.is_err(),
        "a taker with only 50 B must not be able to pay the 100 B settlement"
    );

    let vault_account = svm.get_account(&vault).unwrap();
    let vault_state = spl_token_2022::state::Account::unpack(&vault_account.data).unwrap();
    assert_eq!(vault_state.amount, amount_to_give);

    assert!(svm.get_account(&taker_ata_a).is_none());
    assert!(svm.get_account(&maker_ata_b).is_none());
}
