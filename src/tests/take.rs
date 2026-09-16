use litesvm_token::{CreateAssociatedTokenAccount, MintTo};
use solana_instruction::{AccountMeta, Instruction};
use solana_keypair::Keypair;
use solana_message::Message;
use solana_native_token::LAMPORTS_PER_SOL;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use solana_transaction::Transaction;

use super::setup::{
    assert_closed, associated_token_program, make_escrow, program_id, system_program, token_amount,
    MakeSetup, TOKEN_PROGRAM_ID,
};

fn take_ix(
    s: &MakeSetup,
    taker: &Pubkey,
    taker_ata_a: Pubkey,
    taker_ata_b: Pubkey,
    maker_ata_b: Pubkey,
) -> Instruction {
    Instruction {
        program_id: program_id(),
        accounts: vec![
            AccountMeta::new(*taker, true),
            AccountMeta::new(s.maker.pubkey(), false),
            AccountMeta::new(s.mint_a, false),
            AccountMeta::new(s.mint_b, false),
            AccountMeta::new(s.escrow, false),
            AccountMeta::new(s.vault, false),
            AccountMeta::new(taker_ata_a, false),
            AccountMeta::new(taker_ata_b, false),
            AccountMeta::new(maker_ata_b, false),
            AccountMeta::new(system_program(), false),
            AccountMeta::new(TOKEN_PROGRAM_ID, false),
            AccountMeta::new(associated_token_program(), false),
        ],
        data: vec![1u8],
    }
}

#[test]
fn test_take_instruction() {
    let mut s = make_escrow();
    let taker = Keypair::new();
    s.svm
        .airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL)
        .unwrap();

    let taker_ata_b = CreateAssociatedTokenAccount::new(&mut s.svm, &taker, &s.mint_b)
        .owner(&taker.pubkey())
        .send()
        .unwrap();
    MintTo::new(
        &mut s.svm,
        &s.maker,
        &s.mint_b,
        &taker_ata_b,
        s.amount_to_receive,
    )
    .send()
    .unwrap();

    let taker_ata_a =
        spl_associated_token_account::get_associated_token_address(&taker.pubkey(), &s.mint_a);
    let maker_ata_b =
        spl_associated_token_account::get_associated_token_address(&s.maker.pubkey(), &s.mint_b);

    let maker_sol_before = s.svm.get_balance(&s.maker.pubkey()).unwrap();

    let ix = take_ix(
        &s,
        &taker.pubkey(),
        taker_ata_a,
        taker_ata_b,
        maker_ata_b,
    );
    let message = Message::new(&[ix], Some(&taker.pubkey()));
    let tx = Transaction::new(&[&taker], message, s.svm.latest_blockhash());
    let result = s.svm.send_transaction(tx).unwrap();
    println!("Take CUs Consumed: {}", result.compute_units_consumed);

    assert_eq!(token_amount(&s.svm, &taker_ata_a), s.amount_to_give);
    assert_eq!(token_amount(&s.svm, &maker_ata_b), s.amount_to_receive);
    assert_closed(&s.svm, &s.vault, "vault");
    assert_closed(&s.svm, &s.escrow, "escrow");
    assert!(
        s.svm.get_balance(&s.maker.pubkey()).unwrap() > maker_sol_before,
        "maker should receive rent refunds"
    );
}

#[test]
fn test_take_fails_if_underfunded() {
    let mut s = make_escrow();
    let taker = Keypair::new();
    s.svm
        .airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL)
        .unwrap();

    let taker_ata_b = CreateAssociatedTokenAccount::new(&mut s.svm, &taker, &s.mint_b)
        .owner(&taker.pubkey())
        .send()
        .unwrap();
    MintTo::new(&mut s.svm, &s.maker, &s.mint_b, &taker_ata_b, 50_000_000)
        .send()
        .unwrap();

    let taker_ata_a =
        spl_associated_token_account::get_associated_token_address(&taker.pubkey(), &s.mint_a);
    let maker_ata_b =
        spl_associated_token_account::get_associated_token_address(&s.maker.pubkey(), &s.mint_b);

    let ix = take_ix(
        &s,
        &taker.pubkey(),
        taker_ata_a,
        taker_ata_b,
        maker_ata_b,
    );
    let message = Message::new(&[ix], Some(&taker.pubkey()));
    let tx = Transaction::new(&[&taker], message, s.svm.latest_blockhash());
    let result = s.svm.send_transaction(tx);
    assert!(result.is_err(), "take must fail if taker has only 50 B");
    assert_eq!(token_amount(&s.svm, &s.vault), 500_000_000);
}
