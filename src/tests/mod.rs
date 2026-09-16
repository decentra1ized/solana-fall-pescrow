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

    fn system_program_id() -> Pubkey {
        solana_sdk_ids::system_program::ID
    }

    fn token_program_id() -> Pubkey {
        TOKEN_PROGRAM_ID
    }

    fn associated_token_program_id() -> Pubkey {
        ASSOCIATED_TOKEN_PROGRAM_ID.parse().unwrap()
    }

    /// LiteSVM often keeps a 0-lamport account instead of deleting it.
    fn assert_closed(svm: &LiteSVM, address: &Pubkey) {
        if let Some(acc) = svm.get_account(address) {
            assert_eq!(acc.lamports, 0, "{address} should be closed (0 lamports left)");
        }
    }

    struct MakeSetup {
        svm: LiteSVM,
        maker: Keypair,
        mint_a: Pubkey,
        mint_b: Pubkey,
        maker_ata_a: Pubkey,
        escrow: Pubkey,
        vault: Pubkey,
        amount_to_receive: u64,
        amount_to_give: u64,
    }

    fn make_escrow() -> MakeSetup {
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
            &PROGRAM_ID.parse().unwrap(),
        );

        let vault = spl_associated_token_account::get_associated_token_address(
            &escrow.0,
            &mint_a,
        );

        // 1000 A (6 decimals)
        MintTo::new(&mut svm, &maker, &mint_a, &maker_ata_a, 1_000_000_000)
            .send()
            .unwrap();

        let amount_to_receive: u64 = 100_000_000; // 100 B
        let amount_to_give: u64 = 500_000_000;    // 500 A

        let make_data = [
            vec![0u8],
            amount_to_receive.to_le_bytes().to_vec(),
            amount_to_give.to_le_bytes().to_vec(),
        ]
        .concat();

        let make_ix = Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new(maker.pubkey(), true),
                AccountMeta::new(mint_a, false),
                AccountMeta::new(mint_b, false),
                AccountMeta::new(escrow.0, false),
                AccountMeta::new(maker_ata_a, false),
                AccountMeta::new(vault, false),
                AccountMeta::new(system_program_id(), false),
                AccountMeta::new(token_program_id(), false),
                AccountMeta::new(associated_token_program_id(), false),
            ],
            data: make_data,
        };

        let message = Message::new(&[make_ix], Some(&maker.pubkey()));
        let recent_blockhash = svm.latest_blockhash();
        let transaction = Transaction::new(&[&maker], message, recent_blockhash);
        svm.send_transaction(transaction).unwrap();

        MakeSetup {
            svm,
            maker,
            mint_a,
            mint_b,
            maker_ata_a,
            escrow: escrow.0,
            vault,
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

        // Create the maker's associated token account for Mint A
        let maker_ata_a = CreateAssociatedTokenAccount::new(&mut svm, &payer, &mint_a)
            .owner(&payer.pubkey()).send().unwrap();
        println!("Maker ATA A: {}\n", maker_ata_a);

        // Derive the PDA for the escrow account using the maker's public key and a seed value
        let escrow = Pubkey::find_program_address(
            &[b"escrow".as_ref(), payer.pubkey().as_ref()],
            &PROGRAM_ID.parse().unwrap(),
        );
        println!("Escrow PDA: {}\n", escrow.0);

        // Derive the PDA for the vault associated token account using the escrow PDA and Mint A
        let vault = spl_associated_token_account::get_associated_token_address(
            &escrow.0,  // owner will be the escrow PDA
            &mint_a     // mint
        );
        println!("Vault PDA: {}\n", vault);

        // Define program IDs for associated token program, token program, and system program
        let associated_token_program = ASSOCIATED_TOKEN_PROGRAM_ID.parse::<Pubkey>().unwrap();
        let token_program = TOKEN_PROGRAM_ID;
        let system_program = solana_sdk_ids::system_program::ID;

        // Mint 1,000 tokens (with 6 decimal places) of Mint A to the maker's associated token account
        MintTo::new(&mut svm, &payer, &mint_a, &maker_ata_a, 1000000000)
            .send()
            .unwrap();

        let amount_to_receive: u64 = 100000000; // 100 tokens with 6 decimal places
        let amount_to_give: u64 = 500000000;    // 500 tokens with 6 decimal places
        let bump: u8 = escrow.1;   // canonical bump; the program derives the same one on-chain

        println!("Bump: {}", bump);

        // Create the "Make" instruction to deposit tokens into the escrow
        let make_data = [
            vec![0u8],              // Discriminator for "Make" instruction
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

        // Create and send the transaction containing the "Make" instruction
        let message = Message::new(&[make_ix], Some(&payer.pubkey()));
        let recent_blockhash = svm.latest_blockhash();

        let transaction = Transaction::new(&[&payer], message, recent_blockhash);

        // Send the transaction and capture the result
        let tx = svm.send_transaction(transaction).unwrap();

        // Log transaction details
        println!("\n\nMake transaction successful");
        println!("CUs Consumed: {}", tx.compute_units_consumed);

        // --- extra verification ---
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
        let MakeSetup {
            mut svm,
            maker,
            mint_a,
            mint_b,
            escrow,
            vault,
            amount_to_receive,
            amount_to_give,
            ..
        } = make_escrow();

        let program_id = program_id();

        let taker = Keypair::new();
        svm.airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

        // Bob's source of B — MUST exist and be funded
        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut svm, &taker, &mint_b)
            .owner(&taker.pubkey())
            .send()
            .unwrap();
        MintTo::new(&mut svm, &maker, &mint_b, &taker_ata_b, amount_to_receive)
            .send()
            .unwrap();

        // Derive only — do NOT create these
        let taker_ata_a =
            spl_associated_token_account::get_associated_token_address(&taker.pubkey(), &mint_a);
        let maker_ata_b =
            spl_associated_token_account::get_associated_token_address(&maker.pubkey(), &mint_b);

        // 12 accounts, SAME ORDER as take.rs
        let take_ix = Instruction {
            program_id,
            accounts: vec![
                AccountMeta::new(taker.pubkey(), true),   // 0 signer
                AccountMeta::new(maker.pubkey(), false),  // 1 writable (rent)
                AccountMeta::new(mint_a, false),          // 2
                AccountMeta::new(mint_b, false),          // 3
                AccountMeta::new(escrow, false),          // 4
                AccountMeta::new(vault, false),           // 5
                AccountMeta::new(taker_ata_a, false),     // 6 created on-chain
                AccountMeta::new(taker_ata_b, false),     // 7
                AccountMeta::new(maker_ata_b, false),     // 8 created on-chain
                AccountMeta::new(system_program_id(), false),
                AccountMeta::new(token_program_id(), false),
                AccountMeta::new(associated_token_program_id(), false),
            ],
            data: vec![1u8], // discriminator Take. no bump, no amounts
        };

        let message = Message::new(&[take_ix], Some(&taker.pubkey()));
        let recent_blockhash = svm.latest_blockhash();
        let transaction = Transaction::new(&[&taker], message, recent_blockhash);

        let maker_lamports_before = svm.get_account(&maker.pubkey()).unwrap().lamports;

        let tx = svm.send_transaction(transaction).unwrap();
        println!("\n\nTake transaction successful");
        println!("CUs Consumed: {}", tx.compute_units_consumed);

        let taker_a = spl_token_2022::state::Account::unpack(
            &svm.get_account(&taker_ata_a).unwrap().data,
        )
        .unwrap();
        assert_eq!(taker_a.amount, amount_to_give); // 500 A

        let maker_b = spl_token_2022::state::Account::unpack(
            &svm.get_account(&maker_ata_b).unwrap().data,
        )
        .unwrap();
        assert_eq!(maker_b.amount, amount_to_receive); // 100 B

        assert_closed(&svm, &vault);
        assert_closed(&svm, &escrow);

        let maker_lamports_after = svm.get_account(&maker.pubkey()).unwrap().lamports;
        assert!(
            maker_lamports_after > maker_lamports_before,
            "maker should receive rent from the closed vault and escrow"
        );
    }    

    #[test]
    pub fn test_cancel_instruction() {
        let MakeSetup {
            mut svm,
            maker,
            mint_a,
            maker_ata_a,
            escrow,
            vault,
            ..
        } = make_escrow();

        let cancel_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(maker.pubkey(), true),  // 0 must sign
                AccountMeta::new(mint_a, false),         // 1
                AccountMeta::new(escrow, false),         // 2
                AccountMeta::new(vault, false),          // 3
                AccountMeta::new(maker_ata_a, false),    // 4 A comes back here
                AccountMeta::new(token_program_id(), false), // 5
            ],
            data: vec![2u8],
        };

        let message = Message::new(&[cancel_ix], Some(&maker.pubkey()));
        let recent_blockhash = svm.latest_blockhash();
        let transaction = Transaction::new(&[&maker], message, recent_blockhash);

        let tx = svm.send_transaction(transaction).unwrap();
        println!("\n\nCancel transaction successful");
        println!("CUs Consumed: {}", tx.compute_units_consumed);

        let maker_a = spl_token_2022::state::Account::unpack(
            &svm.get_account(&maker_ata_a).unwrap().data,
        )
        .unwrap();
        assert_eq!(maker_a.amount, 1_000_000_000); // 500 back + 500 she kept

        assert_closed(&svm, &vault);
        assert_closed(&svm, &escrow);
    }    

    #[test]
    pub fn test_take_fails_when_taker_is_underfunded() {
        let MakeSetup {
            mut svm,
            maker,
            mint_a,
            mint_b,
            escrow,
            vault,
            amount_to_receive,
            amount_to_give,
            ..
        } = make_escrow();

        let taker = Keypair::new();
        svm.airdrop(&taker.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();

        let taker_ata_b = CreateAssociatedTokenAccount::new(&mut svm, &taker, &mint_b)
            .owner(&taker.pubkey())
            .send()
            .unwrap();
        MintTo::new(&mut svm, &maker, &mint_b, &taker_ata_b, amount_to_receive / 2)
            .send()
            .unwrap(); // 50 B

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
                AccountMeta::new(system_program_id(), false),
                AccountMeta::new(token_program_id(), false),
                AccountMeta::new(associated_token_program_id(), false),
            ],
            data: vec![1u8],
        };

        let message = Message::new(&[take_ix], Some(&taker.pubkey()));
        let recent_blockhash = svm.latest_blockhash();
        let transaction = Transaction::new(&[&taker], message, recent_blockhash);

        let result = svm.send_transaction(transaction);
        assert!(
            result.is_err(),
            "a taker without enough mint B must not take the escrow"
        );

        let vault_state = spl_token_2022::state::Account::unpack(
            &svm.get_account(&vault).unwrap().data,
        )
        .unwrap();
        assert_eq!(vault_state.amount, amount_to_give, "A must not have moved");
    }    

    #[test]
    pub fn test_cancel_fails_for_a_stranger() {
        let MakeSetup {
            mut svm,
            mint_a,
            escrow,
            vault,
            ..
        } = make_escrow();

        let stranger = Keypair::new();
        svm.airdrop(&stranger.pubkey(), 10 * LAMPORTS_PER_SOL).unwrap();
        let stranger_ata_a = CreateAssociatedTokenAccount::new(&mut svm, &stranger, &mint_a)
            .owner(&stranger.pubkey())
            .send()
            .unwrap();

        let cancel_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(stranger.pubkey(), true), // pretends to be maker
                AccountMeta::new(mint_a, false),
                AccountMeta::new(escrow, false),
                AccountMeta::new(vault, false),
                AccountMeta::new(stranger_ata_a, false),   // would receive the steal
                AccountMeta::new(token_program_id(), false),
            ],
            data: vec![2u8],
        };

        let message = Message::new(&[cancel_ix], Some(&stranger.pubkey()));
        let recent_blockhash = svm.latest_blockhash();
        let transaction = Transaction::new(&[&stranger], message, recent_blockhash);

        let result = svm.send_transaction(transaction);
        assert!(
            result.is_err(),
            "a stranger must not be able to cancel someone else's escrow"
        );

        let vault_state = spl_token_2022::state::Account::unpack(
            &svm.get_account(&vault).unwrap().data,
        )
        .unwrap();
        assert_eq!(vault_state.amount, 500_000_000, "A must still be in the vault");
    }    

}