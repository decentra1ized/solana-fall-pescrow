use litesvm_token::CreateMint;

use solana_instruction::{AccountMeta, Instruction};
use solana_keypair::Keypair;
use solana_message::Message;
use solana_native_token::LAMPORTS_PER_SOL;
use solana_program_pack::Pack;
use solana_pubkey::Pubkey;
use solana_signer::Signer;

use crate::tests::test_cases::*;

#[test]
pub fn test_cancel_instruction() {
    let (mut svm, maker) = setup();

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

    let maker_ata_a =
        spl_associated_token_account::get_associated_token_address(&maker.pubkey(), &mint_a);

    let cancel_ix = Instruction {
        program_id: program_id(),
        accounts: vec![
            AccountMeta::new(maker.pubkey(), true),
            AccountMeta::new(mint_a, false),
            AccountMeta::new(escrow, false),
            AccountMeta::new(vault, false),
            AccountMeta::new(maker_ata_a, false),
            AccountMeta::new(TOKEN_PROGRAM_ID, false),
            AccountMeta::new(
                ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap(),
                false,
            ),
        ],
        data: vec![2u8],
    };

    let message = Message::new(&[cancel_ix], Some(&maker.pubkey()));
    let tx = solana_transaction::Transaction::new(&[&maker], message, svm.latest_blockhash());
    let tx = svm.send_transaction(tx).unwrap();

    println!("Cancel transaction successful");
    println!("CUs Consumed: {}", tx.compute_units_consumed);

    let maker_ata_a_account = svm.get_account(&maker_ata_a).unwrap();
    let maker_ata_a_state =
        spl_token_2022::state::Account::unpack(&maker_ata_a_account.data).unwrap();
    assert_eq!(maker_ata_a_state.amount, 1_000_000_000);

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

    assert_eq!(amount_to_give, 500_000_000);
}

#[test]
pub fn test_cancel_instruction_fails_for_stranger() {
    let (mut svm, maker) = setup();
    let stranger = Keypair::new();
    svm.airdrop(&stranger.pubkey(), 10 * LAMPORTS_PER_SOL)
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

    let maker_ata_a =
        spl_associated_token_account::get_associated_token_address(&maker.pubkey(), &mint_a);

    let cancel_ix = Instruction {
        program_id: program_id(),
        accounts: vec![
            AccountMeta::new(stranger.pubkey(), true),
            AccountMeta::new(mint_a, false),
            AccountMeta::new(escrow, false),
            AccountMeta::new(vault, false),
            AccountMeta::new(maker_ata_a, false),
            AccountMeta::new(TOKEN_PROGRAM_ID, false),
            AccountMeta::new(
                ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap(),
                false,
            ),
        ],
        data: vec![2u8],
    };

    let tx = solana_transaction::Transaction::new(
        &[&stranger],
        Message::new(&[cancel_ix], Some(&stranger.pubkey())),
        svm.latest_blockhash(),
    );
    let result = svm.send_transaction(tx);
    assert!(
        result.is_err(),
        "a stranger must not be able to cancel someone else's escrow"
    );

    let vault_account = svm.get_account(&vault).unwrap();
    let vault_state = spl_token_2022::state::Account::unpack(&vault_account.data).unwrap();
    assert_eq!(vault_state.amount, amount_to_give);

    let maker_ata_a_account = svm.get_account(&maker_ata_a).unwrap();
    let maker_ata_a_state =
        spl_token_2022::state::Account::unpack(&maker_ata_a_account.data).unwrap();
    assert_eq!(maker_ata_a_state.amount, 500_000_000);
}
