#[cfg(test)]
mod tests {

    use std::path::PathBuf;

    use litesvm::{types::FailedTransactionMetadata, LiteSVM};
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

    const AMOUNT_TO_RECEIVE: u64 = 100_000_000; // 100 B
    const AMOUNT_TO_GIVE: u64 = 500_000_000;    // 500 A
    const MAKER_INITIAL_A: u64 = 1_000_000_000; // 1,000 A

    fn program_id() -> Pubkey {
        Pubkey::from(crate::ID)
    }

    fn associated_token_program() -> Pubkey {
        ASSOCIATED_TOKEN_PROGRAM_ID.parse().unwrap()
    }

    fn system_program() -> Pubkey {
        solana_sdk_ids::system_program::ID
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

    struct MakeFixture {
        svm: LiteSVM,
        maker: Keypair,
        mint_a: Pubkey,
        mint_b: Pubkey,
        escrow: Pubkey,
        bump: u8,
        vault: Pubkey,
        maker_ata_a: Pubkey,
    }

    fn make_escrow() -> MakeFixture {
        let (mut svm, maker) = setup();
        let program_id = program_id();
        assert_eq!(program_id.to_string(), PROGRAM_ID);

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

        let (escrow, bump) = Pubkey::find_program_address(
            &[b"escrow".as_ref(), maker.pubkey().as_ref()],
            &program_id,
        );

        let vault = spl_associated_token_account::get_associated_token_address(&escrow, &mint_a);

        MintTo::new(&mut svm, &maker, &mint_a, &maker_ata_a, MAKER_INITIAL_A)
            .send()
            .unwrap();

        let make_data = [
            vec![0u8],
            AMOUNT_TO_RECEIVE.to_le_bytes().to_vec(),
            AMOUNT_TO_GIVE.to_le_bytes().to_vec(),
        ].concat();

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
            data: make_data,
        };

        let tx = send(&mut svm, &maker, make_ix).unwrap();
        println!("Make CUs Consumed: {}", tx.compute_units_consumed);

        MakeFixture {
            svm,
            maker,
            mint_a,
            mint_b,
            escrow,
            bump,
            vault,
            maker_ata_a,
        }
    }

    fn send(
        svm: &mut LiteSVM,
        payer: &Keypair,
        ix: Instruction,
    ) -> Result<litesvm::types::TransactionMetadata, FailedTransactionMetadata> {
        let message = Message::new(&[ix], Some(&payer.pubkey()));
        let transaction = Transaction::new(&[payer], message, svm.latest_blockhash());
        svm.send_transaction(transaction)
    }

    fn token_amount(svm: &LiteSVM, ata: &Pubkey) -> u64 {
        let acc = svm.get_account(ata).unwrap();
        spl_token_2022::state::Account::unpack(&acc.data).unwrap().amount
    }

    fn lamports(svm: &LiteSVM, address: &Pubkey) -> u64 {
        svm.get_account(address).map(|a| a.lamports).unwrap_or(0)
    }

    fn assert_closed(svm: &LiteSVM, address: &Pubkey, label: &str) {
        match svm.get_account(address) {
            None => {}
            Some(acc) => {
                assert_eq!(acc.lamports, 0, "{label} still has lamports");
                assert_eq!(acc.owner, system_program(), "{label} is not owned by system");
            }
        }
    }

