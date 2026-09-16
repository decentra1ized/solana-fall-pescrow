use std::path::PathBuf;

use litesvm::LiteSVM;
use litesvm_token::{
    spl_token::{self},
    CreateAssociatedTokenAccount, CreateMint, MintTo,
};
use solana_instruction::{AccountMeta, Instruction};
use solana_keypair::Keypair;
use solana_message::Message;
use solana_native_token::LAMPORTS_PER_SOL;
use solana_program_pack::Pack;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use solana_transaction::Transaction;

pub(super) const PROGRAM_ID: &str = "4ibrEMW5F6hKnkW4jVedswYv6H6VtwPN6ar6dvXDN1nT";
pub(super) const TOKEN_PROGRAM_ID: Pubkey = spl_token::ID;
pub(super) const ASSOCIATED_TOKEN_PROGRAM_ID: &str = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL";

pub(super) const INITIAL_MAKER_A: u64 = 1_000_000_000;
pub(super) const AMOUNT_TO_RECEIVE: u64 = 100_000_000;
pub(super) const AMOUNT_TO_GIVE: u64 = 500_000_000;

pub(super) struct MakeSetup {
    pub svm: LiteSVM,
    pub maker: Keypair,
    pub mint_a: Pubkey,
    pub mint_b: Pubkey,
    pub maker_ata_a: Pubkey,
    pub escrow: Pubkey,
    pub bump: u8,
    pub vault: Pubkey,
    pub amount_to_give: u64,
    pub amount_to_receive: u64,
}

pub(super) fn program_id() -> Pubkey {
    Pubkey::from(crate::ID)
}

pub(super) fn associated_token_program() -> Pubkey {
    ASSOCIATED_TOKEN_PROGRAM_ID.parse().unwrap()
}

pub(super) fn system_program() -> Pubkey {
    solana_sdk_ids::system_program::ID
}

pub(super) fn setup() -> (LiteSVM, Keypair) {
    let mut svm = LiteSVM::new();
    let payer = Keypair::new();

    // LiteSVM 0.9 still ships the pre-SIMD-0194 Rent sysvar. Match the live cluster.
    #[allow(deprecated)]
    svm.set_sysvar(&solana_rent::Rent {
        lamports_per_byte_year: 6960,
        exemption_threshold: 1.0,
        burn_percent: 50,
    });

    svm.airdrop(&payer.pubkey(), 10 * LAMPORTS_PER_SOL)
        .expect("Airdrop failed");

    let so_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/deploy/escrow.so");
    let program_data = std::fs::read(&so_path).unwrap_or_else(|e| {
        panic!(
            "Failed to read program SO file at {}: {e}. Run `cargo build-sbf` first.",
            so_path.display()
        )
    });

    svm.add_program(program_id(), &program_data)
        .expect("Failed to add program");

    (svm, payer)
}

pub(super) fn make_escrow() -> MakeSetup {
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

    let maker_ata_a = CreateAssociatedTokenAccount::new(&mut svm, &maker, &mint_a)
        .owner(&maker.pubkey())
        .send()
        .unwrap();

    let (escrow, bump) =
        Pubkey::find_program_address(&[b"escrow".as_ref(), maker.pubkey().as_ref()], &program_id);
    let vault = spl_associated_token_account::get_associated_token_address(&escrow, &mint_a);

    MintTo::new(&mut svm, &maker, &mint_a, &maker_ata_a, INITIAL_MAKER_A)
        .send()
        .unwrap();

    let make_ix = Instruction {
        program_id,
        accounts: vec![
            AccountMeta::new(maker.pubkey(), true),
            AccountMeta::new(mint_a, false),
            AccountMeta::new(mint_b, false),
            AccountMeta::new(escrow, false),
            AccountMeta::new(maker_ata_a, false),
            AccountMeta::new(vault, false),
            AccountMeta::new(system_program(), false),
            AccountMeta::new(TOKEN_PROGRAM_ID, false),
            AccountMeta::new(associated_token_program(), false),
        ],
        data: [
            vec![0u8],
            AMOUNT_TO_RECEIVE.to_le_bytes().to_vec(),
            AMOUNT_TO_GIVE.to_le_bytes().to_vec(),
        ]
        .concat(),
    };

    let message = Message::new(&[make_ix], Some(&maker.pubkey()));
    let tx = Transaction::new(&[&maker], message, svm.latest_blockhash());
    let result = svm.send_transaction(tx).unwrap();
    println!("Make CUs Consumed: {}", result.compute_units_consumed);

    MakeSetup {
        svm,
        maker,
        mint_a,
        mint_b,
        maker_ata_a,
        escrow,
        bump,
        vault,
        amount_to_give: AMOUNT_TO_GIVE,
        amount_to_receive: AMOUNT_TO_RECEIVE,
    }
}

pub(super) fn token_amount(svm: &LiteSVM, ata: &Pubkey) -> u64 {
    let acc = svm.get_account(ata).expect("token account missing");
    spl_token_2022::state::Account::unpack(&acc.data)
        .unwrap()
        .amount
}

pub(super) fn assert_closed(svm: &LiteSVM, address: &Pubkey, label: &str) {
    match svm.get_account(address) {
        None => {}
        Some(acc) => assert!(
            acc.lamports == 0,
            "{label} should be closed (0 lamports), got {}",
            acc.lamports
        ),
    }
}
