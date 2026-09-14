// The Make instruction creates a new escrow and deposits Token A.
pub mod make;

// The Take instruction completes the trade between the maker and taker.
pub mod take;

// The Cancel instruction returns Token A to the original maker.
pub mod cancel;

// Re-export the instruction handlers so lib.rs can call them.
pub use make::*;
pub use take::*;
pub use cancel::*;

use pinocchio::error::ProgramError;

/// One-byte instruction discriminators.
///
/// The first byte of every instruction tells the program which
/// operation the user wants to perform.
pub enum EscrowInstructions {
    /// Creates an escrow and deposits Token A into its vault.
    Make = 0,

    /// Completes the escrow trade.
    Take = 1,

    /// Cancels the escrow and returns Token A to the maker.
    Cancel = 2,

    /// Reserved for a future version of Make.
    MakeV2 = 3,
}

/// Converts the first instruction byte into an EscrowInstructions value.
///
/// Unknown numbers are rejected instead of being treated as a valid
/// instruction.
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