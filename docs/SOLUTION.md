# Solution notes: Take, Cancel, and the tests that matter

> Companion to the assignment [README](../README.md). The README says what to build; this
> file says what was built, and why each check is where it is.
>
> Everything here lives in three files: [`src/instructions/take.rs`](../src/instructions/take.rs),
> [`src/instructions/cancel.rs`](../src/instructions/cancel.rs), and the tests in
> [`src/tests/mod.rs`](../src/tests/mod.rs), plus a shared helper in
> [`src/instructions/mod.rs`](../src/instructions/mod.rs).

## Results

| Instruction | Discriminator | Accounts | Compute units |
| ----------- | ------------- | -------- | ------------- |
| `Make`      | `0`           | 9        | ~33k          |
| `Take`      | `1`           | 12       | ~62k          |
| `Cancel`    | `2`           | 6        | ~10k          |

Seven tests pass (`test_id`, Make, Take, Cancel, and three negatives). No new `unsafe`:
the only `unsafe` block in the repo is still the pointer cast inside `Escrow::load_mut`.

Rejections are cheap because validation is ordered before any CPI — an unsigned Cancel
costs **119 CU** to refuse, a Take with a substituted maker **202 CU**.

---

## The shape of both instructions

Take and Cancel are the same program written at two different lengths. Both follow the
pipeline the lecture drew — *parse → check → derive → CPI* — and both end the same way:
drain the vault, close the vault, close the escrow state, rent back to the maker.

```
          Take                                Cancel
  ┌──────────────────────┐            ┌──────────────────────┐
  │ taker.is_signer()    │            │ maker.is_signer()    │  ← authorization
  ├──────────────────────┤            ├──────────────────────┤
  │ escrow.owned_by(ID)  │            │ escrow.owned_by(ID)  │  ← is this even ours?
  ├──────────────────────┤            ├──────────────────────┤
  │ stored maker/mints   │            │ stored maker == signer│ ← identity
  │   == passed accounts │            │ stored mint_a match   │
  ├──────────────────────┤            ├──────────────────────┤
  │ derive_address(bump) │            │ derive_address(bump) │  ← is this the right PDA?
  ├──────────────────────┤            ├──────────────────────┤
  │ vault owner + mint   │            │ vault owner + mint   │  ← is this the right vault?
  │ taker_ata_b owner    │            │ maker_ata_a owner    │
  ├──────────────────────┤            ├──────────────────────┤
  │ CreateIdempotent ×2  │            │          —           │
  │ B: taker → maker     │            │          —           │
  │ A: vault → taker     │            │ A: vault → maker     │  ← invoke_signed
  │ CloseAccount(vault)  │            │ CloseAccount(vault)  │  ← invoke_signed
  │ close escrow by hand │            │ close escrow by hand │
  └──────────────────────┘            └──────────────────────┘
```

The last three rows are identical, which is why the lamport-moving half of the close is
factored into one helper (below). The CPIs themselves are not factored: they differ in
destination and in how many there are, and inlining them keeps each instruction readable
top to bottom — which is the house style this repo already uses in `make.rs`.

---

## `Take`

### Account order

Positional, no names. The client must match this exactly; the same order is mirrored by
the `take_accounts()` helper in the tests, so the two cannot drift apart silently.

| #  | Account                   | W | S | Notes                                              |
| -- | ------------------------- | - | - | -------------------------------------------------- |
| 0  | `taker`                   | ✓ | ✓ | Pays fees, funds any ATA that has to be created     |
| 1  | `maker`                   | ✓ | — | Receives rent refunds; must equal `escrow.maker()`  |
| 2  | `mint_a`                  | — | — | Must equal `escrow.mint_a()`                        |
| 3  | `mint_b`                  | — | — | Must equal `escrow.mint_b()`                        |
| 4  | `escrow_account`          | ✓ | — | PDA `["escrow", maker, bump]`; closed here          |
| 5  | `vault`                   | ✓ | — | ATA(escrow PDA, mint A); closed here                |
| 6  | `taker_ata_a`             | ✓ | — | ATA(taker, mint A); created if missing              |
| 7  | `taker_ata_b`             | ✓ | — | ATA(taker, mint B); must exist and be funded        |
| 8  | `maker_ata_b`             | ✓ | — | ATA(maker, mint B); created if missing              |
| 9  | `system_program`          | — | — |                                                     |
| 10 | `token_program`           | — | — |                                                     |
| 11 | `associated_token_program`| — | — |                                                     |

### What each check is actually preventing

