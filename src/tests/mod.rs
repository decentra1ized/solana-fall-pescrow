#[cfg(test)]
mod test_cases {
    use std::path::PathBuf;

    use litesvm::LiteSVM;
    use litesvm_token::{spl_token, CreateAssociatedTokenAccount, MintTo};
    use solana_instruction::{AccountMeta, Instruction};
    use solana_keypair::Keypair;
    use solana_message::Message;
    use solana_native_token::LAMPORTS_PER_SOL;
    use solana_pubkey::Pubkey;
    use solana_signer::Signer;
    use solana_transaction::Transaction;

    pub mod test_cancel;
    pub mod test_make;
    pub mod test_take;

    const PROGRAM_ID: &str = "4ibrEMW5F6hKnkW4jVedswYv6H6VtwPN6ar6dvXDN1nT";
    const TOKEN_PROGRAM_ID: Pubkey = spl_token::ID;
    const ASSOCIATED_TOKEN_PROGRAM_ID: &str = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL";

    fn program_id() -> Pubkey {
        crate::ID
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

    fn run_make_helper(
        svm: &mut LiteSVM,
        maker: &Keypair,
        mint_a: Pubkey,
        mint_b: Pubkey,
    ) -> (Pubkey, Pubkey, u8, u64, u64) {
        let maker_ata_a = CreateAssociatedTokenAccount::new(svm, maker, &mint_a)
            .owner(&maker.pubkey())
            .send()
            .unwrap();

        let escrow = Pubkey::find_program_address(
            &[b"escrow".as_ref(), maker.pubkey().as_ref()],
            &program_id(),
        );
        let vault = spl_associated_token_account::get_associated_token_address(&escrow.0, &mint_a);

        MintTo::new(svm, maker, &mint_a, &maker_ata_a, 1_000_000_000)
            .send()
            .unwrap();

        let amount_to_receive = 100_000_000_u64;
        let amount_to_give = 500_000_000_u64;
        let make_data = [
            vec![0_u8],
            amount_to_receive.to_le_bytes().to_vec(),
            amount_to_give.to_le_bytes().to_vec(),
        ]
        .concat();

        let make_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(maker.pubkey(), true),
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
            data: make_data,
        };

        let message = Message::new(&[make_ix], Some(&maker.pubkey()));
        let transaction = Transaction::new(&[maker], message, svm.latest_blockhash());
        svm.send_transaction(transaction).unwrap();

        (escrow.0, vault, escrow.1, amount_to_receive, amount_to_give)
    }
}
