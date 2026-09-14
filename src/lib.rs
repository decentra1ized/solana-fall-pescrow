#![allow(unexpected_cfgs)]
use pinocchio::{AccountView, entrypoint, Address, ProgramResult, address::declare_id, error::ProgramError};

use crate::instructions::EscrowInstructions;

mod tests;
mod state;
mod instructions;

entrypoint!(process_instruction);

declare_id!("4ibrEMW5F6hKnkW4jVedswYv6H6VtwPN6ar6dvXDN1nT");

pub fn process_instruction(
    program_id: &Address,
    accounts: &mut [AccountView],
    instruction_data: &[u8],
) -> ProgramResult {

    if program_id != &ID {
        return Err(ProgramError::IncorrectProgramId);
    }

    let (discriminator, data) = instruction_data.split_first()
        .ok_or(ProgramError::InvalidInstructionData)?;

    match EscrowInstructions::try_from(discriminator)? {
        EscrowInstructions::Make => instructions::process_make_instruction(accounts, data)?,
        EscrowInstructions::Take => instructions::process_take_instruction(accounts)?,
        EscrowInstructions::Cancel => instructions::process_cancel_instruction(accounts)?,
        EscrowInstructions::MakeV2 => return Err(ProgramError::InvalidInstructionData),
    }
    Ok(())
}