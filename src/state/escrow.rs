use pinocchio::{account::RefMut, error::ProgramError, AccountView};

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Escrow {
    maker: [u8; 32],
    mint_a: [u8; 32],
    mint_b: [u8; 32],
    amount_to_receive: [u8; 8],
    amount_to_give: [u8; 8],
    pub bump: u8,
}

// The getters are not used by `Make` yet; `Take` and `Cancel` will need them.
impl Escrow {
    /// Total size of the account data: maker + mint_a + mint_b + amount_to_receive + amount_to_give + bump.
    /// Derived from the struct itself so it can never drift out of sync with the fields (113 bytes).
    pub const LEN: usize = core::mem::size_of::<Self>();

    /// Borrow the account data as a typed, mutable `Escrow` view.
    ///
    /// The returned `RefMut` keeps the account's borrow flag set for as long as it is
    /// alive, so the runtime (and Rust) will refuse any CPI or second borrow on this
    /// account until you drop it. Read what you need into locals, then let it go.
    pub fn load_mut(account: &mut AccountView) -> Result<RefMut<'_, Self>, ProgramError> {
        let data = account.try_borrow_mut()?;
        if data.len() != Escrow::LEN {
            return Err(ProgramError::InvalidAccountData);
        }
        if (data.as_ptr() as usize) % core::mem::align_of::<Self>() != 0 {
            return Err(ProgramError::InvalidAccountData);
        }
        // SAFETY: `#[repr(C)]`, alignment 1, and the length check above make the cast sound.
        // `RefMut::map` keeps the borrow guard alive, so this is the only borrow of the data.
        Ok(RefMut::map(data, |bytes| unsafe {
            &mut *(bytes.as_mut_ptr() as *mut Self)
        }))
    }

    pub fn maker(&self) -> pinocchio::Address {
        pinocchio::Address::from(self.maker)
    }

    pub fn set_maker(&mut self, maker: &pinocchio::Address) {
        self.maker.copy_from_slice(maker.as_ref());
    }

    pub fn mint_a(&self) -> pinocchio::Address {
        pinocchio::Address::from(self.mint_a)
    }

    pub fn set_mint_a(&mut self, mint_a: &pinocchio::Address) {
        self.mint_a.copy_from_slice(mint_a.as_ref());
    }

    pub fn mint_b(&self) -> pinocchio::Address {
        pinocchio::Address::from(self.mint_b)
    }

    pub fn set_mint_b(&mut self, mint_b: &pinocchio::Address) {
        self.mint_b.copy_from_slice(mint_b.as_ref());
    }

    pub fn amount_to_receive(&self) -> u64 {
        u64::from_le_bytes(self.amount_to_receive)
    }

    pub fn set_amount_to_receive(&mut self, amount: u64) {
        self.amount_to_receive = amount.to_le_bytes();
    }

    pub fn amount_to_give(&self) -> u64 {
        u64::from_le_bytes(self.amount_to_give)
    }

    pub fn set_amount_to_give(&mut self, amount: u64) {
        self.amount_to_give = amount.to_le_bytes();
    }
}
