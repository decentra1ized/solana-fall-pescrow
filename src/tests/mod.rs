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

        #[allow(deprecated)]
        svm.set_sysvar(&solana_rent::Rent {
            lamports_per_byte_year: 6960,
            exemption_threshold: 1.0,
            burn_percent: 50,
        });

        svm.airdrop(&payer.pubkey(), 10 * LAMPORTS_PER_SOL).expect("Airdrop failed");

        let so_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/deploy/escrow.so");
        let program_data = std::fs::read(&so_path).unwrap_or_else(|e| {
            panic!("Failed to read program SO file at {}: {e}. Run `cargo build-sbf` first.", so_path.display())
        });
        svm.add_program(program_id(), &program_data).expect("Failed to add program");
        (svm, payer)
    }

    struct EscrowReady {
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
        make_cus: u64,
    }

    fn setup_after_make() -> EscrowReady {
        let (mut svm, maker) = setup();
        let program_id = program_id();

        let mint_a = CreateMint::new(&mut svm, &maker).decimals(6).authority(&maker.pubkey()).send().unwrap();
        let mint_b = CreateMint::new(&mut svm, &maker).decimals(6).authority(&maker.pubkey()).send().unwrap();
        let maker_ata_a = CreateAssociatedTokenAccount::new(&mut svm, &maker, &mint_a)
            .owner(&maker.pubkey()).send().unwrap();

        let escrow = Pubkey::find_program_address(&[b"escrow".as_ref(), maker.pubkey().as_ref()], &program_id);
        let vault = spl_associated_token_account::get_associated_token_address(&escrow.0, &mint_a);

        MintTo::new(&mut svm, &maker, &mint_a, &maker_ata_a, 1_000_000_000).send().unwrap();

        let amount_to_receive: u64 = 100_000_000;
        let amount_to_give: u64 = 500_000_000;

        let associated_token_program = ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();
        let token_program = TOKEN_PROGRAM_ID;
        let system_program = solana_sdk_ids::system_program::ID;

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
            data: [
                vec![0u8],
                amount_to_receive.to_le_bytes().to_vec(),
                amount_to_give.to_le_bytes().to_vec(),
            ].concat(),
        };

        let message = Message::new(&[make_ix], Some(&maker.pubkey()));
        let tx = svm.send_transaction(Transaction::new(&[&maker], message, svm.latest_blockhash())).unwrap();

        EscrowReady {
            svm,
            maker,
            mint_a,
            mint_b,
            maker_ata_a,
            escrow: escrow.0,
            bump: escrow.1,
            vault,
            amount_to_receive,
            amount_to_give,
            make_cus: tx.compute_units_consumed,
        }
    }

    fn token_amount(svm: &LiteSVM, ata: &Pubkey) -> u64 {
        let acc = svm.get_account(ata).unwrap();
        spl_token_2022::state::Account::unpack(&acc.data).unwrap().amount
    }

    fn is_closed(svm: &LiteSVM, key: &Pubkey) -> bool {
        match svm.get_account(key) {
            None => true,
            Some(acc) => acc.lamports == 0 || acc.data.is_empty(),
        }
    }

    #[test]
    pub fn test_make_instruction() {
        let ready = setup_after_make();
        println!("Make transaction successful");
        println!("CUs Consumed: {}", ready.make_cus);
        assert_eq!(token_amount(&ready.svm, &ready.vault), ready.amount_to_give);
        assert_eq!(token_amount(&ready.svm, &ready.maker_ata_a), 1_000_000_000 - ready.amount_to_give);
        let esc = ready.svm.get_account(&ready.escrow).unwrap();
        assert_eq!(esc.data[112], ready.bump);
    }

    #[test]
    pub fn test_take_instruction() {
        let mut ready = setup_after_make();
        let program_id = program_id();
        let associated_token_program = ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();
        let token_program = TOKEN_PROGRAM_ID;
        let system_program = solana_sdk_ids::system_program::ID;

        let taker = Keypair::new();
        ready.svm.airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut ready.svm, &taker, &ready.mint_b)
            .owner(&taker.pubkey()).send().unwrap();
        MintTo::new(&mut ready.svm, &ready.maker, &ready.mint_b, &taker_ata_b, ready.amount_to_receive)
            .send()
            .unwrap();

        let taker_ata_a = spl_associated_token_account::get_associated_token_address(&taker.pubkey(), &ready.mint_a);
        let maker_ata_b = spl_associated_token_account::get_associated_token_address(&ready.maker.pubkey(), &ready.mint_b);

        let take_ix = Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new(taker.pubkey(), true),
                AccountMeta::new(ready.maker.pubkey(), false),
                AccountMeta::new(ready.mint_a, false),
                AccountMeta::new(ready.mint_b, false),
                AccountMeta::new(ready.escrow, false),
                AccountMeta::new(ready.vault, false),
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
        let tx = ready.svm.send_transaction(Transaction::new(&[&taker], message, ready.svm.latest_blockhash())).unwrap();
        println!("Take transaction successful");
        println!("CUs Consumed: {}", tx.compute_units_consumed);

        assert_eq!(token_amount(&ready.svm, &taker_ata_a), ready.amount_to_give);
        assert_eq!(token_amount(&ready.svm, &maker_ata_b), ready.amount_to_receive);
        assert!(is_closed(&ready.svm, &ready.vault), "vault should be closed");
        assert!(is_closed(&ready.svm, &ready.escrow), "escrow should be closed");
    }
    #[test]
    pub fn test_cancel_instruction() {
        let mut ready = setup_after_make();
        let program_id = program_id();
        let token_program = TOKEN_PROGRAM_ID;

        let cancel_ix = Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new(ready.maker.pubkey(), true),
                AccountMeta::new(ready.mint_a, false),
                AccountMeta::new(ready.escrow, false),
                AccountMeta::new(ready.vault, false),
                AccountMeta::new(ready.maker_ata_a, false),
                AccountMeta::new(token_program, false),
            ],
            data: vec![2u8],
        };

        let message = Message::new(&[cancel_ix], Some(&ready.maker.pubkey()));
        let tx = ready.svm.send_transaction(Transaction::new(&[&ready.maker], message, ready.svm.latest_blockhash())).unwrap();
        println!("Cancel transaction successful");
        println!("CUs Consumed: {}", tx.compute_units_consumed);

        assert_eq!(token_amount(&ready.svm, &ready.maker_ata_a), 1_000_000_000);
        assert!(is_closed(&ready.svm, &ready.vault), "vault should be closed");
        assert!(is_closed(&ready.svm, &ready.escrow), "escrow should be closed");
    }

    #[test]
    pub fn test_take_insufficient_b() {
        let mut ready = setup_after_make();
        let program_id = program_id();
        let associated_token_program = ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();
        let token_program = TOKEN_PROGRAM_ID;
        let system_program = solana_sdk_ids::system_program::ID;

        let taker = Keypair::new();
        ready.svm.airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut ready.svm, &taker, &ready.mint_b)
            .owner(&taker.pubkey()).send().unwrap();
        MintTo::new(&mut ready.svm, &ready.maker, &ready.mint_b, &taker_ata_b, ready.amount_to_receive / 2)
            .send()
            .unwrap();

        let taker_ata_a = spl_associated_token_account::get_associated_token_address(&taker.pubkey(), &ready.mint_a);
        let maker_ata_b = spl_associated_token_account::get_associated_token_address(&ready.maker.pubkey(), &ready.mint_b);

        let take_ix = Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new(taker.pubkey(), true),
                AccountMeta::new(ready.maker.pubkey(), false),
                AccountMeta::new(ready.mint_a, false),
                AccountMeta::new(ready.mint_b, false),
                AccountMeta::new(ready.escrow, false),
                AccountMeta::new(ready.vault, false),
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
        let result = ready.svm.send_transaction(Transaction::new(&[&taker], message, ready.svm.latest_blockhash()));
        assert!(result.is_err(), "underfunded Take must fail");
        assert_eq!(token_amount(&ready.svm, &ready.vault), ready.amount_to_give);
        println!("Underfunded Take failed as expected");
    }

    #[test]
    pub fn test_cancel_by_stranger() {
        let mut ready = setup_after_make();
        let program_id = program_id();
        let token_program = TOKEN_PROGRAM_ID;

        let stranger = Keypair::new();
        ready.svm.airdrop(&stranger.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

        let cancel_ix = Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new(stranger.pubkey(), true),
                AccountMeta::new(ready.mint_a, false),
                AccountMeta::new(ready.escrow, false),
                AccountMeta::new(ready.vault, false),
                AccountMeta::new(ready.maker_ata_a, false),
                AccountMeta::new(token_program, false),
            ],
            data: vec![2u8],
        };

        let message = Message::new(&[cancel_ix], Some(&stranger.pubkey()));
        let result = ready.svm.send_transaction(Transaction::new(&[&stranger], message, ready.svm.latest_blockhash()));
        assert!(result.is_err(), "stranger Cancel must fail");
        assert_eq!(token_amount(&ready.svm, &ready.vault), ready.amount_to_give);
        println!("Stranger Cancel failed as expected");
    }

}
