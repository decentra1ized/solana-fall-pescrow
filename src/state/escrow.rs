use pinocchio::{AccountView, account::{Ref, RefMut}, error::ProgramError};

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

impl Escrow {
    /// Derived from the struct itself so it can never drift out of sync with the fields (113 bytes).
    pub const LEN: usize = core::mem::size_of::<Self>();

    fn check_len(len: usize) -> Result<(), ProgramError> {
        if len != Self::LEN {
            return Err(ProgramError::InvalidAccountData);
        }
        Ok(())
    }

    /// Read-only view; use when the caller only inspects fields (`Take`, `Cancel`).
    pub fn load(account: &AccountView) -> Result<Ref<'_, Self>, ProgramError> {
        let data = account.try_borrow()?;
        Self::check_len(data.len())?;
        // SAFETY: `#[repr(C)]` and the length check above make the cast sound; alignment
        // is 1 so no pointer-alignment check is needed. `Ref::map` keeps the borrow guard
        // alive, so no mutable borrow of the data can coexist with this one.
        Ok(Ref::map(data, |bytes| unsafe { &*(bytes.as_ptr() as *const Self) }))
    }

    /// Mutable view; the returned guard holds the account's borrow flag until dropped,
    /// so no CPI or second borrow on this account can happen while it is alive.
    pub fn load_mut(account: &mut AccountView) -> Result<RefMut<'_, Self>, ProgramError> {
        let data = account.try_borrow_mut()?;
        Self::check_len(data.len())?;
        // SAFETY: see `load` above; `RefMut::map` keeps the borrow guard alive.
        Ok(RefMut::map(data, |bytes| unsafe { &mut *(bytes.as_mut_ptr() as *mut Self) }))
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

    #[allow(dead_code)]
    pub fn amount_to_give(&self) -> u64 {
        u64::from_le_bytes(self.amount_to_give)
    }

    pub fn set_amount_to_give(&mut self, amount: u64) {
        self.amount_to_give = amount.to_le_bytes();
    }
}