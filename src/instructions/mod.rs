pub mod make;
pub mod take;
pub mod cancel;
pub use take::*;
pub use cancel::*;

pub use make::*;
use pinocchio::error::ProgramError;

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