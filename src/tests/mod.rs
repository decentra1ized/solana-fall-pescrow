#[cfg(test)]
mod tests {

    use std::path::PathBuf;

    use litesvm::LiteSVM;
    use litesvm_token::{spl_token::{self}, CreateAssociatedTokenAccount, CreateMint, MintTo};

    use solana_instruction::{AccountMeta, Instruction};
    use solana_keypair::Keypair;
    use solana_message::Message;
    use solana_native_token::LAMPORTS_PER_SOL;
    use solana_pubkey::Pubkey;
    use solana_signer::Signer;
    use solana_transaction::Transaction;
    use solana_program_pack::Pack;

    const PROGRAM_ID: &str = "4ibrEMW5F6hKnkW4jVedswYv6H6VtwPN6ar6dvXDN1nT";
    const TOKEN_PROGRAM_ID: Pubkey = spl_token::ID;
    const ASSOCIATED_TOKEN_PROGRAM_ID: &str = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL";

    fn program_id() -> Pubkey {
        Pubkey::from(crate::ID)
    }

    fn setup() -> (LiteSVM, Keypair) {

        let mut svm = LiteSVM::new();
        let payer = Keypair::new();

        // LiteSVM 0.9 still ships the pre-SIMD-0194 Rent sysvar (3480 lamports/byte-year,
        // 2-year exemption threshold). Mainnet has activated SIMD-0194, which folds the
        // threshold into the rate (6960 lamports/byte, threshold 1.0), and pinocchio 0.11
        // computes rent exemption that way. Set the sysvar to match the live cluster.
        #[allow(deprecated)]
        svm.set_sysvar(&solana_rent::Rent {
            lamports_per_byte_year: 6960,
            exemption_threshold: 1.0,
            burn_percent: 50,
        });

        svm
            .airdrop(&payer.pubkey(), 10 * LAMPORTS_PER_SOL)
            .expect("Airdrop failed");

        // Load program SO file (produced by `cargo build-sbf`)
        let so_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/deploy/escrow.so");

        let program_data = std::fs::read(&so_path)
            .unwrap_or_else(|e| panic!("Failed to read program SO file at {}: {e}. Run `cargo build-sbf` first.", so_path.display()));

        svm.add_program(program_id(), &program_data).expect("Failed to add program");

        (svm, payer)

    }

    /// Shared setup for every test past Make: two mints, a funded maker ATA
    /// for A, and a sent Make transaction. Returns everything the next steps
    /// need to build Take or Cancel.
    struct MadeEscrow {
        svm: LiteSVM,
        maker: Keypair,
        mint_a: Pubkey,
        mint_b: Pubkey,
        escrow: Pubkey,
        bump: u8,
        vault: Pubkey,
        maker_ata_a: Pubkey,
        amount_to_receive: u64,
        amount_to_give: u64,
    }

    fn make_escrow() -> MadeEscrow {
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
            .owner(&maker.pubkey()).send().unwrap();

        let escrow = Pubkey::find_program_address(
            &[b"escrow".as_ref(), maker.pubkey().as_ref()],
            &PROGRAM_ID.parse().unwrap(),
        );

        let vault = spl_associated_token_account::get_associated_token_address(
            &escrow.0,
            &mint_a,
        );

        let associated_token_program = ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();
        let token_program = TOKEN_PROGRAM_ID;
        let system_program = solana_sdk_ids::system_program::ID;

        MintTo::new(&mut svm, &maker, &mint_a, &maker_ata_a, 1000000000)
            .send()
            .unwrap();

        let amount_to_receive: u64 = 100000000;
        let amount_to_give: u64 = 500000000;
        let bump: u8 = escrow.1;

        let make_data = [
            vec![0u8],
            amount_to_receive.to_le_bytes().to_vec(),
            amount_to_give.to_le_bytes().to_vec(),
        ].concat();
        let make_ix = Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new(maker.pubkey(), true),
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

        let message = Message::new(&[make_ix], Some(&maker.pubkey()));
        let recent_blockhash = svm.latest_blockhash();
        let transaction = Transaction::new(&[&maker], message, recent_blockhash);
        svm.send_transaction(transaction).expect("make should succeed");

        MadeEscrow {
            svm,
            maker,
            mint_a,
            mint_b,
            escrow: escrow.0,
            bump,
            vault,
            maker_ata_a,
            amount_to_receive,
            amount_to_give,
        }
    }

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

