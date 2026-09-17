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
    use spl_associated_token_account::get_associated_token_address;

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
        let program_data = std::fs::read(&so_path)
            .unwrap_or_else(|e| panic!("Failed to read program SO file at {}: {e}. Run `cargo build-sbf` first.", so_path.display()));
    
        svm.add_program(program_id(), &program_data).expect("Failed to add program");

        (svm, payer)
    }

    // 1. The Helper: Sets up the trade window so tests don't repeat themselves
    fn setup_make() -> (LiteSVM, Keypair, Pubkey, Pubkey, Pubkey, Pubkey, u8, Pubkey) {
        let (mut svm, maker) = setup();
        let program_id = program_id();

        let mint_a = CreateMint::new(&mut svm, &maker).decimals(6).authority(&maker.pubkey()).send().unwrap();
        let mint_b = CreateMint::new(&mut svm, &maker).decimals(6).authority(&maker.pubkey()).send().unwrap();

        let maker_ata_a = CreateAssociatedTokenAccount::new(&mut svm, &maker, &mint_a)
            .owner(&maker.pubkey()).send().unwrap();

        let escrow = Pubkey::find_program_address(&[b"escrow", maker.pubkey().as_ref()], &program_id);
        let vault = get_associated_token_address(&escrow.0, &mint_a);

        MintTo::new(&mut svm, &maker, &mint_a, &maker_ata_a, 1000000000).send().unwrap();

        let amount_to_receive: u64 = 100_000_000;
        let amount_to_give: u64 = 500_000_000;

        let make_data = [
            vec![0u8], 
            amount_to_receive.to_le_bytes().to_vec(),
            amount_to_give.to_le_bytes().to_vec(),
        ].concat();

        let make_ix = Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new(maker.pubkey(), true),
                AccountMeta::new_readonly(mint_a, false),
                AccountMeta::new_readonly(mint_b, false),
                AccountMeta::new(escrow.0, false),
                AccountMeta::new(maker_ata_a, false),
                AccountMeta::new(vault, false),
                AccountMeta::new_readonly(solana_sdk_ids::system_program::ID, false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
                AccountMeta::new_readonly(ASSOCIATED_TOKEN_PROGRAM_ID.parse().unwrap(), false),
            ],
            data: make_data,
        };

        let tx = Transaction::new(&[&maker], Message::new(&[make_ix], Some(&maker.pubkey())), svm.latest_blockhash());
        svm.send_transaction(tx).unwrap();

        (svm, maker, mint_a, mint_b, maker_ata_a, escrow.0, escrow.1, vault)
    }

    #[test]
    pub fn test_make_instruction() {
        let (_, _, _, _, _, _, _, _) = setup_make();
        // Since setup_make unwraps the tx, a successful return proves Make works.
    }

    #[test]
    pub fn test_take_instruction() {
        let (mut svm, maker, mint_a, mint_b, _maker_ata_a, escrow_pda, _bump, vault_pda) = setup_make();
        
        let taker = Keypair::new();
        svm.airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut svm, &taker, &mint_b)
            .owner(&taker.pubkey()).send().unwrap();
        MintTo::new(&mut svm, &maker, &mint_b, &taker_ata_b, 100_000_000).send().unwrap();

        let taker_ata_a = get_associated_token_address(&taker.pubkey(), &mint_a);
        let maker_ata_b = get_associated_token_address(&maker.pubkey(), &mint_b);

        let take_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(taker.pubkey(), true),
                AccountMeta::new(maker.pubkey(), false),
                AccountMeta::new_readonly(mint_a, false),
                AccountMeta::new_readonly(mint_b, false),
                AccountMeta::new(escrow_pda, false),
                AccountMeta::new(vault_pda, false),
                AccountMeta::new(taker_ata_a, false),
                AccountMeta::new(taker_ata_b, false),
                AccountMeta::new(maker_ata_b, false),
                AccountMeta::new_readonly(solana_sdk_ids::system_program::ID, false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
                AccountMeta::new_readonly(ASSOCIATED_TOKEN_PROGRAM_ID.parse().unwrap(), false),
            ],
            data: vec![1u8],
        };

        let tx = Transaction::new(&[&taker], Message::new(&[take_ix], Some(&taker.pubkey())), svm.latest_blockhash());
        let res = svm.send_transaction(tx).unwrap();
        
        println!("Take CUs Consumed: {}", res.compute_units_consumed);

        let t_ata_a = svm.get_account(&taker_ata_a).unwrap();
        let t_state_a = spl_token_2022::state::Account::unpack(&t_ata_a.data).unwrap();
        assert_eq!(t_state_a.amount, 500_000_000);

        let m_ata_b = svm.get_account(&maker_ata_b).unwrap();
        let m_state_b = spl_token_2022::state::Account::unpack(&m_ata_b.data).unwrap();
        assert_eq!(m_state_b.amount, 100_000_000);

        assert!(svm.get_account(&vault_pda).is_none());
        assert!(svm.get_account(&escrow_pda).is_none());
    }

    #[test]
    pub fn test_cancel_instruction() {
        let (mut svm, maker, mint_a, _mint_b, maker_ata_a, escrow_pda, _bump, vault_pda) = setup_make();

        let cancel_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(maker.pubkey(), true),
                AccountMeta::new_readonly(mint_a, false),
                AccountMeta::new(escrow_pda, false),
                AccountMeta::new(vault_pda, false),
                AccountMeta::new(maker_ata_a, false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
            ],
            data: vec![2u8],
        };

        let tx = Transaction::new(&[&maker], Message::new(&[cancel_ix], Some(&maker.pubkey())), svm.latest_blockhash());
        let res = svm.send_transaction(tx).unwrap();
        
        println!("Cancel CUs Consumed: {}", res.compute_units_consumed);

        let m_ata_a = svm.get_account(&maker_ata_a).unwrap();
        let m_state_a = spl_token_2022::state::Account::unpack(&m_ata_a.data).unwrap();
        assert_eq!(m_state_a.amount, 1_000_000_000);

        assert!(svm.get_account(&vault_pda).is_none());
        assert!(svm.get_account(&escrow_pda).is_none());
    }

    #[test]
    pub fn test_take_insufficient_funds() {
        let (mut svm, maker, mint_a, mint_b, _maker_ata_a, escrow_pda, _bump, vault_pda) = setup_make();
        
        let taker = Keypair::new();
        svm.airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut svm, &taker, &mint_b)
            .owner(&taker.pubkey()).send().unwrap();
        
        // Give taker only 50 tokens instead of the required 100
        MintTo::new(&mut svm, &maker, &mint_b, &taker_ata_b, 50_000_000).send().unwrap();

        let taker_ata_a = get_associated_token_address(&taker.pubkey(), &mint_a);
        let maker_ata_b = get_associated_token_address(&maker.pubkey(), &mint_b);

        let take_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(taker.pubkey(), true),
                AccountMeta::new(maker.pubkey(), false),
                AccountMeta::new_readonly(mint_a, false),
                AccountMeta::new_readonly(mint_b, false),
                AccountMeta::new(escrow_pda, false),
                AccountMeta::new(vault_pda, false),
                AccountMeta::new(taker_ata_a, false),
                AccountMeta::new(taker_ata_b, false),
                AccountMeta::new(maker_ata_b, false),
                AccountMeta::new_readonly(solana_sdk_ids::system_program::ID, false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
                AccountMeta::new_readonly(ASSOCIATED_TOKEN_PROGRAM_ID.parse().unwrap(), false),
            ],
            data: vec![1u8],
        };

        let tx = Transaction::new(&[&taker], Message::new(&[take_ix], Some(&taker.pubkey())), svm.latest_blockhash());
        let result = svm.send_transaction(tx);
        
        assert!(result.is_err(), "Take with insufficient funds should fail");
    }

    #[test]
    pub fn test_cancel_wrong_signer() {
        let (mut svm, _maker, mint_a, _mint_b, maker_ata_a, escrow_pda, _bump, vault_pda) = setup_make();
        
        let hacker = Keypair::new();
        svm.airdrop(&hacker.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

        let cancel_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(hacker.pubkey(), true), // Hacker trying to sign as maker
                AccountMeta::new_readonly(mint_a, false),
                AccountMeta::new(escrow_pda, false),
                AccountMeta::new(vault_pda, false),
                AccountMeta::new(maker_ata_a, false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
            ],
            data: vec![2u8],
        };

        let tx = Transaction::new(&[&hacker], Message::new(&[cancel_ix], Some(&hacker.pubkey())), svm.latest_blockhash());
        let result = svm.send_transaction(tx);
        
        assert!(result.is_err(), "A stranger must not be able to cancel");

        // Prove tokens are still locked in the vault
        let vault_acc = svm.get_account(&vault_pda).unwrap();
        let vault_state = spl_token_2022::state::Account::unpack(&vault_acc.data).unwrap();
        assert_eq!(vault_state.amount, 500_000_000);
    }
}