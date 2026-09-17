#[cfg(test)]
mod tests {

    use std::path::PathBuf;

    use litesvm::LiteSVM;
    use litesvm::types::{FailedTransactionMetadata, TransactionMetadata};
    use litesvm_token::{spl_token::{self}, CreateAssociatedTokenAccount, CreateMint, MintTo};

    use solana_instruction::{AccountMeta, Instruction};
    use solana_keypair::Keypair;
    use solana_message::Message;
    use solana_native_token::LAMPORTS_PER_SOL;
    use solana_pubkey::Pubkey;
    use solana_signer::Signer;
    use solana_transaction::Transaction;
    use solana_program_pack::Pack;
    use solana_address::Address;
    use spl_associated_token_account::get_associated_token_address;

    const PROGRAM_ID: &str = "4ibrEMW5F6hKnkW4jVedswYv6H6VtwPN6ar6dvXDN1nT";
    const TOKEN_PROGRAM_ID: Pubkey = spl_token::ID;
    const ASSOCIATED_TOKEN_PROGRAM_ID: &str = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL";

    // 6 decimals everywhere
    const MAKER_INITIAL: u64 = 1_000_000_000;
    const AMOUNT_TO_RECEIVE: u64 = 100_000_000;
    const AMOUNT_TO_GIVE: u64 = 500_000_000;

    fn program_id() -> Pubkey {
        Pubkey::from(crate::ID)
    }

    fn setup() -> (LiteSVM, Keypair, Keypair) {
        let mut svm = LiteSVM::new();
        let maker = Keypair::new();
        let taker = Keypair::new();

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
            .airdrop(&maker.pubkey(), 10 * LAMPORTS_PER_SOL)
            .expect("Airdrop failed");
        svm
            .airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL)
            .expect("Airdrop failed");

        // Load program SO file (produced by `cargo build-sbf`)
        let so_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/deploy/escrow.so");

        let program_data = std::fs::read(&so_path)
            .unwrap_or_else(|e| panic!("Failed to read program SO file at {}: {e}. Run `cargo build-sbf` first.", so_path.display()));

        svm.add_program(program_id(), &program_data).expect("Failed to add program");

