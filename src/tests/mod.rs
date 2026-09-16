use litesvm::LiteSVM;
use litesvm_token::{spl_token, CreateAssociatedTokenAccount, CreateMint, MintTo};
use solana_instruction::{AccountMeta, Instruction};
use solana_keypair::Keypair;
use solana_message::Message;
use solana_native_token::LAMPORTS_PER_SOL;
use solana_program_pack::Pack;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use solana_transaction::Transaction;
use std::path::PathBuf;

const INITIAL_A: u64 = 1_000_000_000;
const RECEIVE_B: u64 = 100_000_000;
const GIVE_A: u64 = 500_000_000;

struct Fixture {
    svm: LiteSVM,
    maker: Keypair,
    mint_a: Pubkey,
    mint_b: Pubkey,
    maker_ata_a: Pubkey,
    escrow: Pubkey,
    bump: u8,
    vault: Pubkey,
}

fn program_id() -> Pubkey {
    Pubkey::from(crate::ID)
}

fn setup() -> (LiteSVM, Keypair) {
    let mut svm = LiteSVM::new();
    let payer = Keypair::new();
    #[allow(deprecated)]
    svm.set_sysvar(&solana_rent::Rent {
        lamports_per_byte_year: 6960,
        exemption_threshold: 1.0,
        burn_percent: 50,
    });
    svm.airdrop(&payer.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/deploy/escrow.so");
    let bytes = std::fs::read(&path).unwrap_or_else(|e| {
        panic!(
            "failed to read {}: {e}; run `cargo build-sbf` first",
            path.display()
        )
    });
    svm.add_program(program_id(), &bytes).unwrap();
    (svm, payer)
}

fn make_fixture() -> Fixture {
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
    let maker_ata_a = CreateAssociatedTokenAccount::new(&mut svm, &maker, &mint_a)
        .owner(&maker.pubkey())
        .send()
        .unwrap();
    MintTo::new(&mut svm, &maker, &mint_a, &maker_ata_a, INITIAL_A)
        .send()
        .unwrap();
    let (escrow, bump) =
        Pubkey::find_program_address(&[b"escrow", maker.pubkey().as_ref()], &program_id());
    let vault = spl_associated_token_account::get_associated_token_address(&escrow, &mint_a);
    let data = [
        vec![0],
        RECEIVE_B.to_le_bytes().to_vec(),
        GIVE_A.to_le_bytes().to_vec(),
    ]
    .concat();
    let ix = Instruction {
        program_id: program_id(),
        data,
        accounts: vec![
            AccountMeta::new(maker.pubkey(), true),
            AccountMeta::new_readonly(mint_a, false),
            AccountMeta::new_readonly(mint_b, false),
            AccountMeta::new(escrow, false),
            AccountMeta::new(maker_ata_a, false),
            AccountMeta::new(vault, false),
            AccountMeta::new_readonly(solana_sdk_ids::system_program::ID, false),
            AccountMeta::new_readonly(spl_token::ID, false),
            AccountMeta::new_readonly(spl_associated_token_account::ID, false),
        ],
    };
    let tx = Transaction::new(
        &[&maker],
        Message::new(&[ix], Some(&maker.pubkey())),
        svm.latest_blockhash(),
    );
    let meta = svm.send_transaction(tx).unwrap();
    println!("Make CUs Consumed: {}", meta.compute_units_consumed);
    Fixture {
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

fn amount(svm: &LiteSVM, key: &Pubkey) -> u64 {
    let acc = svm.get_account(key).expect("token account missing");
    spl_token_2022::state::Account::unpack(&acc.data)
        .unwrap()
        .amount
}

fn assert_closed(svm: &LiteSVM, key: &Pubkey) {
    if let Some(acc) = svm.get_account(key) {
        assert_eq!(acc.lamports, 0);
        assert_eq!(acc.owner, solana_sdk_ids::system_program::ID);
    }
}

fn take_ix(
    f: &Fixture,
    taker: &Keypair,
    maker: Pubkey,
    ata_a: Pubkey,
    ata_b: Pubkey,
    maker_b: Pubkey,
) -> Instruction {
    Instruction {
        program_id: program_id(),
        data: vec![1],
        accounts: vec![
            AccountMeta::new(taker.pubkey(), true),
            AccountMeta::new(maker, false),
            AccountMeta::new_readonly(f.mint_a, false),
            AccountMeta::new_readonly(f.mint_b, false),
            AccountMeta::new(f.escrow, false),
            AccountMeta::new(f.vault, false),
            AccountMeta::new(ata_a, false),
            AccountMeta::new(ata_b, false),
            AccountMeta::new(maker_b, false),
            AccountMeta::new_readonly(solana_sdk_ids::system_program::ID, false),
            AccountMeta::new_readonly(spl_token::ID, false),
            AccountMeta::new_readonly(spl_associated_token_account::ID, false),
        ],
    }
}

fn cancel_ix(f: &Fixture, signer: Pubkey) -> Instruction {
    Instruction {
        program_id: program_id(),
        data: vec![2],
        accounts: vec![
            AccountMeta::new(signer, true),
            AccountMeta::new_readonly(f.mint_a, false),
            AccountMeta::new(f.escrow, false),
            AccountMeta::new(f.vault, false),
            AccountMeta::new(f.maker_ata_a, false),
            AccountMeta::new_readonly(spl_token::ID, false),
        ],
    }
}

#[test]
fn test_make_instruction() {
    let f = make_fixture();
    assert_eq!(amount(&f.svm, &f.vault), GIVE_A);
    assert_eq!(amount(&f.svm, &f.maker_ata_a), INITIAL_A - GIVE_A);
    let esc = f.svm.get_account(&f.escrow).unwrap();
    assert_eq!(esc.owner, program_id());
    assert_eq!(esc.data.len(), 113);
    assert_eq!(&esc.data[0..32], f.maker.pubkey().as_ref());
    assert_eq!(&esc.data[32..64], f.mint_a.as_ref());
    assert_eq!(&esc.data[64..96], f.mint_b.as_ref());
    assert_eq!(
        u64::from_le_bytes(esc.data[96..104].try_into().unwrap()),
        RECEIVE_B
    );
    assert_eq!(
        u64::from_le_bytes(esc.data[104..112].try_into().unwrap()),
        GIVE_A
    );
    assert_eq!(esc.data[112], f.bump);
}

#[test]
fn test_take_instruction() {
    let mut f = make_fixture();
    let taker = Keypair::new();
    f.svm
        .airdrop(&taker.pubkey(), 2 * LAMPORTS_PER_SOL)
        .unwrap();
    let taker_b = CreateAssociatedTokenAccount::new(&mut f.svm, &taker, &f.mint_b)
        .owner(&taker.pubkey())
        .send()
        .unwrap();
    MintTo::new(&mut f.svm, &f.maker, &f.mint_b, &taker_b, RECEIVE_B)
        .send()
        .unwrap();
    let taker_a =
        spl_associated_token_account::get_associated_token_address(&taker.pubkey(), &f.mint_a);
    let maker_b =
        spl_associated_token_account::get_associated_token_address(&f.maker.pubkey(), &f.mint_b);
    let maker_sol = f.svm.get_balance(&f.maker.pubkey()).unwrap();
    let ix = take_ix(&f, &taker, f.maker.pubkey(), taker_a, taker_b, maker_b);
    let tx = Transaction::new(
        &[&taker],
        Message::new(&[ix], Some(&taker.pubkey())),
        f.svm.latest_blockhash(),
    );
    let meta = f.svm.send_transaction(tx).unwrap();
    println!("Take CUs Consumed: {}", meta.compute_units_consumed);
    assert_eq!(amount(&f.svm, &taker_a), GIVE_A);
    assert_eq!(amount(&f.svm, &maker_b), RECEIVE_B);
    assert_closed(&f.svm, &f.vault);
    assert_closed(&f.svm, &f.escrow);
    assert!(f.svm.get_balance(&f.maker.pubkey()).unwrap() > maker_sol);
}

#[test]
fn test_cancel_instruction() {
    let mut f = make_fixture();
    let ix = cancel_ix(&f, f.maker.pubkey());
    let tx = Transaction::new(
        &[&f.maker],
        Message::new(&[ix], Some(&f.maker.pubkey())),
        f.svm.latest_blockhash(),
    );
    let meta = f.svm.send_transaction(tx).unwrap();
    println!("Cancel CUs Consumed: {}", meta.compute_units_consumed);
    assert_eq!(amount(&f.svm, &f.maker_ata_a), INITIAL_A);
    assert_closed(&f.svm, &f.vault);
    assert_closed(&f.svm, &f.escrow);
}

#[test]
fn test_take_fails_with_only_50_b() {
    let mut f = make_fixture();
    let taker = Keypair::new();
    f.svm
        .airdrop(&taker.pubkey(), 2 * LAMPORTS_PER_SOL)
        .unwrap();
    let taker_b = CreateAssociatedTokenAccount::new(&mut f.svm, &taker, &f.mint_b)
        .owner(&taker.pubkey())
        .send()
        .unwrap();
    MintTo::new(&mut f.svm, &f.maker, &f.mint_b, &taker_b, RECEIVE_B / 2)
        .send()
        .unwrap();
    let taker_a =
        spl_associated_token_account::get_associated_token_address(&taker.pubkey(), &f.mint_a);
    let maker_b =
        spl_associated_token_account::get_associated_token_address(&f.maker.pubkey(), &f.mint_b);
    let ix = take_ix(&f, &taker, f.maker.pubkey(), taker_a, taker_b, maker_b);
    let tx = Transaction::new(
        &[&taker],
        Message::new(&[ix], Some(&taker.pubkey())),
        f.svm.latest_blockhash(),
    );
    let result = f.svm.send_transaction(tx);
    assert!(result.is_err(), "underfunded Take must fail");
    let failure = result.unwrap_err();
    let consumed = failure.meta.compute_units_consumed;
    println!("Underfunded Take CUs Consumed before rejection: {consumed}");
    assert_eq!(amount(&f.svm, &f.vault), GIVE_A);
    assert_eq!(amount(&f.svm, &taker_b), RECEIVE_B / 2);
    assert!(f.svm.get_account(&taker_a).is_none());
    assert!(f.svm.get_account(&maker_b).is_none());
    assert!(f.svm.get_account(&f.escrow).is_some());
}

#[test]
fn test_cancel_fails_for_stranger() {
    let mut f = make_fixture();
    let stranger = Keypair::new();
    f.svm.airdrop(&stranger.pubkey(), LAMPORTS_PER_SOL).unwrap();
    let ix = cancel_ix(&f, stranger.pubkey());
    let tx = Transaction::new(
        &[&stranger],
        Message::new(&[ix], Some(&stranger.pubkey())),
        f.svm.latest_blockhash(),
    );
    let result = f.svm.send_transaction(tx);
    assert!(result.is_err(), "a stranger must not be able to cancel");
    let failure = result.unwrap_err();
    let consumed = failure.meta.compute_units_consumed;
    println!("Stranger Cancel CUs Consumed before rejection: {consumed}");
    assert_eq!(amount(&f.svm, &f.vault), GIVE_A);
    assert_eq!(amount(&f.svm, &f.maker_ata_a), INITIAL_A - GIVE_A);
    assert!(f.svm.get_account(&f.escrow).is_some());
}
