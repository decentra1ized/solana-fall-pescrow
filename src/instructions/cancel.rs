use pinocchio::{AccountView, ProgramResult};

pub fn process_cancel_instruction(_accounts: &[AccountView], _data: &[u8]) -> ProgramResult {
    Ok(())
}