Every one of these is a door someone would otherwise walk through. Stated as the attack
it stops, rather than as a rule:

- **`taker.is_signer()`** — without it, anyone can settle a deal using someone else's
  token account as the source of payment.
- **`escrow_account.owned_by(&crate::ID)`** — this is the one that has to come *before*
  `Escrow::load_mut`. Zero-copy state is a pointer cast over raw account bytes; there is
  no discriminator and no ownership check hidden inside it. Hand the program an account
  it does not own and it will cheerfully read whatever bytes are there as maker, mints
  and bump. Anchor's `Account<'info, T>` did this check for you.
- **stored `maker` / `mint_a` / `mint_b` vs. the passed accounts** — the taker chooses
  which accounts to pass. Without the cross-check they can substitute an accomplice in
  the maker slot, redirect the asking price there, and still collect the vault. This
  costs nothing and closes an attack where every signature involved is valid.
- **`derive_address` vs. `escrow_account.address()`** — stops a substituted escrow
  account that happens to be owned by this program.
- **vault `owner()` and `mint()`** — the program is about to sign for this account with
  its own PDA authority. It has to be certain the vault is the one this escrow created,
  and that it holds mint A rather than something else.
- **`taker_ata_b` owner and mint** — the payment source is supplied, not created, so it
  gets checked like any other input.

### Details worth knowing

**The stored bump.** `Make` paid for `find_program_address` once — a loop that hashes
until it finds an off-curve address, a few thousand CU — and wrote the canonical bump
into the escrow state. `Take` and `Cancel` read that bump back and call
`pinocchio_pubkey::derive_address`, a single `sha256` syscall:

```rust
let bump_seed = [bump];
let derived_escrow = pinocchio_pubkey::derive_address(
    &[b"escrow".as_ref(), maker.address().as_ref(), &bump_seed],
    None,
    &crate::ID.to_bytes(),
);
if derived_escrow != *escrow_account.address().as_array() {
    return Err(ProgramError::InvalidSeeds);
}
```

This is safe *only* because `Make` derived the canonical bump on-chain rather than
accepting one from the client. A client-supplied bump would let one maker open several
"the" escrows at different non-canonical addresses.

**Scoping the `RefMut`.** `Escrow::load_mut` returns a `RefMut<Escrow>`, not `&mut Escrow`.
The guard keeps the account's borrow flag set, and the runtime refuses any CPI on a
borrowed account — `close()` refuses too. So the state is read inside a block and only
`Copy` primitives escape:

```rust
let (amount_to_receive, bump) = {
    let escrow_state = Escrow::load_mut(escrow_account)?;
    // ... cross-checks against maker / mint_a / mint_b ...
    (escrow_state.amount_to_receive(), escrow_state.bump)
};
```

Forget the braces and the first CPI fails with `AccountBorrowFailed`, at runtime, with
nothing in the compiler to warn you. This is the single most common way to get stuck on
this assignment.

**Idempotent ATA creation.** `taker_ata_a` and `maker_ata_b` may not exist yet — the
taker has never held mint A, the maker may have never held mint B. `CreateIdempotent`
creates them if missing and is a no-op otherwise, funded by the taker. The ATA program
derives the address itself and rejects a mismatch, so no separate address check is
needed for these two.

**Three CPIs, two authorities.** The payment leg is authorized by the taker, who signed
the transaction, so it is a plain `.invoke()`. The two vault legs are authorized by the
escrow PDA, so they are `.invoke_signed(&[signer])` with seeds
`["escrow", maker, bump]`. Note `multisig_signers: &[] as &[&AccountView]` — new in
pinocchio-token 0.6, no default, required on every token CPI.

---

## `Cancel`

Six accounts: `maker` (writable, signer), `mint_a`, `escrow_account`, `vault`,
`maker_ata_a`, `token_program`. Same pipeline, one counterparty, no ATA creation — hence
~10k CU against Take's ~62k.

The thing worth saying about Cancel is that its authorization is **two checks, not one**,
and each is useless without the other:

```rust
if !maker.is_signer() {
    return Err(ProgramError::MissingRequiredSignature);
}
// ...
if escrow_state.maker() != *maker.address() {
    return Err(ProgramError::InvalidAccountOwner);
}
```

- Drop the signer check and anyone can pass the real maker's account, unsigned, and
  trigger a cancellation the maker never authorized.
- Drop the identity check and a stranger can sign in the maker slot, point
  `maker_ata_a` at their own token account, and drain someone else's escrow.

Both are exercised separately in `test_cancel_fails_for_a_stranger`, because a test that
only covers one of them would still pass with the other deleted.

`maker_ata_a` is validated (owner and mint) for the same reason `taker_ata_b` is in Take:
it is a destination the caller chose, not one the program derived.

---

## The shared close helper

Closing a program-owned account by hand has an ordering requirement that is easy to get
backwards, so it is written once, in `instructions/mod.rs`, and called by both:

```rust
pub fn close_program_account(
    account: &mut AccountView,
    destination: &mut AccountView,
) -> ProgramResult {
    let refund = account.lamports();
    let new_destination_balance = destination
        .lamports()
        .checked_add(refund)
        .ok_or(ProgramError::ArithmeticOverflow)?;

    destination.set_lamports(new_destination_balance);
    account.set_lamports(0);
    account.close()
}
```

The lamports must move **before** `close()`. Close first and the runtime rejects the
whole instruction as unbalanced — lamports cannot appear or vanish inside an instruction,
only move. `close()` also refuses to run while any borrow on the account data is alive,
which is the `RefMut` scoping rule showing up a second time.

The vault is a *token* account, not a program-owned one, so it closes through the token
program's `CloseAccount` CPI instead. Only the escrow state account is closed by hand.

---

## Dispatch

Both instructions are wired into the match in `lib.rs`, and the wildcard arm is gone:

```rust
match EscrowInstructions::try_from(discriminator)? {
    EscrowInstructions::Make   => instructions::process_make_instruction(accounts, data)?,
    EscrowInstructions::Take   => instructions::process_take_instruction(accounts)?,
    EscrowInstructions::Cancel => instructions::process_cancel_instruction(accounts)?,
    // Reserved, not implemented yet: reject it by name rather than letting a
    // catch-all arm hide a future instruction that was never wired up.
    EscrowInstructions::MakeV2 => return Err(ProgramError::InvalidInstructionData),
}
```

`_ => Err(...)` would have worked identically today. The difference shows up later: with
an explicit arm per variant, adding a fifth instruction to the enum makes the compiler
demand it be handled. A wildcard would silently reject it at runtime instead, and the
test for the new instruction would fail somewhere far from the cause.

---

## Tests

`make_escrow()` factors the whole Make setup — two mints, the maker's ATA, 1000 A minted,
the Make transaction — into one fixture returning a `MadeEscrow` struct. Every other test
starts from a live deal. The Rent sysvar override stays inside `setup()`; without it every
test dies at Make with `InsufficientFundsForRent`, because LiteSVM 0.9 still ships the
pre-SIMD-0194 rent rate while pinocchio 0.11 computes exemption the folded way.

| Test | What it proves |
| ---- | -------------- |
| `test_make_instruction` | Shipped with the repo, unchanged |
| `test_take_instruction` | Taker gets 500 A, maker gets 100 B, both PDAs closed, maker's lamports up by *exactly* both rents |
| `test_cancel_instruction` | Maker's A balance goes 500 → 1000, both PDAs closed, lamports up by both rents less the 5000-lamport fee |
| `test_take_fails_when_taker_cannot_afford_price` | Rejected `Custom(1)` (`InsufficientFunds`); the vault is untouched |
| `test_cancel_fails_for_a_stranger` | Stranger-as-maker → `InvalidAccountOwner`; real maker unsigned → `MissingRequiredSignature`. Vault untouched both times |
| `test_take_fails_with_wrong_maker` | Substituted maker → `InvalidAccountData`; the impostor is never paid |

Two things the negative tests do deliberately:

**They assert on state, not just on `is_err()`.** A transaction can fail for the wrong
reason — a typo in the account list fails too. Reading the vault back and asserting it
still holds all 500 A is what distinguishes "the guard worked" from "the test was
malformed".

**The underfunded-taker test is really an atomicity test.** The escrow has no balance
check of its own; it asks the token program to move the asking price and lets that CPI
fail. So what is being verified is that a failure in CPI #1 rolls back the whole
instruction, rather than leaving the vault half-drained. That is a property of the
runtime, but it is worth pinning down, because the three CPIs are not individually
reversible.

All tests print compute units, failures included — the CU figure for a rejected
transaction comes off `failure.meta.compute_units_consumed`.

---

## Running it

```bash
cargo build-sbf          # writes target/deploy/escrow.so
cargo test -- --nocapture
```

The tests load the compiled `.so`, not the source, so the build has to come first. Edit a
`.rs` file, skip `cargo build-sbf`, and you will be testing the previous version of the
program — which produces failures that make no sense against the code in front of you.
