use super::*;

#[test]
fn test_cancel_instruction() {
    let (mut svm, maker, mint_a, _mint_b, maker_ata_a, escrow, vault) = make_escrow();
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
    let message = Message::new(&[ix], Some(&maker.pubkey()));
    let transaction = Transaction::new(&[&maker], message, svm.latest_blockhash());
    let result = svm.send_transaction(transaction).unwrap();
    println!("Cancel CUs Consumed: {}", result.compute_units_consumed);

    assert_eq!(token_amount(&svm, &maker_ata_a), 1_000_000_000);
    assert_closed(&svm, &vault);
    assert_closed(&svm, &escrow);
}

#[test]
fn test_cancel_rejects_stranger() {
    let (mut svm, _maker, mint_a, _mint_b, maker_ata_a, escrow, vault) = make_escrow();
    let stranger = Keypair::new();
    svm.airdrop(&stranger.pubkey(), LAMPORTS_PER_SOL).unwrap();
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
    let message = Message::new(&[ix], Some(&stranger.pubkey()));
    let transaction = Transaction::new(&[&stranger], message, svm.latest_blockhash());

    assert!(svm.send_transaction(transaction).is_err());
    assert_eq!(token_amount(&svm, &vault), 500_000_000);
    assert!(svm.get_account(&escrow).is_some());
}
