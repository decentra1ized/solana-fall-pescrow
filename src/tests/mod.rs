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

    /// Everything a Take or Cancel test needs after a successful Make.
    struct MakeSetup {
        svm: LiteSVM,
        maker: Keypair,
        mint_a: Pubkey,
        mint_b: Pubkey,
        maker_ata_a: Pubkey,
        escrow: Pubkey,
        bump: u8,
        vault: Pubkey,
        amount_to_receive: u64,
        amount_to_give: u64,
    }

    /// Runs the full Make flow (two mints, a funded maker ATA, the Make
    /// instruction) and returns everything Take/Cancel tests build on.
    /// Reuses setup() so the Rent sysvar override is never lost.
    fn make_escrow() -> MakeSetup {
        let (mut svm, payer) = setup();
        let prog_id = program_id();

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
            .owner(&payer.pubkey()).send().unwrap();

        let escrow = Pubkey::find_program_address(
            &[b"escrow".as_ref(), payer.pubkey().as_ref()],
            &PROGRAM_ID.parse().unwrap(),
        );

        let vault = spl_associated_token_account::get_associated_token_address(
            &escrow.0,
            &mint_a,
        );

        let associated_token_program = ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();
        let token_program = TOKEN_PROGRAM_ID;
        let system_program = solana_sdk_ids::system_program::ID;

        MintTo::new(&mut svm, &payer, &mint_a, &maker_ata_a, 1_000_000_000)
            .send()
            .unwrap();

        let amount_to_receive: u64 = 100_000_000;
        let amount_to_give: u64 = 500_000_000;
        let bump: u8 = escrow.1;

        let make_data = [
            vec![0u8],
            amount_to_receive.to_le_bytes().to_vec(),
            amount_to_give.to_le_bytes().to_vec(),
        ].concat();

        let make_ix = Instruction {
            program_id: prog_id,
            accounts: vec![
                AccountMeta::new(payer.pubkey(), true),
                AccountMeta::new_readonly(mint_a, false),
                AccountMeta::new_readonly(mint_b, false),
                AccountMeta::new(escrow.0, false),
                AccountMeta::new(maker_ata_a, false),
                AccountMeta::new(vault, false),
                AccountMeta::new_readonly(system_program, false),
                AccountMeta::new_readonly(token_program, false),
                AccountMeta::new_readonly(associated_token_program, false),
            ],
            data: make_data,
        };

        let message = Message::new(&[make_ix], Some(&payer.pubkey()));
        let recent_blockhash = svm.latest_blockhash();
        let transaction = Transaction::new(&[&payer], message, recent_blockhash);

        let tx = svm.send_transaction(transaction).unwrap();
        println!("\nMake transaction successful");
        println!("CUs Consumed: {}", tx.compute_units_consumed);

        MakeSetup {
            svm,
            maker: payer,
            mint_a,
            mint_b,
            maker_ata_a,
            escrow: escrow.0,
            bump,
            vault,
            amount_to_receive,
            amount_to_give,
        }
    }

    /// True if the account is gone, or is present but zeroed out (0 lamports,
    /// owned by the System Program) — both count as "closed" in LiteSVM.
    fn assert_closed(svm: &LiteSVM, pubkey: &Pubkey) {
        match svm.get_account(pubkey) {
            None => {}
            Some(acc) => {
                assert_eq!(acc.lamports, 0, "{pubkey} should have 0 lamports after close");
                assert_eq!(acc.owner, solana_sdk_ids::system_program::ID, "{pubkey} should be owned by the System Program after close");
            }
        }
    }

    #[test]
    pub fn test_make_instruction() {
        let s = make_escrow();

        assert_eq!(program_id().to_string(), PROGRAM_ID);

        let vault_acc = s.svm.get_account(&s.vault).unwrap();
        let vault_state = spl_token_2022::state::Account::unpack(&vault_acc.data).unwrap();
        assert_eq!(vault_state.owner, s.escrow);
        assert_eq!(vault_state.amount, s.amount_to_give);

        let maker_acc = s.svm.get_account(&s.maker_ata_a).unwrap();
        let maker_state = spl_token_2022::state::Account::unpack(&maker_acc.data).unwrap();
        assert_eq!(maker_state.amount, 1_000_000_000 - s.amount_to_give);

        let esc = s.svm.get_account(&s.escrow).unwrap();
        assert_eq!(esc.owner, program_id());
        let d = &esc.data;
        assert_eq!(&d[0..32], s.maker.pubkey().as_ref());
        assert_eq!(u64::from_le_bytes(d[96..104].try_into().unwrap()), s.amount_to_receive);
        assert_eq!(u64::from_le_bytes(d[104..112].try_into().unwrap()), s.amount_to_give);
        assert_eq!(d[112], s.bump);
    }

    #[test]
    pub fn test_take_instruction() {
        let mut s = make_escrow();

        let taker = Keypair::new();
        s.svm.airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut s.svm, &taker, &s.mint_b)
            .owner(&taker.pubkey()).send().unwrap();
        MintTo::new(&mut s.svm, &s.maker, &s.mint_b, &taker_ata_b, 100_000_000)
            .send()
            .unwrap();

        // Not created yet — the program creates these via CreateIdempotent.
        let taker_ata_a = spl_associated_token_account::get_associated_token_address(&taker.pubkey(), &s.mint_a);
        let maker_ata_b = spl_associated_token_account::get_associated_token_address(&s.maker.pubkey(), &s.mint_b);

        let associated_token_program = ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();
        let token_program = TOKEN_PROGRAM_ID;
        let system_program = solana_sdk_ids::system_program::ID;

        let take_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(taker.pubkey(), true),
                AccountMeta::new(s.maker.pubkey(), false),
                AccountMeta::new_readonly(s.mint_a, false),
                AccountMeta::new_readonly(s.mint_b, false),
                AccountMeta::new(s.escrow, false),
                AccountMeta::new(s.vault, false),
                AccountMeta::new(taker_ata_a, false),
                AccountMeta::new(taker_ata_b, false),
                AccountMeta::new(maker_ata_b, false),
                AccountMeta::new_readonly(system_program, false),
                AccountMeta::new_readonly(token_program, false),
                AccountMeta::new_readonly(associated_token_program, false),
            ],
            data: vec![1u8],
        };

        let message = Message::new(&[take_ix], Some(&taker.pubkey()));
        let recent_blockhash = s.svm.latest_blockhash();
        let transaction = Transaction::new(&[&taker], message, recent_blockhash);

        let maker_lamports_before = s.svm.get_account(&s.maker.pubkey()).unwrap().lamports;

        let tx = s.svm.send_transaction(transaction).unwrap();
        println!("\nTake transaction successful");
        println!("CUs Consumed: {}", tx.compute_units_consumed);

        let taker_ata_a_acc = s.svm.get_account(&taker_ata_a).unwrap();
        let taker_ata_a_state = spl_token_2022::state::Account::unpack(&taker_ata_a_acc.data).unwrap();
        assert_eq!(taker_ata_a_state.amount, s.amount_to_give, "taker should receive all 500 A");

        let maker_ata_b_acc = s.svm.get_account(&maker_ata_b).unwrap();
        let maker_ata_b_state = spl_token_2022::state::Account::unpack(&maker_ata_b_acc.data).unwrap();
        assert_eq!(maker_ata_b_state.amount, s.amount_to_receive, "maker should receive 100 B");

        assert_closed(&s.svm, &s.vault);
        assert_closed(&s.svm, &s.escrow);

        let maker_lamports_after = s.svm.get_account(&s.maker.pubkey()).unwrap().lamports;
        assert!(
            maker_lamports_after > maker_lamports_before,
            "maker should receive rent refunds from the closed vault and escrow"
        );
    }

    #[test]
    pub fn test_cancel_instruction() {
        let mut s = make_escrow();

        let token_program = TOKEN_PROGRAM_ID;

        let cancel_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(s.maker.pubkey(), true),
                AccountMeta::new_readonly(s.mint_a, false),
                AccountMeta::new(s.escrow, false),
                AccountMeta::new(s.vault, false),
                AccountMeta::new(s.maker_ata_a, false),
                AccountMeta::new_readonly(token_program, false),
            ],
            data: vec![2u8],
        };

        let message = Message::new(&[cancel_ix], Some(&s.maker.pubkey()));
        let recent_blockhash = s.svm.latest_blockhash();
        let transaction = Transaction::new(&[&s.maker], message, recent_blockhash);

        let tx = s.svm.send_transaction(transaction).unwrap();
        println!("\nCancel transaction successful");
        println!("CUs Consumed: {}", tx.compute_units_consumed);

        let maker_ata_a_acc = s.svm.get_account(&s.maker_ata_a).unwrap();
        let maker_ata_a_state = spl_token_2022::state::Account::unpack(&maker_ata_a_acc.data).unwrap();
        assert_eq!(maker_ata_a_state.amount, 1_000_000_000, "maker should have all 1000 A back");

        assert_closed(&s.svm, &s.vault);
        assert_closed(&s.svm, &s.escrow);
    }

    #[test]
    pub fn test_take_fails_with_insufficient_funds() {
        let mut s = make_escrow();

        let taker = Keypair::new();
        s.svm.airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut s.svm, &taker, &s.mint_b)
            .owner(&taker.pubkey()).send().unwrap();
        // Only 50 B — the escrow wants 100 B. The token program should reject
        // the transfer, and nothing else should move.
        MintTo::new(&mut s.svm, &s.maker, &s.mint_b, &taker_ata_b, 50_000_000)
            .send()
            .unwrap();

        let taker_ata_a = spl_associated_token_account::get_associated_token_address(&taker.pubkey(), &s.mint_a);
        let maker_ata_b = spl_associated_token_account::get_associated_token_address(&s.maker.pubkey(), &s.mint_b);

        let associated_token_program = ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();
        let token_program = TOKEN_PROGRAM_ID;
        let system_program = solana_sdk_ids::system_program::ID;

        let take_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(taker.pubkey(), true),
                AccountMeta::new(s.maker.pubkey(), false),
                AccountMeta::new_readonly(s.mint_a, false),
                AccountMeta::new_readonly(s.mint_b, false),
                AccountMeta::new(s.escrow, false),
                AccountMeta::new(s.vault, false),
                AccountMeta::new(taker_ata_a, false),
                AccountMeta::new(taker_ata_b, false),
                AccountMeta::new(maker_ata_b, false),
                AccountMeta::new_readonly(system_program, false),
                AccountMeta::new_readonly(token_program, false),
                AccountMeta::new_readonly(associated_token_program, false),
            ],
            data: vec![1u8],
        };

        let message = Message::new(&[take_ix], Some(&taker.pubkey()));
        let recent_blockhash = s.svm.latest_blockhash();
        let transaction = Transaction::new(&[&taker], message, recent_blockhash);

        let result = s.svm.send_transaction(transaction);
        assert!(result.is_err(), "an underfunded taker must not be able to complete a Take");

        // Confirm nothing moved: the vault still holds the full 500 A.
        let vault_acc = s.svm.get_account(&s.vault).unwrap();
        let vault_state = spl_token_2022::state::Account::unpack(&vault_acc.data).unwrap();
        assert_eq!(vault_state.amount, s.amount_to_give);
    }

    #[test]
    pub fn test_cancel_fails_for_stranger() {
        let mut s = make_escrow();

        let stranger = Keypair::new();
        s.svm.airdrop(&stranger.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

        let token_program = TOKEN_PROGRAM_ID;

        // The stranger signs, and puts themselves in the maker slot — but
        // escrow.maker() on-chain still points at the real maker. The
        // cross-check in cancel.rs step 3 must reject this before any
        // tokens move.
        let cancel_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(stranger.pubkey(), true),
                AccountMeta::new_readonly(s.mint_a, false),
                AccountMeta::new(s.escrow, false),
                AccountMeta::new(s.vault, false),
                AccountMeta::new(s.maker_ata_a, false),
                AccountMeta::new_readonly(token_program, false),
            ],
            data: vec![2u8],
        };

        let message = Message::new(&[cancel_ix], Some(&stranger.pubkey()));
        let recent_blockhash = s.svm.latest_blockhash();
        let transaction = Transaction::new(&[&stranger], message, recent_blockhash);

        let result = s.svm.send_transaction(transaction);
        assert!(result.is_err(), "a stranger must not be able to cancel someone else's escrow");

        let vault_acc = s.svm.get_account(&s.vault).unwrap();
        let vault_state = spl_token_2022::state::Account::unpack(&vault_acc.data).unwrap();
        assert_eq!(vault_state.amount, s.amount_to_give, "the 500 A must still be in the vault");
    }
}
