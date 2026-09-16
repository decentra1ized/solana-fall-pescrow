use solana_program_pack::Pack;
use solana_pubkey::Pubkey;
use solana_signer::Signer;

use super::setup::{
    make_escrow, program_id, token_amount, INITIAL_MAKER_A, PROGRAM_ID,
};

#[test]
fn test_make_instruction() {
    let s = make_escrow();
    let program_id = program_id();

    assert_eq!(program_id.to_string(), PROGRAM_ID);

    println!("Mint A: {}", s.mint_a);
    println!("Mint B: {}", s.mint_b);
    println!("Maker ATA A: {}\n", s.maker_ata_a);
    println!("Escrow PDA: {}\n", s.escrow);
    println!("Vault PDA: {}\n", s.vault);
    println!("Bump: {}", s.bump);

    let vault_acc = s.svm.get_account(&s.vault).unwrap();
    let vault_state = spl_token_2022::state::Account::unpack(&vault_acc.data).unwrap();
    println!(
        "Vault owner: {} (escrow PDA? {})",
        vault_state.owner,
        vault_state.owner == s.escrow
    );
    println!("Vault balance: {}", vault_state.amount);
    assert_eq!(vault_state.amount, s.amount_to_give);

    println!("Maker ATA balance: {}", token_amount(&s.svm, &s.maker_ata_a));
    assert_eq!(
        token_amount(&s.svm, &s.maker_ata_a),
        INITIAL_MAKER_A - s.amount_to_give
    );

    let esc = s.svm.get_account(&s.escrow).unwrap();
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
    assert_eq!(&d[0..32], s.maker.pubkey().as_ref());
    assert_eq!(
        u64::from_le_bytes(d[96..104].try_into().unwrap()),
        s.amount_to_receive
    );
    assert_eq!(
        u64::from_le_bytes(d[104..112].try_into().unwrap()),
        s.amount_to_give
    );
    assert_eq!(d[112], s.bump);
}