        let maker_ata_a = CreateAssociatedTokenAccount::new(&mut svm, &payer, &mint_a)
            .owner(&payer.pubkey()).send().unwrap();
        println!("Maker ATA A: {}\n", maker_ata_a);

        let escrow = Pubkey::find_program_address(
            &[b"escrow".as_ref(), payer.pubkey().as_ref()],
            &PROGRAM_ID.parse().unwrap(),
        );
        println!("Escrow PDA: {}\n", escrow.0);

        let vault = spl_associated_token_account::get_associated_token_address(
            &escrow.0,
            &mint_a
        );
        println!("Vault PDA: {}\n", vault);

        let associated_token_program = ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();
        let token_program = TOKEN_PROGRAM_ID;
        let system_program = solana_sdk_ids::system_program::ID;

        MintTo::new(&mut svm, &payer, &mint_a, &maker_ata_a, 1000000000)
            .send()
            .unwrap();

        let amount_to_receive: u64 = 100000000;
        let amount_to_give: u64 = 500000000;
        let bump: u8 = escrow.1;

        println!("Bump: {}", bump);

        let make_data = [
            vec![0u8],
            amount_to_receive.to_le_bytes().to_vec(),
            amount_to_give.to_le_bytes().to_vec(),
        ].concat();
        let make_ix = Instruction {
            program_id: program_id,
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

        let message = Message::new(&[make_ix], Some(&payer.pubkey()));
        let recent_blockhash = svm.latest_blockhash();

        let transaction = Transaction::new(&[&payer], message, recent_blockhash);

        let tx = svm.send_transaction(transaction).unwrap();

        println!("\n\nMake transaction successful");
        println!("CUs Consumed: {}", tx.compute_units_consumed);

        let vault_acc = svm.get_account(&vault).unwrap();
        let vault_state = spl_token_2022::state::Account::unpack(&vault_acc.data).unwrap();
        println!("Vault owner: {} (escrow PDA? {})", vault_state.owner, vault_state.owner == escrow.0);
        println!("Vault balance: {}", vault_state.amount);
        assert_eq!(vault_state.amount, amount_to_give);

        let maker_acc = svm.get_account(&maker_ata_a).unwrap();
        let maker_state = spl_token_2022::state::Account::unpack(&maker_acc.data).unwrap();
        println!("Maker ATA balance: {}", maker_state.amount);
        assert_eq!(maker_state.amount, 1000000000 - amount_to_give);

        let esc = svm.get_account(&escrow.0).unwrap();
        println!("Escrow account owner: {} (program? {})", esc.owner, esc.owner == program_id);
        println!("Escrow data len: {}", esc.data.len());
        let d = &esc.data;
        println!("  maker   = {}", Pubkey::new_from_array(d[0..32].try_into().unwrap()));
        println!("  mint_a  = {}", Pubkey::new_from_array(d[32..64].try_into().unwrap()));
        println!("  mint_b  = {}", Pubkey::new_from_array(d[64..96].try_into().unwrap()));
        println!("  receive = {}", u64::from_le_bytes(d[96..104].try_into().unwrap()));
        println!("  give    = {}", u64::from_le_bytes(d[104..112].try_into().unwrap()));
        println!("  bump    = {}", d[112]);
        assert_eq!(&d[0..32], payer.pubkey().as_ref());
        assert_eq!(u64::from_le_bytes(d[96..104].try_into().unwrap()), amount_to_receive);
        assert_eq!(u64::from_le_bytes(d[104..112].try_into().unwrap()), amount_to_give);
        assert_eq!(d[112], bump);
    }

