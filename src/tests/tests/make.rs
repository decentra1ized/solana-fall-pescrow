use super::*;

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
    let mint_b = CreateMint::new(&mut svm, &payer)
        .decimals(6)
        .authority(&payer.pubkey())
        .send()
        .unwrap();
    let maker_ata_a = CreateAssociatedTokenAccount::new(&mut svm, &payer, &mint_a)
        .owner(&payer.pubkey())
        .send()
        .unwrap();
    MintTo::new(&mut svm, &payer, &mint_a, &maker_ata_a, 1_000_000_000)
        .send()
        .unwrap();

    let escrow = Pubkey::find_program_address(
        &[b"escrow".as_ref(), payer.pubkey().as_ref()],
        &PROGRAM_ID.parse().unwrap(),
    );
    let vault = spl_associated_token_account::get_associated_token_address(&escrow.0, &mint_a);
    let make_ix = Instruction {
        program_id,
        accounts: vec![
            AccountMeta::new(payer.pubkey(), true),
            AccountMeta::new(mint_a, false),
            AccountMeta::new(mint_b, false),
            AccountMeta::new(escrow.0, false),
            AccountMeta::new(maker_ata_a, false),
            AccountMeta::new(vault, false),
            AccountMeta::new(solana_sdk_ids::system_program::ID, false),
            AccountMeta::new(TOKEN_PROGRAM_ID, false),
            AccountMeta::new(ASSOCIATED_TOKEN_PROGRAM_ID.parse().unwrap(), false),
        ],
        data: [
            vec![0u8],
            100_000_000u64.to_le_bytes().to_vec(),
            500_000_000u64.to_le_bytes().to_vec(),
        ]
        .concat(),
    };
    let message = Message::new(&[make_ix], Some(&payer.pubkey()));
    let transaction = Transaction::new(&[&payer], message, svm.latest_blockhash());
    let tx = svm.send_transaction(transaction).unwrap();
    println!("Make CUs Consumed: {}", tx.compute_units_consumed);

    let vault_acc = svm.get_account(&vault).unwrap();
    let vault_state = spl_token_2022::state::Account::unpack(&vault_acc.data).unwrap();
    assert_eq!(vault_state.owner, escrow.0);
    assert_eq!(vault_state.amount, 500_000_000);

    let maker_acc = svm.get_account(&maker_ata_a).unwrap();
    let maker_state = spl_token_2022::state::Account::unpack(&maker_acc.data).unwrap();
    assert_eq!(maker_state.amount, 500_000_000);

    let escrow_acc = svm.get_account(&escrow.0).unwrap();
    assert_eq!(&escrow_acc.data[0..32], payer.pubkey().as_ref());
    assert_eq!(
        u64::from_le_bytes(escrow_acc.data[96..104].try_into().unwrap()),
        100_000_000
    );
    assert_eq!(
        u64::from_le_bytes(escrow_acc.data[104..112].try_into().unwrap()),
        500_000_000
    );
    assert_eq!(escrow_acc.data[112], escrow.1);
}
