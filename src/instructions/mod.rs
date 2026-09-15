pub mod cancel;
pub mod make;
pub mod take;

pub use cancel::*;
pub use make::*;
pub use take::*;
use pinocchio::{AccountView, ProgramResult, error::ProgramError};

pub enum EscrowInstructions {
    Make = 0,
    Take = 1,
    Cancel = 2,
    MakeV2 = 3,
}

impl TryFrom<&u8> for EscrowInstructions {
    type Error = ProgramError;

    fn try_from(value: &u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(EscrowInstructions::Make),
            1 => Ok(EscrowInstructions::Take),
            2 => Ok(EscrowInstructions::Cancel),
            3 => Ok(EscrowInstructions::MakeV2),
            _ => Err(ProgramError::InvalidInstructionData),
        }
    }
}

/// Close a program-owned account: drain its lamports into `destination`, then zero out
/// its data length, lamports and owner.
///
/// The lamports have to move *before* the close, otherwise the runtime rejects the
/// instruction for an unbalanced lamport total. `close()` also refuses to run while any
/// borrow on the account data is alive, so every `Escrow::load_mut` guard must be dropped
/// by the time we get here.
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
