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

    /// The maker is minted this much of mint A before Make runs.
    const MAKER_INITIAL_A: u64 = 1_000_000_000;
    /// 100 tokens of mint B, with 6 decimal places.
    const AMOUNT_TO_RECEIVE: u64 = 100_000_000;
    /// 500 tokens of mint A, with 6 decimal places.
    const AMOUNT_TO_GIVE: u64 = 500_000_000;

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

        #[allow(deprecated)]
        svm.set_sysvar(&solana_rent::Rent {
            lamports_per_byte_year: 6960,
            exemption_threshold: 1.0,
            burn_percent: 50,
        });

        svm
            .airdrop(&payer.pubkey(), 10 * LAMPORTS_PER_SOL)
            .expect("Airdrop failed");

        let so_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/deploy/escrow.so");

        let program_data = std::fs::read(&so_path)
            .unwrap_or_else(|e| panic!("Failed to read program SO file at {}: {e}. Run `cargo build-sbf` first.", so_path.display()));

        svm.add_program(program_id(), &program_data).expect("Failed to add program");

        (svm, payer)

    }

    /// Everything a Take or Cancel test needs after Make has run.
    struct EscrowFixture {
        svm: LiteSVM,
        maker: Keypair,
        mint_a: Pubkey,
        mint_b: Pubkey,
        maker_ata_a: Pubkey,
        escrow: Pubkey,
        bump: u8,
        vault: Pubkey,
    }

    /// Create both mints, fund the maker with mint A, and send Make.
    ///
    /// It reuses `setup()`, so the Rent sysvar override stays in place. Without
    /// that override the Make step fails with `InsufficientFundsForRent`.
    fn make_escrow() -> EscrowFixture {
        let (mut svm, maker) = setup();

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

        MintTo::new(&mut svm, &maker, &mint_a, &maker_ata_a, MAKER_INITIAL_A)
            .send()
            .unwrap();

        let (escrow, bump) = Pubkey::find_program_address(
            &[b"escrow".as_ref(), maker.pubkey().as_ref()],
            &program_id(),
        );

        let vault = spl_associated_token_account::get_associated_token_address(&escrow, &mint_a);

        let make_data = [
            vec![0u8],              // Discriminator for "Make"
            AMOUNT_TO_RECEIVE.to_le_bytes().to_vec(),
            AMOUNT_TO_GIVE.to_le_bytes().to_vec(),
        ].concat();

        let make_ix = Instruction {
            program_id: program_id(),
            accounts: vec![
                AccountMeta::new(maker.pubkey(), true),
                AccountMeta::new_readonly(mint_a, false),
                AccountMeta::new_readonly(mint_b, false),
                AccountMeta::new(escrow, false),
                AccountMeta::new(maker_ata_a, false),
                AccountMeta::new(vault, false),
                AccountMeta::new_readonly(system_program(), false),
                AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
                AccountMeta::new_readonly(associated_token_program(), false),
            ],
            data: make_data,
        };

        let message = Message::new(&[make_ix], Some(&maker.pubkey()));
        let blockhash = svm.latest_blockhash();
        let tx = svm.send_transaction(Transaction::new(&[&maker], message, blockhash)).unwrap();

        println!("Make CUs Consumed: {}", tx.compute_units_consumed);

        EscrowFixture { svm, maker, mint_a, mint_b, maker_ata_a, escrow, bump, vault }
    }

    /// Read an SPL token account balance.
    fn token_balance(svm: &LiteSVM, ata: &Pubkey) -> u64 {
        let acc = svm.get_account(ata).unwrap_or_else(|| panic!("token account {ata} not found"));
        spl_token_2022::state::Account::unpack(&acc.data).unwrap().amount
    }

    /// A closed account is either gone, or left with no lamports and no data.
    fn assert_closed(svm: &LiteSVM, key: &Pubkey, label: &str) {
        match svm.get_account(key) {
            None => println!("{label} is closed (account removed)"),
            Some(acc) => {
                assert_eq!(acc.lamports, 0, "{label} still holds lamports");
                assert!(acc.data.is_empty(), "{label} still holds data");
                println!("{label} is closed (0 lamports, 0 bytes)");
            }
        }
    }

    /// Give a keypair SOL and an associated token account holding `amount` of `mint`.
    fn fund_with_tokens(
        svm: &mut LiteSVM,
        mint_authority: &Keypair,
        owner: &Keypair,
        mint: &Pubkey,
        amount: u64,
    ) -> Pubkey {
        svm.airdrop(&owner.pubkey(), 10 * LAMPORTS_PER_SOL).expect("Airdrop failed");

        let ata = CreateAssociatedTokenAccount::new(svm, owner, mint)
            .owner(&owner.pubkey())
            .send()
            .unwrap();

        if amount > 0 {
            MintTo::new(svm, mint_authority, mint, &ata, amount).send().unwrap();
        }

        ata
    }

    /// The twelve Take accounts, in the order `take.rs` destructures them.
    fn take_accounts(
        taker: &Pubkey,
        maker: &Pubkey,
        mint_a: &Pubkey,
        mint_b: &Pubkey,
        escrow: &Pubkey,
        vault: &Pubkey,
        taker_ata_a: &Pubkey,
        taker_ata_b: &Pubkey,
        maker_ata_b: &Pubkey,
    ) -> Vec<AccountMeta> {
        vec![
            AccountMeta::new(*taker, true),
            AccountMeta::new(*maker, false),
            AccountMeta::new_readonly(*mint_a, false),
            AccountMeta::new_readonly(*mint_b, false),
            AccountMeta::new(*escrow, false),
            AccountMeta::new(*vault, false),
            AccountMeta::new(*taker_ata_a, false),
            AccountMeta::new(*taker_ata_b, false),
            AccountMeta::new(*maker_ata_b, false),
            AccountMeta::new_readonly(system_program(), false),
            AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
            AccountMeta::new_readonly(associated_token_program(), false),
        ]
    }

    /// The six Cancel accounts, in the order `cancel.rs` destructures them.
    fn cancel_accounts(
        maker: &Pubkey,
        mint_a: &Pubkey,
        escrow: &Pubkey,
        vault: &Pubkey,
        maker_ata_a: &Pubkey,
    ) -> Vec<AccountMeta> {
        vec![
            AccountMeta::new(*maker, true),
            AccountMeta::new_readonly(*mint_a, false),
            AccountMeta::new(*escrow, false),
            AccountMeta::new(*vault, false),
            AccountMeta::new(*maker_ata_a, false),
            AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
        ]
    }

    #[test]
    pub fn test_make_instruction() {
        assert_eq!(program_id().to_string(), PROGRAM_ID);

        let f = make_escrow();

        println!("Mint A: {}", f.mint_a);
        println!("Mint B: {}", f.mint_b);
        println!("Maker ATA A: {}", f.maker_ata_a);
        println!("Escrow PDA: {}", f.escrow);
        println!("Vault PDA: {}", f.vault);
        println!("Bump: {}", f.bump);

        let vault_acc = f.svm.get_account(&f.vault).unwrap();
        let vault_state = spl_token_2022::state::Account::unpack(&vault_acc.data).unwrap();
        println!("Vault owner: {} (escrow PDA? {})", vault_state.owner, vault_state.owner == f.escrow);
        println!("Vault balance: {}", vault_state.amount);
        assert_eq!(vault_state.owner, f.escrow);
        assert_eq!(vault_state.amount, AMOUNT_TO_GIVE);

        println!("Maker ATA balance: {}", token_balance(&f.svm, &f.maker_ata_a));
        assert_eq!(token_balance(&f.svm, &f.maker_ata_a), MAKER_INITIAL_A - AMOUNT_TO_GIVE);

        let esc = f.svm.get_account(&f.escrow).unwrap();
        println!("Escrow account owner: {} (program? {})", esc.owner, esc.owner == program_id());
        println!("Escrow data len: {}", esc.data.len());
        let d = &esc.data;
        println!("  maker   = {}", Pubkey::new_from_array(d[0..32].try_into().unwrap()));
        println!("  mint_a  = {}", Pubkey::new_from_array(d[32..64].try_into().unwrap()));
        println!("  mint_b  = {}", Pubkey::new_from_array(d[64..96].try_into().unwrap()));
        println!("  receive = {}", u64::from_le_bytes(d[96..104].try_into().unwrap()));
        println!("  give    = {}", u64::from_le_bytes(d[104..112].try_into().unwrap()));
        println!("  bump    = {}", d[112]);
        assert_eq!(&d[0..32], f.maker.pubkey().as_ref());
        assert_eq!(&d[32..64], f.mint_a.as_ref());
        assert_eq!(&d[64..96], f.mint_b.as_ref());
        assert_eq!(u64::from_le_bytes(d[96..104].try_into().unwrap()), AMOUNT_TO_RECEIVE);
        assert_eq!(u64::from_le_bytes(d[104..112].try_into().unwrap()), AMOUNT_TO_GIVE);
        assert_eq!(d[112], f.bump);
    }

    #[test]
    pub fn test_take_instruction() {
        let mut f = make_escrow();
        let taker = Keypair::new();

        // The taker holds mint B only. The program creates the two destination
        // token accounts with CreateIdempotent, so the test does not create them.
        let taker_ata_b = fund_with_tokens(&mut f.svm, &f.maker, &taker, &f.mint_b, AMOUNT_TO_RECEIVE);
        let taker_ata_a = spl_associated_token_account::get_associated_token_address(&taker.pubkey(), &f.mint_a);
        let maker_ata_b = spl_associated_token_account::get_associated_token_address(&f.maker.pubkey(), &f.mint_b);

        let maker_lamports_before = f.svm.get_account(&f.maker.pubkey()).unwrap().lamports;
        let vault_rent = f.svm.get_account(&f.vault).unwrap().lamports;
        let escrow_rent = f.svm.get_account(&f.escrow).unwrap().lamports;

        let take_ix = Instruction {
            program_id: program_id(),
            accounts: take_accounts(
                &taker.pubkey(), &f.maker.pubkey(), &f.mint_a, &f.mint_b,
                &f.escrow, &f.vault, &taker_ata_a, &taker_ata_b, &maker_ata_b,
            ),
            data: vec![1u8],
        };

        let message = Message::new(&[take_ix], Some(&taker.pubkey()));
        let blockhash = f.svm.latest_blockhash();
        let tx = f.svm.send_transaction(Transaction::new(&[&taker], message, blockhash)).unwrap();

        println!("\nTake transaction successful");
        println!("Take CUs Consumed: {}", tx.compute_units_consumed);

        println!("Taker ATA A balance: {}", token_balance(&f.svm, &taker_ata_a));
        assert_eq!(token_balance(&f.svm, &taker_ata_a), AMOUNT_TO_GIVE);

        println!("Maker ATA B balance: {}", token_balance(&f.svm, &maker_ata_b));
        assert_eq!(token_balance(&f.svm, &maker_ata_b), AMOUNT_TO_RECEIVE);

        println!("Taker ATA B balance: {}", token_balance(&f.svm, &taker_ata_b));
        assert_eq!(token_balance(&f.svm, &taker_ata_b), 0);

        assert_closed(&f.svm, &f.vault, "Vault");
        assert_closed(&f.svm, &f.escrow, "Escrow");

        // The maker paid the rent of both accounts in Make and gets it back here.
        let maker_lamports_after = f.svm.get_account(&f.maker.pubkey()).unwrap().lamports;
        println!(
            "Maker lamports: {} -> {} (refund {}, vault rent {} + escrow rent {})",
            maker_lamports_before, maker_lamports_after,
            maker_lamports_after - maker_lamports_before, vault_rent, escrow_rent,
        );
        assert_eq!(maker_lamports_after, maker_lamports_before + vault_rent + escrow_rent);
    }

    #[test]
    pub fn test_cancel_instruction() {
        let mut f = make_escrow();

        let maker_lamports_before = f.svm.get_account(&f.maker.pubkey()).unwrap().lamports;
        let vault_rent = f.svm.get_account(&f.vault).unwrap().lamports;
        let escrow_rent = f.svm.get_account(&f.escrow).unwrap().lamports;

        let cancel_ix = Instruction {
            program_id: program_id(),
            accounts: cancel_accounts(&f.maker.pubkey(), &f.mint_a, &f.escrow, &f.vault, &f.maker_ata_a),
            data: vec![2u8],
        };

        let message = Message::new(&[cancel_ix], Some(&f.maker.pubkey()));
        let blockhash = f.svm.latest_blockhash();
        let tx = f.svm.send_transaction(Transaction::new(&[&f.maker], message, blockhash)).unwrap();

        println!("\nCancel transaction successful");
        println!("Cancel CUs Consumed: {}", tx.compute_units_consumed);

        println!("Maker ATA A balance: {}", token_balance(&f.svm, &f.maker_ata_a));
        assert_eq!(token_balance(&f.svm, &f.maker_ata_a), MAKER_INITIAL_A);

        assert_closed(&f.svm, &f.vault, "Vault");
        assert_closed(&f.svm, &f.escrow, "Escrow");

        // The maker paid the transaction fee, so compare against the refund only.
        let maker_lamports_after = f.svm.get_account(&f.maker.pubkey()).unwrap().lamports;
        println!(
            "Maker lamports: {} -> {} (rent refunded {} + {})",
            maker_lamports_before, maker_lamports_after, vault_rent, escrow_rent,
        );
        assert!(maker_lamports_after > maker_lamports_before);
    }

    #[test]
    pub fn test_take_fails_when_taker_is_underfunded() {
        let mut f = make_escrow();
        let taker = Keypair::new();

        // Half of what the escrow asks for.
        let underfunded = AMOUNT_TO_RECEIVE / 2;
        let taker_ata_b = fund_with_tokens(&mut f.svm, &f.maker, &taker, &f.mint_b, underfunded);
        let taker_ata_a = spl_associated_token_account::get_associated_token_address(&taker.pubkey(), &f.mint_a);
        let maker_ata_b = spl_associated_token_account::get_associated_token_address(&f.maker.pubkey(), &f.mint_b);

        let take_ix = Instruction {
            program_id: program_id(),
            accounts: take_accounts(
                &taker.pubkey(), &f.maker.pubkey(), &f.mint_a, &f.mint_b,
                &f.escrow, &f.vault, &taker_ata_a, &taker_ata_b, &maker_ata_b,
            ),
            data: vec![1u8],
        };

        let message = Message::new(&[take_ix], Some(&taker.pubkey()));
        let blockhash = f.svm.latest_blockhash();
        let result = f.svm.send_transaction(Transaction::new(&[&taker], message, blockhash));

        println!("\nUnderfunded Take result: {:?}", result.as_ref().err().map(|e| e.err.to_string()));
        assert!(result.is_err(), "a taker without enough of mint B must not be able to take");

        // The whole transaction rolled back, so nothing moved.
        assert_eq!(token_balance(&f.svm, &f.vault), AMOUNT_TO_GIVE, "vault must keep mint A");
        assert_eq!(token_balance(&f.svm, &taker_ata_b), underfunded, "taker must keep mint B");
        assert!(f.svm.get_account(&f.escrow).is_some(), "escrow must stay open");
    }

    #[test]
    pub fn test_cancel_by_stranger_fails() {
        let mut f = make_escrow();
        let stranger = Keypair::new();

        // The stranger signs, and points the drain at their own token account.
        // Only the stored-maker check stands between them and the vault.
        let stranger_ata_a = fund_with_tokens(&mut f.svm, &f.maker, &stranger, &f.mint_a, 0);

        let cancel_ix = Instruction {
            program_id: program_id(),
            accounts: cancel_accounts(&stranger.pubkey(), &f.mint_a, &f.escrow, &f.vault, &stranger_ata_a),
            data: vec![2u8],
        };

        let message = Message::new(&[cancel_ix], Some(&stranger.pubkey()));
        let blockhash = f.svm.latest_blockhash();
        let result = f.svm.send_transaction(Transaction::new(&[&stranger], message, blockhash));

        println!("\nStranger Cancel result: {:?}", result.as_ref().err().map(|e| e.err.to_string()));
        assert!(result.is_err(), "a stranger must not be able to cancel someone else's escrow");

        assert_eq!(token_balance(&f.svm, &f.vault), AMOUNT_TO_GIVE, "vault must keep mint A");
        assert_eq!(token_balance(&f.svm, &stranger_ata_a), 0, "the stranger must receive nothing");
        assert!(f.svm.get_account(&f.escrow).is_some(), "escrow must stay open");
    }

    #[test]
    pub fn test_take_with_wrong_maker_fails() {
        let mut f = make_escrow();
        let taker = Keypair::new();
        let stranger = Keypair::new();

        let taker_ata_b = fund_with_tokens(&mut f.svm, &f.maker, &taker, &f.mint_b, AMOUNT_TO_RECEIVE);
        let taker_ata_a = spl_associated_token_account::get_associated_token_address(&taker.pubkey(), &f.mint_a);
        // The payment is aimed at a stranger instead of the stored maker.
        let stranger_ata_b = spl_associated_token_account::get_associated_token_address(&stranger.pubkey(), &f.mint_b);

        let take_ix = Instruction {
            program_id: program_id(),
            accounts: take_accounts(
                &taker.pubkey(), &stranger.pubkey(), &f.mint_a, &f.mint_b,
                &f.escrow, &f.vault, &taker_ata_a, &taker_ata_b, &stranger_ata_b,
            ),
            data: vec![1u8],
        };

        let message = Message::new(&[take_ix], Some(&taker.pubkey()));
        let blockhash = f.svm.latest_blockhash();
        let result = f.svm.send_transaction(Transaction::new(&[&taker], message, blockhash));

        println!("\nWrong-maker Take result: {:?}", result.as_ref().err().map(|e| e.err.to_string()));
        assert!(result.is_err(), "Take must reject a maker that does not match the escrow state");

        assert_eq!(token_balance(&f.svm, &f.vault), AMOUNT_TO_GIVE, "vault must keep mint A");
        assert_eq!(token_balance(&f.svm, &taker_ata_b), AMOUNT_TO_RECEIVE, "taker must keep mint B");
        assert!(f.svm.get_account(&f.escrow).is_some(), "escrow must stay open");
    }
}