        (svm, maker, taker)
    }

    fn setup_mint(svm: &mut LiteSVM, payer: &Keypair) -> Address {
        CreateMint::new(svm, payer)
            .decimals(6)
            .authority(&payer.pubkey())
            .send()
            .unwrap()
    }

    struct MakeSetup {
        svm: LiteSVM,
        maker: Keypair,
        taker: Keypair,
        mint_a: Pubkey,
        mint_b: Pubkey,
        escrow: Pubkey,
        bump: u8,
        vault: Pubkey,
        maker_ata_a: Pubkey,
    }

    fn send_tx(
        svm: &mut LiteSVM,
        ix: Instruction,
        payer: &Keypair,
    ) -> Result<TransactionMetadata, FailedTransactionMetadata> {
        let message = Message::new(&[ix], Some(&payer.pubkey()));
        let blockhash = svm.latest_blockhash();
        svm.send_transaction(Transaction::new(&[payer], message, blockhash))
    }

    fn token_balance(svm: &LiteSVM, ata: &Pubkey) -> u64 {
        let acc = svm.get_account(ata).expect("token account does not exist");
        spl_token_2022::state::Account::unpack(&acc.data).unwrap().amount
    }

    fn sol_balance(svm: &LiteSVM, key: &Pubkey) -> u64 {
        svm.get_balance(key).unwrap_or(0)
    }

    fn assert_closed(svm: &LiteSVM, address: &Pubkey) {
        if let Some(acc) = svm.get_account(address) {
            assert_eq!(acc.lamports, 0);
            assert!(acc.data.is_empty());
            assert_eq!(acc.owner, solana_sdk_ids::system_program::ID);
        }
    }

    // Mint B to the taker so they can pay. The maker is mint B's authority.
    fn fund_taker_with_b(svm: &mut LiteSVM, maker: &Keypair, taker: &Keypair, mint_b: &Pubkey, amount: u64) -> Pubkey {
        let taker_ata_b = CreateAssociatedTokenAccount::new(svm, taker, mint_b)
            .owner(&taker.pubkey()).send().unwrap();
        MintTo::new(svm, maker, mint_b, &taker_ata_b, amount).send().unwrap();
        taker_ata_b
    }

    // Account order mirrors take.rs
    fn take_ix(s: &MakeSetup, taker_ata_a: Pubkey, taker_ata_b: Pubkey, maker_ata_b: Pubkey) -> Instruction {
        Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(s.taker.pubkey(), true),
                AccountMeta::new(s.maker.pubkey(), false),
                AccountMeta::new_readonly(s.mint_a, false),
                AccountMeta::new_readonly(s.mint_b, false),
                AccountMeta::new(s.escrow, false),
                AccountMeta::new(s.vault, false),
                AccountMeta::new(taker_ata_a, false),
                AccountMeta::new(taker_ata_b, false),
                AccountMeta::new(maker_ata_b, false),
                AccountMeta::new_readonly(solana_sdk_ids::system_program::ID, false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
                AccountMeta::new_readonly(ASSOCIATED_TOKEN_PROGRAM_ID.parse().unwrap(), false),
            ],
            data: vec![1u8],
        }
    }

    // Account order mirrors cancel.rs
    fn cancel_ix(s: &MakeSetup, maker: &Pubkey) -> Instruction {
        Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(*maker, true),
                AccountMeta::new_readonly(s.mint_a, false),
                AccountMeta::new(s.escrow, false),
                AccountMeta::new(s.vault, false),
                AccountMeta::new(s.maker_ata_a, false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
            ],
            data: vec![2u8],
        }
    }

    fn setup_make() -> MakeSetup {
        let (mut svm, maker, taker) = setup();

        let program_id = program_id();
        assert_eq!(program_id.to_string(), PROGRAM_ID);

        let mint_a = setup_mint(&mut svm, &maker);
        let mint_b = setup_mint(&mut svm, &maker);
        println!("Mint A: {}", mint_a);
        println!("Mint B: {}", mint_b);

        let maker_ata_a = CreateAssociatedTokenAccount::new(&mut svm, &maker, &mint_a)
            .owner(&maker.pubkey()).send().unwrap();
        MintTo::new(&mut svm, &maker, &mint_a, &maker_ata_a, MAKER_INITIAL)
            .send()
            .unwrap();
        println!("Maker ATA A: {}", maker_ata_a);

        let (escrow, bump) = Pubkey::find_program_address(
            &[b"escrow".as_ref(), maker.pubkey().as_ref()],
            &program_id,
        );
        let vault = get_associated_token_address(&escrow, &mint_a);
        println!("Escrow PDA: {} (bump {})", escrow, bump);
        println!("Vault: {}\n", vault);

        let make_data = [
            vec![0u8],
            AMOUNT_TO_RECEIVE.to_le_bytes().to_vec(),
            AMOUNT_TO_GIVE.to_le_bytes().to_vec(),
        ].concat();
        let make_ix = Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new(maker.pubkey(), true),
                AccountMeta::new_readonly(mint_a, false),
                AccountMeta::new_readonly(mint_b, false),
                AccountMeta::new(escrow, false),
                AccountMeta::new(maker_ata_a, false),
                AccountMeta::new(vault, false),
                AccountMeta::new_readonly(solana_sdk_ids::system_program::ID, false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
                AccountMeta::new_readonly(ASSOCIATED_TOKEN_PROGRAM_ID.parse().unwrap(), false),
            ],
            data: make_data,
        };

        let tx = send_tx(&mut svm, make_ix, &maker).unwrap();
        println!("Make transaction successful");
        println!("CUs Consumed: {}", tx.compute_units_consumed);

        MakeSetup { svm, maker, taker, mint_a, mint_b, escrow, bump, vault, maker_ata_a }
    }

    #[test]
    fn test_make_instruction() {
        let MakeSetup { svm, maker, mint_a, mint_b, escrow, bump, vault, maker_ata_a, .. } = setup_make();

        let vault_acc = svm.get_account(&vault).unwrap();
        let vault_state = spl_token_2022::state::Account::unpack(&vault_acc.data).unwrap();
        assert_eq!(vault_state.owner, escrow);
        assert_eq!(vault_state.amount, AMOUNT_TO_GIVE);
        assert_eq!(token_balance(&svm, &maker_ata_a), MAKER_INITIAL - AMOUNT_TO_GIVE);

        let esc = svm.get_account(&escrow).unwrap();
        assert_eq!(esc.owner, program_id());
        assert_eq!(esc.data.len(), 113);
        let d = &esc.data;
        assert_eq!(&d[0..32], maker.pubkey().as_ref());
        assert_eq!(&d[32..64], mint_a.as_ref());
        assert_eq!(&d[64..96], mint_b.as_ref());
        assert_eq!(u64::from_le_bytes(d[96..104].try_into().unwrap()), AMOUNT_TO_RECEIVE);
        assert_eq!(u64::from_le_bytes(d[104..112].try_into().unwrap()), AMOUNT_TO_GIVE);
        assert_eq!(d[112], bump);
    }

    #[test]
    fn test_take_instruction() {
        let mut s = setup_make();

        let taker_ata_b = fund_taker_with_b(&mut s.svm, &s.maker, &s.taker, &s.mint_b, AMOUNT_TO_RECEIVE);
        let taker_ata_a = get_associated_token_address(&s.taker.pubkey(), &s.mint_a);
        let maker_ata_b = get_associated_token_address(&s.maker.pubkey(), &s.mint_b);

        let maker_sol_before = sol_balance(&s.svm, &s.maker.pubkey());
        let rent_refund = s.svm.get_account(&s.vault).unwrap().lamports
            + s.svm.get_account(&s.escrow).unwrap().lamports;

        let ix = take_ix(&s, taker_ata_a, taker_ata_b, maker_ata_b);
        let tx = send_tx(&mut s.svm, ix, &s.taker).unwrap();
        println!("Take transaction successful");
        println!("CUs Consumed: {}", tx.compute_units_consumed);

        assert_eq!(token_balance(&s.svm, &taker_ata_a), AMOUNT_TO_GIVE);
        assert_eq!(token_balance(&s.svm, &taker_ata_b), 0);
        assert_eq!(token_balance(&s.svm, &maker_ata_b), AMOUNT_TO_RECEIVE);
        assert_closed(&s.svm, &s.vault);
        assert_closed(&s.svm, &s.escrow);
        // maker is not the fee payer here, so the delta is exactly the two rents
        assert_eq!(sol_balance(&s.svm, &s.maker.pubkey()), maker_sol_before + rent_refund);
    }

    #[test]
    fn test_cancel_instruction() {
        let mut s = setup_make();

        let maker_sol_before = sol_balance(&s.svm, &s.maker.pubkey());
        let rent_refund = s.svm.get_account(&s.vault).unwrap().lamports
            + s.svm.get_account(&s.escrow).unwrap().lamports;

        let ix = cancel_ix(&s, &s.maker.pubkey());
        let tx = send_tx(&mut s.svm, ix, &s.maker).unwrap();
        println!("Cancel transaction successful");
        println!("CUs Consumed: {}", tx.compute_units_consumed);

        assert_eq!(token_balance(&s.svm, &s.maker_ata_a), MAKER_INITIAL);
        assert_closed(&s.svm, &s.vault);
        assert_closed(&s.svm, &s.escrow);
        assert_eq!(sol_balance(&s.svm, &s.maker.pubkey()), maker_sol_before + rent_refund - tx.fee);
    }

    #[test]
    fn test_take_fails_when_taker_underfunded() {
        let mut s = setup_make();

        let taker_ata_b = fund_taker_with_b(&mut s.svm, &s.maker, &s.taker, &s.mint_b, AMOUNT_TO_RECEIVE / 2);
        let taker_ata_a = get_associated_token_address(&s.taker.pubkey(), &s.mint_a);
        let maker_ata_b = get_associated_token_address(&s.maker.pubkey(), &s.mint_b);

        let ix = take_ix(&s, taker_ata_a, taker_ata_b, maker_ata_b);
        let result = send_tx(&mut s.svm, ix, &s.taker);
        assert!(result.is_err(), "taker with 50 B must not be able to take a 100 B deal");

        assert_eq!(token_balance(&s.svm, &s.vault), AMOUNT_TO_GIVE);
        assert_eq!(token_balance(&s.svm, &taker_ata_b), AMOUNT_TO_RECEIVE / 2);
        assert!(s.svm.get_account(&s.escrow).is_some());
    }

    #[test]
    fn test_cancel_fails_for_stranger() {
        let mut s = setup_make();

        // stranger passes themselves as the maker
        let ix = cancel_ix(&s, &s.taker.pubkey());
        let result = send_tx(&mut s.svm, ix, &s.taker);
        assert!(result.is_err(), "a stranger must not be able to cancel");

        assert_eq!(token_balance(&s.svm, &s.vault), AMOUNT_TO_GIVE);
        assert!(s.svm.get_account(&s.escrow).is_some());
    }
}
