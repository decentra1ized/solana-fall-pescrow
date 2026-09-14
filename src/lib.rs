#![allow(unexpected_cfgs)]

use pinocchio::{
    address::declare_id,
    entrypoint,
    error::ProgramError,
    AccountView,
    Address,
    ProgramResult,
};

use crate::instructions::EscrowInstructions;

mod instructions;
mod state;
mod tests;

// Registers process_instruction as the program's Solana entrypoint.
entrypoint!(process_instruction);

// The public address of this escrow program.
declare_id!("4ibrEMW5F6hKnkW4jVedswYv6H6VtwPN6ar6dvXDN1nT");

/// Receives every instruction sent to the escrow program.
///
/// The first byte is the instruction discriminator:
///
/// 0 = Make
/// 1 = Take
/// 2 = Cancel
/// 3 = MakeV2
///
/// The remaining bytes contain any instruction-specific data.
pub fn process_instruction(
    program_id: &Address,
    accounts: &mut [AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    // Confirm this instruction was sent to the correct program.
    if program_id != &ID {
        return Err(ProgramError::IncorrectProgramId);
    }

    // Separate the discriminator from any remaining instruction data.
    //
    // Empty instruction data is rejected because there would be no
    // operation for the program to perform.
    let (discriminator, data) = instruction_data
        .split_first()
        .ok_or(ProgramError::InvalidInstructionData)?;

    // Send the accounts and remaining data to the correct handler.
    match EscrowInstructions::try_from(discriminator)? {
        EscrowInstructions::Make => {
            instructions::process_make_instruction(accounts, data)?;
        }

        EscrowInstructions::Take => {
            instructions::process_take_instruction(accounts, data)?;
        }

        EscrowInstructions::Cancel => {
            instructions::process_cancel_instruction(accounts, data)?;
        }

        // MakeV2 exists in the instruction enum but is not part of this
        // assignment. Reject it explicitly so it cannot be mistaken for
        // an implemented instruction.
        EscrowInstructions::MakeV2 => {
            return Err(ProgramError::InvalidInstructionData);
        }
    }

    Ok(())
}