#[cfg(test)]
mod tests {

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

        svm.airdrop(&payer.pubkey(), 10 * LAMPORTS_PER_SOL)
            .expect("Airdrop failed");

        // Load program SO file (produced by `cargo build-sbf`)
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

    fn assert_account_closed(svm: &LiteSVM, address: &Pubkey) {
        match svm.get_account(address) {
            None => {}
            Some(account) => {
                assert_eq!(account.lamports, 0);
                assert_eq!(account.owner, solana_sdk_ids::system_program::ID);
            }
        }
    }

    fn make_setup() -> (
        LiteSVM,
        Keypair,
        Pubkey,
        Pubkey,
        Pubkey,
        u8,
        Pubkey,
        Pubkey,
        u64,
        u64,
    ) {
        let (mut svm, payer) = setup();

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

        let escrow = Pubkey::find_program_address(
            &[b"escrow".as_ref(), payer.pubkey().as_ref()],
            &program_id(),
        );
        let vault = spl_associated_token_account::get_associated_token_address(&escrow.0, &mint_a);

        MintTo::new(&mut svm, &payer, &mint_a, &maker_ata_a, 1_000_000_000)
            .send()
            .unwrap();

        let amount_to_receive = 100_000_000u64;
        let amount_to_give = 500_000_000u64;

        let make_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(payer.pubkey(), true),
                AccountMeta::new(mint_a, false),
                AccountMeta::new(mint_b, false),
                AccountMeta::new(escrow.0, false),
                AccountMeta::new(maker_ata_a, false),
                AccountMeta::new(vault, false),
                AccountMeta::new(solana_sdk_ids::system_program::ID, false),
                AccountMeta::new(TOKEN_PROGRAM_ID, false),
                AccountMeta::new(
                    ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap(),
                    false,
                ),
            ],
            data: [
                vec![0u8],
                amount_to_receive.to_le_bytes().to_vec(),
                amount_to_give.to_le_bytes().to_vec(),
            ]
            .concat(),
        };

        let tx = Transaction::new(
            &[&payer],
            Message::new(&[make_ix], Some(&payer.pubkey())),
            svm.latest_blockhash(),
        );
        svm.send_transaction(tx).unwrap();

        (
            svm,
            payer,
            mint_a,
            mint_b,
            escrow.0,
            escrow.1,
            vault,
            maker_ata_a,
            amount_to_receive,
            amount_to_give,
        )
    }

    #[test]
    pub fn test_make_instruction() {
        let (
            svm,
            payer,
            mint_a,
            mint_b,
            escrow,
            bump,
            vault,
            maker_ata_a,
            amount_to_receive,
            amount_to_give,
        ) = make_setup();

        let vault_acc = svm.get_account(&vault).unwrap();
        let vault_state = spl_token_2022::state::Account::unpack(&vault_acc.data).unwrap();
        assert_eq!(vault_state.amount, amount_to_give);

        let maker_acc = svm.get_account(&maker_ata_a).unwrap();
        let maker_state = spl_token_2022::state::Account::unpack(&maker_acc.data).unwrap();
        assert_eq!(maker_state.amount, 1_000_000_000 - amount_to_give);

        let esc = svm.get_account(&escrow).unwrap();
        let d = &esc.data;
        assert_eq!(&d[0..32], payer.pubkey().as_ref());
        assert_eq!(
            u64::from_le_bytes(d[96..104].try_into().unwrap()),
            amount_to_receive
        );
        assert_eq!(
            u64::from_le_bytes(d[104..112].try_into().unwrap()),
            amount_to_give
        );
        assert_eq!(d[112], bump);
        assert_eq!(d[32..64], mint_a.to_bytes());
        assert_eq!(d[64..96], mint_b.to_bytes());
    }

    #[test]
    pub fn test_take_instruction() {
        let (
            mut svm,
            maker,
            mint_a,
            mint_b,
            escrow,
            _bump,
            vault,
            maker_ata_a,
            amount_to_receive,
            amount_to_give,
        ) = make_setup();

        let taker = Keypair::new();
        svm.airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut svm, &taker, &mint_b)
            .owner(&taker.pubkey())
            .send()
            .unwrap();
        MintTo::new(&mut svm, &maker, &mint_b, &taker_ata_b, 100_000_000)
            .send()
            .unwrap();

        let taker_ata_a =
            spl_associated_token_account::get_associated_token_address(&taker.pubkey(), &mint_a);
        let maker_ata_b =
            spl_associated_token_account::get_associated_token_address(&maker.pubkey(), &mint_b);

        let take_ix = Instruction {
            program_id: program_id(),
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
                AccountMeta::new(solana_sdk_ids::system_program::ID, false),
                AccountMeta::new(TOKEN_PROGRAM_ID, false),
                AccountMeta::new(
                    ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap(),
                    false,
                ),
            ],
            data: vec![1u8],
        };

        let maker_balance_before = svm.get_account(&maker.pubkey()).unwrap().lamports;

        let tx = Transaction::new(
            &[&taker],
            Message::new(&[take_ix], Some(&taker.pubkey())),
            svm.latest_blockhash(),
        );
        let result = svm.send_transaction(tx).unwrap();
        println!(
            "Take compute_units_consumed: {}",
            result.compute_units_consumed
        );

        let taker_ata_a_account = svm.get_account(&taker_ata_a).unwrap();
        let taker_ata_a_state =
            spl_token_2022::state::Account::unpack(&taker_ata_a_account.data).unwrap();
        assert_eq!(taker_ata_a_state.amount, amount_to_give);

        let maker_ata_b_account = svm.get_account(&maker_ata_b).unwrap();
        let maker_ata_b_state =
            spl_token_2022::state::Account::unpack(&maker_ata_b_account.data).unwrap();
        assert_eq!(maker_ata_b_state.amount, amount_to_receive);

        assert_account_closed(&svm, &vault);
        assert_account_closed(&svm, &escrow);

        let maker_balance_after = svm.get_account(&maker.pubkey()).unwrap().lamports;
        println!("maker_balance_before = {maker_balance_before}");
        println!("maker_balance_after = {maker_balance_after}");
        assert!(
            maker_balance_after > maker_balance_before,
            "maker should receive rent refund when escrow and vault are closed"
        );
    }

    #[test]
    pub fn test_cancel_instruction() {
        let (
            mut svm,
            maker,
            mint_a,
            _mint_b,
            escrow,
            _bump,
            vault,
            maker_ata_a,
            _amount_to_receive,
            amount_to_give,
        ) = make_setup();

        let cancel_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(maker.pubkey(), true),
                AccountMeta::new(mint_a, false),
                AccountMeta::new(escrow, false),
                AccountMeta::new(vault, false),
                AccountMeta::new(maker_ata_a, false),
                AccountMeta::new(TOKEN_PROGRAM_ID, false),
            ],
            data: vec![2u8],
        };

        let tx = Transaction::new(
            &[&maker],
            Message::new(&[cancel_ix], Some(&maker.pubkey())),
            svm.latest_blockhash(),
        );
        let result = svm.send_transaction(tx).unwrap();
        println!(
            "Cancel compute_units_consumed: {}",
            result.compute_units_consumed
        );

        let maker_ata_a_account = svm.get_account(&maker_ata_a).unwrap();
        let maker_ata_a_state =
            spl_token_2022::state::Account::unpack(&maker_ata_a_account.data).unwrap();
        assert_eq!(maker_ata_a_state.amount, 1_000_000_000);
        assert_account_closed(&svm, &vault);
        assert_account_closed(&svm, &escrow);
        assert_eq!(amount_to_give, 500_000_000);
    }

    #[test]
    pub fn test_take_instruction_underfunded_taker() {
        let (
            mut svm,
            maker,
            mint_a,
            mint_b,
            escrow,
            _bump,
            vault,
            _maker_ata_a,
            _amount_to_receive,
            amount_to_give,
        ) = make_setup();

        let taker = Keypair::new();
        svm.airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut svm, &taker, &mint_b)
            .owner(&taker.pubkey())
            .send()
            .unwrap();
        MintTo::new(&mut svm, &maker, &mint_b, &taker_ata_b, 50_000_000)
            .send()
            .unwrap();

        let taker_ata_a =
            spl_associated_token_account::get_associated_token_address(&taker.pubkey(), &mint_a);
        let maker_ata_b =
            spl_associated_token_account::get_associated_token_address(&maker.pubkey(), &mint_b);

        let take_ix = Instruction {
            program_id: program_id(),
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
                AccountMeta::new(solana_sdk_ids::system_program::ID, false),
                AccountMeta::new(TOKEN_PROGRAM_ID, false),
                AccountMeta::new(
                    ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap(),
                    false,
                ),
            ],
            data: vec![1u8],
        };

        let tx = Transaction::new(
            &[&taker],
            Message::new(&[take_ix], Some(&taker.pubkey())),
            svm.latest_blockhash(),
        );
        let result = svm.send_transaction(tx);
        assert!(result.is_err(), "underfunded taker should fail");

        let vault_acc = svm.get_account(&vault).unwrap();
        let vault_state = spl_token_2022::state::Account::unpack(&vault_acc.data).unwrap();
        assert_eq!(vault_state.amount, amount_to_give);
        assert!(svm.get_account(&escrow).is_some());
    }

    #[test]
    pub fn test_cancel_instruction_stranger() {
        let (
            mut svm,
            maker,
            mint_a,
            _mint_b,
            escrow,
            _bump,
            vault,
            maker_ata_a,
            _amount_to_receive,
            amount_to_give,
        ) = make_setup();

        let stranger = Keypair::new();
        svm.airdrop(&stranger.pubkey(), 10 * LAMPORTS_PER_SOL)
            .unwrap();

        let cancel_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(stranger.pubkey(), true),
                AccountMeta::new(mint_a, false),
                AccountMeta::new(escrow, false),
                AccountMeta::new(vault, false),
                AccountMeta::new(maker_ata_a, false),
                AccountMeta::new(TOKEN_PROGRAM_ID, false),
            ],
            data: vec![2u8],
        };

        let tx = Transaction::new(
            &[&stranger],
            Message::new(&[cancel_ix], Some(&stranger.pubkey())),
            svm.latest_blockhash(),
        );
        let result = svm.send_transaction(tx);
        assert!(result.is_err(), "stranger must not be able to cancel");

        let vault_acc = svm.get_account(&vault).unwrap();
        let vault_state = spl_token_2022::state::Account::unpack(&vault_acc.data).unwrap();
        assert_eq!(vault_state.amount, amount_to_give);
        assert_ne!(stranger.pubkey(), maker.pubkey());
    }
}
