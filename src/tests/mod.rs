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

    const MAKER_INITIAL_A: u64 = 1_000_000_000; // 1,000 tokens with 6 decimals

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
    struct EscrowSetup {
        svm: LiteSVM,
        maker: Keypair,
        mint_a: Pubkey,
        mint_b: Pubkey,
        escrow: Pubkey,
        bump: u8,
        vault: Pubkey,
    }

    /// Runs the Make instruction (two mints, a funded maker ATA, the Make transaction
    /// itself) and hands back what Take/Cancel tests build on. The maker is the same
    /// keypair as the fee payer, matching how the account list expects `maker` to sign.
    fn make_escrow(amount_to_receive: u64, amount_to_give: u64) -> EscrowSetup {
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

        let escrow = Pubkey::find_program_address(
            &[b"escrow".as_ref(), maker.pubkey().as_ref()],
            &program_id,
        );

        let vault = spl_associated_token_account::get_associated_token_address(&escrow.0, &mint_a);

        let associated_token_program = ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();
        let token_program = TOKEN_PROGRAM_ID;
        let system_program = solana_sdk_ids::system_program::ID;

        MintTo::new(&mut svm, &maker, &mint_a, &maker_ata_a, MAKER_INITIAL_A)
            .send()
            .unwrap();

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

        let tx = svm.send_transaction(transaction).unwrap();
        println!("Make CUs Consumed: {}", tx.compute_units_consumed);

        EscrowSetup {
            svm,
            maker,
            mint_a,
            mint_b,
            escrow: escrow.0,
            bump: escrow.1,
            vault,
        }
    }

    #[test]
    pub fn test_make_instruction() {
        assert_eq!(program_id().to_string(), PROGRAM_ID);

        let amount_to_receive: u64 = 100_000_000; // 100 tokens with 6 decimal places
        let amount_to_give: u64 = 500_000_000;    // 500 tokens with 6 decimal places

        let setup = make_escrow(amount_to_receive, amount_to_give);

        let vault_acc = setup.svm.get_account(&setup.vault).unwrap();
        let vault_state = spl_token_2022::state::Account::unpack(&vault_acc.data).unwrap();
        println!("Vault owner: {} (escrow PDA? {})", vault_state.owner, vault_state.owner == setup.escrow);
        println!("Vault balance: {}", vault_state.amount);
        assert_eq!(vault_state.amount, amount_to_give);

        let maker_ata_a = spl_associated_token_account::get_associated_token_address(&setup.maker.pubkey(), &setup.mint_a);
        let maker_acc = setup.svm.get_account(&maker_ata_a).unwrap();
        let maker_state = spl_token_2022::state::Account::unpack(&maker_acc.data).unwrap();
        println!("Maker ATA balance: {}", maker_state.amount);
        assert_eq!(maker_state.amount, MAKER_INITIAL_A - amount_to_give);

        let esc = setup.svm.get_account(&setup.escrow).unwrap();
        println!("Escrow account owner: {} (program? {})", esc.owner, esc.owner == program_id());
        println!("Escrow data len: {}", esc.data.len());
        let d = &esc.data;
        println!("  maker   = {}", Pubkey::new_from_array(d[0..32].try_into().unwrap()));
        println!("  mint_a  = {}", Pubkey::new_from_array(d[32..64].try_into().unwrap()));
        println!("  mint_b  = {}", Pubkey::new_from_array(d[64..96].try_into().unwrap()));
        println!("  receive = {}", u64::from_le_bytes(d[96..104].try_into().unwrap()));
        println!("  give    = {}", u64::from_le_bytes(d[104..112].try_into().unwrap()));
        println!("  bump    = {}", d[112]);
        assert_eq!(&d[0..32], setup.maker.pubkey().as_ref());
        assert_eq!(u64::from_le_bytes(d[96..104].try_into().unwrap()), amount_to_receive);
        assert_eq!(u64::from_le_bytes(d[104..112].try_into().unwrap()), amount_to_give);
        assert_eq!(d[112], setup.bump);
    }

    #[test]
    pub fn test_take_instruction() {
        let amount_to_receive: u64 = 100_000_000;
        let amount_to_give: u64 = 500_000_000;
        let mut setup = make_escrow(amount_to_receive, amount_to_give);

        let taker = Keypair::new();
        setup.svm.airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL).expect("Airdrop failed");

        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut setup.svm, &taker, &setup.mint_b)
            .owner(&taker.pubkey())
            .send()
            .unwrap();

        MintTo::new(&mut setup.svm, &setup.maker, &setup.mint_b, &taker_ata_b, amount_to_receive)
            .send()
            .unwrap();

        // Not created yet: Take must create these via CreateIdempotent.
        let taker_ata_a = spl_associated_token_account::get_associated_token_address(&taker.pubkey(), &setup.mint_a);
        let maker_ata_b = spl_associated_token_account::get_associated_token_address(&setup.maker.pubkey(), &setup.mint_b);

        let associated_token_program = ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();
        let token_program = TOKEN_PROGRAM_ID;
        let system_program = solana_sdk_ids::system_program::ID;

        let take_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(taker.pubkey(), true),
                AccountMeta::new(setup.maker.pubkey(), false),
                AccountMeta::new(setup.mint_a, false),
                AccountMeta::new(setup.mint_b, false),
                AccountMeta::new(setup.escrow, false),
                AccountMeta::new(setup.vault, false),
                AccountMeta::new(taker_ata_a, false),
                AccountMeta::new(taker_ata_b, false),
                AccountMeta::new(maker_ata_b, false),
                AccountMeta::new(system_program, false),
                AccountMeta::new(token_program, false),
                AccountMeta::new(associated_token_program, false),
            ],
            data: vec![1u8],
        };

        let maker_lamports_before = setup.svm.get_account(&setup.maker.pubkey()).unwrap().lamports;

        let message = Message::new(&[take_ix], Some(&taker.pubkey()));
        let recent_blockhash = setup.svm.latest_blockhash();
        let transaction = Transaction::new(&[&taker], message, recent_blockhash);

        let tx = setup.svm.send_transaction(transaction).unwrap();
        println!("Take transaction successful");
        println!("CUs Consumed: {}", tx.compute_units_consumed);

        let taker_ata_a_acc = setup.svm.get_account(&taker_ata_a).unwrap();
        let taker_ata_a_state = spl_token_2022::state::Account::unpack(&taker_ata_a_acc.data).unwrap();
        assert_eq!(taker_ata_a_state.amount, amount_to_give);

        let maker_ata_b_acc = setup.svm.get_account(&maker_ata_b).unwrap();
        let maker_ata_b_state = spl_token_2022::state::Account::unpack(&maker_ata_b_acc.data).unwrap();
        assert_eq!(maker_ata_b_state.amount, amount_to_receive);

        assert!(setup.svm.get_account(&setup.vault).map_or(true, |a| a.lamports == 0));
        assert!(setup.svm.get_account(&setup.escrow).map_or(true, |a| a.lamports == 0));

        let maker_lamports_after = setup.svm.get_account(&setup.maker.pubkey()).unwrap().lamports;
        assert!(maker_lamports_after > maker_lamports_before, "maker should receive the rent from both closed accounts");
    }

    #[test]
    pub fn test_cancel_instruction() {
        let amount_to_receive: u64 = 100_000_000;
        let amount_to_give: u64 = 500_000_000;
        let mut setup = make_escrow(amount_to_receive, amount_to_give);

        let maker_ata_a = spl_associated_token_account::get_associated_token_address(&setup.maker.pubkey(), &setup.mint_a);
        let token_program = TOKEN_PROGRAM_ID;

        let cancel_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(setup.maker.pubkey(), true),
                AccountMeta::new(setup.mint_a, false),
                AccountMeta::new(setup.escrow, false),
                AccountMeta::new(setup.vault, false),
                AccountMeta::new(maker_ata_a, false),
                AccountMeta::new(token_program, false),
            ],
            data: vec![2u8],
        };

        let message = Message::new(&[cancel_ix], Some(&setup.maker.pubkey()));
        let recent_blockhash = setup.svm.latest_blockhash();
        let transaction = Transaction::new(&[&setup.maker], message, recent_blockhash);

        let tx = setup.svm.send_transaction(transaction).unwrap();
        println!("Cancel transaction successful");
        println!("CUs Consumed: {}", tx.compute_units_consumed);

        let maker_acc = setup.svm.get_account(&maker_ata_a).unwrap();
        let maker_state = spl_token_2022::state::Account::unpack(&maker_acc.data).unwrap();
        assert_eq!(maker_state.amount, MAKER_INITIAL_A);

        assert!(setup.svm.get_account(&setup.vault).map_or(true, |a| a.lamports == 0));
        assert!(setup.svm.get_account(&setup.escrow).map_or(true, |a| a.lamports == 0));
    }

    #[test]
    pub fn test_take_instruction_underfunded_taker_fails() {
        let amount_to_receive: u64 = 100_000_000;
        let amount_to_give: u64 = 500_000_000;
        let mut setup = make_escrow(amount_to_receive, amount_to_give);

        let taker = Keypair::new();
        setup.svm.airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL).expect("Airdrop failed");

        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut setup.svm, &taker, &setup.mint_b)
            .owner(&taker.pubkey())
            .send()
            .unwrap();

        // Taker only has half of what the escrow requires.
        let underfunded_amount = amount_to_receive / 2;
        MintTo::new(&mut setup.svm, &setup.maker, &setup.mint_b, &taker_ata_b, underfunded_amount)
            .send()
            .unwrap();

        let taker_ata_a = spl_associated_token_account::get_associated_token_address(&taker.pubkey(), &setup.mint_a);
        let maker_ata_b = spl_associated_token_account::get_associated_token_address(&setup.maker.pubkey(), &setup.mint_b);

        let associated_token_program = ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();
        let token_program = TOKEN_PROGRAM_ID;
        let system_program = solana_sdk_ids::system_program::ID;

        let take_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(taker.pubkey(), true),
                AccountMeta::new(setup.maker.pubkey(), false),
                AccountMeta::new(setup.mint_a, false),
                AccountMeta::new(setup.mint_b, false),
                AccountMeta::new(setup.escrow, false),
                AccountMeta::new(setup.vault, false),
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
        let recent_blockhash = setup.svm.latest_blockhash();
        let transaction = Transaction::new(&[&taker], message, recent_blockhash);

        let result = setup.svm.send_transaction(transaction);
        assert!(result.is_err(), "Take with an underfunded taker must fail, not just leave a partial trade");

        // Nothing moved: the vault still holds the full deposit.
        let vault_acc = setup.svm.get_account(&setup.vault).unwrap();
        let vault_state = spl_token_2022::state::Account::unpack(&vault_acc.data).unwrap();
        assert_eq!(vault_state.amount, amount_to_give);
    }

    #[test]
    pub fn test_cancel_instruction_wrong_signer_fails() {
        let amount_to_receive: u64 = 100_000_000;
        let amount_to_give: u64 = 500_000_000;
        let mut setup = make_escrow(amount_to_receive, amount_to_give);

        let stranger = Keypair::new();
        setup.svm.airdrop(&stranger.pubkey(), 10 * LAMPORTS_PER_SOL).expect("Airdrop failed");

        let stranger_ata_a = spl_associated_token_account::get_associated_token_address(&stranger.pubkey(), &setup.mint_a);
        let token_program = TOKEN_PROGRAM_ID;

        // Same escrow and vault as the real maker's, but signed and directed by a stranger.
        let cancel_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(stranger.pubkey(), true),
                AccountMeta::new(setup.mint_a, false),
                AccountMeta::new(setup.escrow, false),
                AccountMeta::new(setup.vault, false),
                AccountMeta::new(stranger_ata_a, false),
                AccountMeta::new(token_program, false),
            ],
            data: vec![2u8],
        };

        let message = Message::new(&[cancel_ix], Some(&stranger.pubkey()));
        let recent_blockhash = setup.svm.latest_blockhash();
        let transaction = Transaction::new(&[&stranger], message, recent_blockhash);

        let result = setup.svm.send_transaction(transaction);
        assert!(result.is_err(), "Cancel signed by a non-maker stranger must fail");

        // The vault must still hold the full deposit; nothing was drained.
        let vault_acc = setup.svm.get_account(&setup.vault).unwrap();
        let vault_state = spl_token_2022::state::Account::unpack(&vault_acc.data).unwrap();
        assert_eq!(vault_state.amount, amount_to_give);
    }
}