    fn take_ix(
        fixture: &MakeFixture,
        taker: &Keypair,
        taker_ata_a: Pubkey,
        taker_ata_b: Pubkey,
        maker_ata_b: Pubkey,
    ) -> Instruction {
        Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(taker.pubkey(), true),
                AccountMeta::new(fixture.maker.pubkey(), false),
                AccountMeta::new_readonly(fixture.mint_a, false),
                AccountMeta::new_readonly(fixture.mint_b, false),
                AccountMeta::new(fixture.escrow, false),
                AccountMeta::new(fixture.vault, false),
                AccountMeta::new(taker_ata_a, false),
                AccountMeta::new(taker_ata_b, false),
                AccountMeta::new(maker_ata_b, false),
                AccountMeta::new_readonly(system_program(), false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
                AccountMeta::new_readonly(associated_token_program(), false),
            ],
            data: vec![1u8],
        }
    }

    fn cancel_ix(fixture: &MakeFixture, maker: Pubkey, maker_is_signer: bool) -> Instruction {
        Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(maker, maker_is_signer),
                AccountMeta::new_readonly(fixture.mint_a, false),
                AccountMeta::new(fixture.escrow, false),
                AccountMeta::new(fixture.vault, false),
                AccountMeta::new(fixture.maker_ata_a, false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
            ],
            data: vec![2u8],
        }
    }

    fn fund_taker(fixture: &mut MakeFixture, amount_b: u64) -> (Keypair, Pubkey, Pubkey, Pubkey) {
        let taker = Keypair::new();
        fixture.svm.airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut fixture.svm, &taker, &fixture.mint_b)
            .owner(&taker.pubkey())
            .send()
            .unwrap();

        MintTo::new(&mut fixture.svm, &fixture.maker, &fixture.mint_b, &taker_ata_b, amount_b)
            .send()
            .unwrap();

        let taker_ata_a = spl_associated_token_account::get_associated_token_address(
            &taker.pubkey(),
            &fixture.mint_a,
        );
        let maker_ata_b = spl_associated_token_account::get_associated_token_address(
            &fixture.maker.pubkey(),
            &fixture.mint_b,
        );

        (taker, taker_ata_a, taker_ata_b, maker_ata_b)
    }

    #[test]
    pub fn test_make_instruction() {
        let fx = make_escrow();

        let vault_acc = fx.svm.get_account(&fx.vault).unwrap();
        let vault_state = spl_token_2022::state::Account::unpack(&vault_acc.data).unwrap();
        println!("Vault owner: {} (escrow PDA? {})", vault_state.owner, vault_state.owner == fx.escrow);
        println!("Vault balance: {}", vault_state.amount);
        assert_eq!(vault_state.amount, AMOUNT_TO_GIVE);

        let maker_state_amount = token_amount(&fx.svm, &fx.maker_ata_a);
        println!("Maker ATA balance: {}", maker_state_amount);
        assert_eq!(maker_state_amount, MAKER_INITIAL_A - AMOUNT_TO_GIVE);

        let esc = fx.svm.get_account(&fx.escrow).unwrap();
        println!("Escrow account owner: {} (program? {})", esc.owner, esc.owner == program_id());
        println!("Escrow data len: {}", esc.data.len());
        let d = &esc.data;
        println!("  maker   = {}", Pubkey::new_from_array(d[0..32].try_into().unwrap()));
        println!("  mint_a  = {}", Pubkey::new_from_array(d[32..64].try_into().unwrap()));
        println!("  mint_b  = {}", Pubkey::new_from_array(d[64..96].try_into().unwrap()));
        println!("  receive = {}", u64::from_le_bytes(d[96..104].try_into().unwrap()));
        println!("  give    = {}", u64::from_le_bytes(d[104..112].try_into().unwrap()));
        println!("  bump    = {}", d[112]);
        assert_eq!(&d[0..32], fx.maker.pubkey().as_ref());
        assert_eq!(u64::from_le_bytes(d[96..104].try_into().unwrap()), AMOUNT_TO_RECEIVE);
        assert_eq!(u64::from_le_bytes(d[104..112].try_into().unwrap()), AMOUNT_TO_GIVE);
        assert_eq!(d[112], fx.bump);
    }

    #[test]
    pub fn test_take_instruction() {
        let mut fx = make_escrow();
        let (taker, taker_ata_a, taker_ata_b, maker_ata_b) = fund_taker(&mut fx, AMOUNT_TO_RECEIVE);

        let maker_sol_before = lamports(&fx.svm, &fx.maker.pubkey());
        let escrow_rent = lamports(&fx.svm, &fx.escrow);
        let vault_rent = lamports(&fx.svm, &fx.vault);

        let ix = take_ix(&fx, &taker, taker_ata_a, taker_ata_b, maker_ata_b);
        let tx = send(&mut fx.svm, &taker, ix).expect("Take should succeed");
        println!("Take CUs Consumed: {}", tx.compute_units_consumed);

        assert_eq!(token_amount(&fx.svm, &taker_ata_a), AMOUNT_TO_GIVE);
        assert_eq!(token_amount(&fx.svm, &maker_ata_b), AMOUNT_TO_RECEIVE);
        assert_closed(&fx.svm, &fx.vault, "vault");
        assert_closed(&fx.svm, &fx.escrow, "escrow");
        assert_eq!(
            lamports(&fx.svm, &fx.maker.pubkey()),
            maker_sol_before + escrow_rent + vault_rent,
            "maker should receive rent from both closed accounts"
        );
    }

    #[test]
    pub fn test_cancel_instruction() {
        let mut fx = make_escrow();
        let maker_pk = fx.maker.pubkey();

        let ix = cancel_ix(&fx, maker_pk, true);
        let tx = send(&mut fx.svm, &fx.maker, ix).expect("Cancel should succeed");
        println!("Cancel CUs Consumed: {}", tx.compute_units_consumed);

        assert_eq!(token_amount(&fx.svm, &fx.maker_ata_a), MAKER_INITIAL_A);
        assert_closed(&fx.svm, &fx.vault, "vault");
        assert_closed(&fx.svm, &fx.escrow, "escrow");
    }

    #[test]
    pub fn test_take_fails_when_taker_has_only_50_b() {
        let mut fx = make_escrow();
        let (taker, taker_ata_a, taker_ata_b, maker_ata_b) =
            fund_taker(&mut fx, AMOUNT_TO_RECEIVE / 2);

        let ix = take_ix(&fx, &taker, taker_ata_a, taker_ata_b, maker_ata_b);
        let result = send(&mut fx.svm, &taker, ix);
        assert!(result.is_err(), "underfunded Take must fail");

        assert_eq!(token_amount(&fx.svm, &fx.vault), AMOUNT_TO_GIVE);
        assert_eq!(token_amount(&fx.svm, &fx.maker_ata_a), MAKER_INITIAL_A - AMOUNT_TO_GIVE);
    }

    #[test]
    pub fn test_cancel_fails_when_signed_by_stranger() {
        let mut fx = make_escrow();

        let stranger = Keypair::new();
        fx.svm.airdrop(&stranger.pubkey(), LAMPORTS_PER_SOL).unwrap();

        let ix = cancel_ix(&fx, stranger.pubkey(), true);
        let result = send(&mut fx.svm, &stranger, ix);
        assert!(result.is_err(), "a stranger must not be able to cancel");

        let vault_acc = fx.svm.get_account(&fx.vault).unwrap();
        let vault_state = spl_token_2022::state::Account::unpack(&vault_acc.data).unwrap();
        assert_eq!(vault_state.amount, AMOUNT_TO_GIVE);
        assert_eq!(token_amount(&fx.svm, &fx.maker_ata_a), MAKER_INITIAL_A - AMOUNT_TO_GIVE);
    }
}
