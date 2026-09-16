use solana_instruction::{AccountMeta, Instruction};
use solana_keypair::Keypair;
use solana_message::Message;
use solana_native_token::LAMPORTS_PER_SOL;
use solana_signer::Signer;
use solana_transaction::Transaction;

use super::setup::{
    assert_closed, make_escrow, program_id, token_amount, MakeSetup, INITIAL_MAKER_A,
    TOKEN_PROGRAM_ID,
};

fn cancel_ix(s: &MakeSetup) -> Instruction {
    Instruction {
        program_id: program_id(),
        accounts: vec![
            AccountMeta::new(s.maker.pubkey(), true),
            AccountMeta::new(s.mint_a, false),
            AccountMeta::new(s.escrow, false),
            AccountMeta::new(s.vault, false),
            AccountMeta::new(s.maker_ata_a, false),
            AccountMeta::new(TOKEN_PROGRAM_ID, false),
        ],
        data: vec![2u8],
    }
}

#[test]
fn test_cancel_instruction() {
    let mut s = make_escrow();
    let ix = cancel_ix(&s);
    let message = Message::new(&[ix], Some(&s.maker.pubkey()));
    let tx = Transaction::new(&[&s.maker], message, s.svm.latest_blockhash());
    let result = s.svm.send_transaction(tx).unwrap();
    println!("Cancel CUs Consumed: {}", result.compute_units_consumed);

    assert_eq!(token_amount(&s.svm, &s.maker_ata_a), INITIAL_MAKER_A);
    assert_closed(&s.svm, &s.vault, "vault");
    assert_closed(&s.svm, &s.escrow, "escrow");
}

#[test]
fn test_cancel_fails_if_stranger_signs() {
    let mut s = make_escrow();
    let stranger = Keypair::new();
    s.svm
        .airdrop(&stranger.pubkey(), 10 * LAMPORTS_PER_SOL)
        .unwrap();

    // Alice is still the maker account, but she does not sign. If the program only
    // checks AccountMeta flags from the client, this would pass; maker.is_signer() must fail.
    let mut ix = cancel_ix(&s);
    ix.accounts[0] = AccountMeta::new(s.maker.pubkey(), false);
    let message = Message::new(&[ix], Some(&stranger.pubkey()));
    let tx = Transaction::new(&[&stranger], message, s.svm.latest_blockhash());
    let result = s.svm.send_transaction(tx);
    assert!(
        result.is_err(),
        "a stranger must not be able to cancel someone else's escrow"
    );
    assert_eq!(token_amount(&s.svm, &s.vault), 500_000_000);
}