    #[test]
    pub fn test_take_instruction() {
        let MadeEscrow {
            mut svm, maker, mint_a, mint_b, escrow, vault, maker_ata_a: _,
            amount_to_receive, amount_to_give, ..
        } = make_escrow();

        let program_id = program_id();
        let token_program = TOKEN_PROGRAM_ID;
        let associated_token_program = ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();
        let system_program = solana_sdk_ids::system_program::ID;

        let taker = Keypair::new();
        svm.airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut svm, &taker, &mint_b)
            .owner(&taker.pubkey()).send().unwrap();
        MintTo::new(&mut svm, &maker, &mint_b, &taker_ata_b, 100000000)
            .send()
            .unwrap();

        let taker_ata_a = spl_associated_token_account::get_associated_token_address(
            &taker.pubkey(), &mint_a,
        );
        let maker_ata_b = spl_associated_token_account::get_associated_token_address(
            &maker.pubkey(), &mint_b,
        );

        let maker_sol_before = svm.get_balance(&maker.pubkey()).unwrap();

        let take_data = vec![1u8];
        let take_ix = Instruction {
            program_id,
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
                AccountMeta::new(system_program, false),
                AccountMeta::new(token_program, false),
                AccountMeta::new(associated_token_program, false),
            ],
            data: take_data,
        };

        let message = Message::new(&[take_ix], Some(&taker.pubkey()));
        let recent_blockhash = svm.latest_blockhash();
        let transaction = Transaction::new(&[&taker], message, recent_blockhash);
        let tx = svm.send_transaction(transaction).expect("take should succeed");

        println!("\n\nTake transaction successful");
        println!("CUs Consumed: {}", tx.compute_units_consumed);

        let taker_ata_a_acc = svm.get_account(&taker_ata_a).unwrap();
        let taker_ata_a_state = spl_token_2022::state::Account::unpack(&taker_ata_a_acc.data).unwrap();
        assert_eq!(taker_ata_a_state.amount, amount_to_give);

        let maker_ata_b_acc = svm.get_account(&maker_ata_b).unwrap();
        let maker_ata_b_state = spl_token_2022::state::Account::unpack(&maker_ata_b_acc.data).unwrap();
        assert_eq!(maker_ata_b_state.amount, amount_to_receive);

        assert!(
            svm.get_account(&vault).map_or(true, |a| a.lamports == 0),
            "vault should be closed"
        );
        assert!(
            svm.get_account(&escrow).map_or(true, |a| a.lamports == 0),
            "escrow should be closed"
        );

        let maker_sol_after = svm.get_balance(&maker.pubkey()).unwrap();
        assert!(
            maker_sol_after > maker_sol_before,
            "maker should have received the rent refund from the closed accounts"
        );
    }

    #[test]
    pub fn test_cancel_instruction() {
        let MadeEscrow {
            mut svm, maker, mint_a, mint_b: _, escrow, vault, maker_ata_a,
            amount_to_receive: _, amount_to_give, ..
        } = make_escrow();

        let program_id = program_id();
        let token_program = TOKEN_PROGRAM_ID;

        let cancel_data = vec![2u8];
        let cancel_ix = Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new(maker.pubkey(), true),
                AccountMeta::new(mint_a, false),
                AccountMeta::new(escrow, false),
                AccountMeta::new(vault, false),
                AccountMeta::new(maker_ata_a, false),
                AccountMeta::new(token_program, false),
            ],
            data: cancel_data,
        };

        let message = Message::new(&[cancel_ix], Some(&maker.pubkey()));
        let recent_blockhash = svm.latest_blockhash();
        let transaction = Transaction::new(&[&maker], message, recent_blockhash);
        let tx = svm.send_transaction(transaction).expect("cancel should succeed");

        println!("\n\nCancel transaction successful");
        println!("CUs Consumed: {}", tx.compute_units_consumed);

        let maker_ata_a_acc = svm.get_account(&maker_ata_a).unwrap();
        let maker_ata_a_state = spl_token_2022::state::Account::unpack(&maker_ata_a_acc.data).unwrap();
        assert_eq!(maker_ata_a_state.amount, 1000000000);
        let _ = amount_to_give;

        assert!(
            svm.get_account(&vault).map_or(true, |a| a.lamports == 0),
            "vault should be closed"
        );
        assert!(
            svm.get_account(&escrow).map_or(true, |a| a.lamports == 0),
            "escrow should be closed"
        );
    }

    #[test]
    pub fn test_take_with_underfunded_taker_fails() {
        let MadeEscrow {
            mut svm, maker, mint_a, mint_b, escrow, vault, maker_ata_a: _,
            amount_to_receive: _, amount_to_give: _, ..
        } = make_escrow();

        let program_id = program_id();
        let token_program = TOKEN_PROGRAM_ID;
        let associated_token_program = ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();
        let system_program = solana_sdk_ids::system_program::ID;

        let taker = Keypair::new();
        svm.airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

        // Only 50 B, less than the 100 B the escrow requires.
        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut svm, &taker, &mint_b)
            .owner(&taker.pubkey()).send().unwrap();
        MintTo::new(&mut svm, &maker, &mint_b, &taker_ata_b, 50000000)
            .send()
            .unwrap();

        let taker_ata_a = spl_associated_token_account::get_associated_token_address(
            &taker.pubkey(), &mint_a,
        );
        let maker_ata_b = spl_associated_token_account::get_associated_token_address(
            &maker.pubkey(), &mint_b,
        );

        let take_ix = Instruction {
            program_id,
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
                AccountMeta::new(system_program, false),
                AccountMeta::new(token_program, false),
                AccountMeta::new(associated_token_program, false),
            ],
            data: vec![1u8],
        };

        let message = Message::new(&[take_ix], Some(&taker.pubkey()));
        let recent_blockhash = svm.latest_blockhash();
        let transaction = Transaction::new(&[&taker], message, recent_blockhash);
        let res = svm.send_transaction(transaction);

        assert!(res.is_err(), "take with an underfunded taker must fail");

        // Nothing should have moved: the vault still holds its original balance.
        let vault_acc = svm.get_account(&vault).unwrap();
        let vault_state = spl_token_2022::state::Account::unpack(&vault_acc.data).unwrap();
        assert_eq!(vault_state.amount, 500000000, "vault must be untouched after a failed take");
    }

    #[test]
    pub fn test_cancel_by_stranger_fails() {
        let MadeEscrow {
            mut svm, maker: _, mint_a, mint_b: _, escrow, vault, maker_ata_a,
            amount_to_receive: _, amount_to_give, ..
        } = make_escrow();

        let program_id = program_id();
        let token_program = TOKEN_PROGRAM_ID;

        // A stranger, not the maker, tries to cancel.
        let stranger = Keypair::new();
        svm.airdrop(&stranger.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

        let cancel_ix = Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new(stranger.pubkey(), true),
                AccountMeta::new(mint_a, false),
                AccountMeta::new(escrow, false),
                AccountMeta::new(vault, false),
                AccountMeta::new(maker_ata_a, false),
                AccountMeta::new(token_program, false),
            ],
            data: vec![2u8],
        };

        let message = Message::new(&[cancel_ix], Some(&stranger.pubkey()));
        let recent_blockhash = svm.latest_blockhash();
        let transaction = Transaction::new(&[&stranger], message, recent_blockhash);
        let res = svm.send_transaction(transaction);

        assert!(res.is_err(), "cancel signed by a stranger must fail");

        // The 500 A must still be sitting in the vault, untouched.
        let vault_acc = svm.get_account(&vault).unwrap();
        let vault_state = spl_token_2022::state::Account::unpack(&vault_acc.data).unwrap();
        assert_eq!(vault_state.amount, amount_to_give, "vault must be untouched after a rejected cancel");
    }
}
