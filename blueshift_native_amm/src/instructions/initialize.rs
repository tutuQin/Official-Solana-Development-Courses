use core::mem::{size_of, MaybeUninit};

use pinocchio::{
    account_info::AccountInfo,
    instruction::{Seed, Signer},
    program_error::ProgramError,
    sysvars::{rent::Rent, Sysvar},
    ProgramResult,
};
use pinocchio_token::instructions::InitializeMint2;

use crate::state::Config;

// ─── Accounts ───────────────────────────────────────────────────────────────

pub struct InitializeAccounts<'a> {
    pub initializer: &'a AccountInfo,
    pub mint_lp: &'a AccountInfo,
    pub config: &'a AccountInfo,
    pub system_program: &'a AccountInfo,
    pub token_program: &'a AccountInfo,
}

impl<'a> TryFrom<&'a [AccountInfo]> for InitializeAccounts<'a> {
    type Error = ProgramError;
    fn try_from(accounts: &'a [AccountInfo]) -> Result<Self, Self::Error> {
        let [initializer, mint_lp, config, system_program, token_program, ..] = accounts else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };
        Ok(Self {
            initializer,
            mint_lp,
            config,
            system_program,
            token_program,
        })
    }
}

// ─── Instruction Data ───────────────────────────────────────────────────────

#[repr(C, packed)]
pub struct InitializeInstructionData {
    pub seed: u64,
    pub fee: u16,
    pub mint_x: [u8; 32],
    pub mint_y: [u8; 32],
    pub config_bump: [u8; 1],
    pub lp_bump: [u8; 1],
    pub authority: [u8; 32],
}

impl TryFrom<&[u8]> for InitializeInstructionData {
    type Error = ProgramError;
    fn try_from(data: &[u8]) -> Result<Self, Self::Error> {
        const INITIALIZE_DATA_LEN_WITH_AUTHORITY: usize = size_of::<InitializeInstructionData>();
        const INITIALIZE_DATA_LEN: usize =
            INITIALIZE_DATA_LEN_WITH_AUTHORITY - size_of::<[u8; 32]>();

        match data.len() {
            INITIALIZE_DATA_LEN_WITH_AUTHORITY => {
                Ok(unsafe { (data.as_ptr() as *const Self).read_unaligned() })
            }
            INITIALIZE_DATA_LEN => {
                let mut raw: MaybeUninit<[u8; INITIALIZE_DATA_LEN_WITH_AUTHORITY]> =
                    MaybeUninit::uninit();
                let raw_ptr = raw.as_mut_ptr() as *mut u8;
                unsafe {
                    core::ptr::copy_nonoverlapping(data.as_ptr(), raw_ptr, INITIALIZE_DATA_LEN);
                    core::ptr::write_bytes(raw_ptr.add(INITIALIZE_DATA_LEN), 0, 32);
                    Ok((raw.as_ptr() as *const Self).read_unaligned())
                }
            }
            _ => Err(ProgramError::InvalidInstructionData),
        }
    }
}

// ─── Instruction ────────────────────────────────────────────────────────────

pub struct Initialize<'a> {
    pub accounts: InitializeAccounts<'a>,
    pub instruction_data: InitializeInstructionData,
}

impl<'a> TryFrom<(&'a [u8], &'a [AccountInfo])> for Initialize<'a> {
    type Error = ProgramError;
    fn try_from((data, accounts): (&'a [u8], &'a [AccountInfo])) -> Result<Self, Self::Error> {
        let accounts = InitializeAccounts::try_from(accounts)?;
        let instruction_data = InitializeInstructionData::try_from(data)?;
        Ok(Self {
            accounts,
            instruction_data,
        })
    }
}

impl<'a> Initialize<'a> {
    pub const DISCRIMINATOR: &'a u8 = &0;

    pub fn process(&mut self) -> ProgramResult {
        let rent = Rent::get()?;

        // 1. Create Config account
        let seed_binding = self.instruction_data.seed.to_le_bytes();
        let config_seeds = [
            Seed::from(b"config"),
            Seed::from(&seed_binding),
            Seed::from(&self.instruction_data.mint_x),
            Seed::from(&self.instruction_data.mint_y),
            Seed::from(&self.instruction_data.config_bump),
        ];
        let config_signer = Signer::from(&config_seeds);

        pinocchio_system::instructions::CreateAccount {
            from: self.accounts.initializer,
            to: self.accounts.config,
            lamports: rent.minimum_balance(Config::LEN),
            space: Config::LEN as u64,
            owner: &crate::ID,
        }
        .invoke_signed(&[config_signer])?;

        // 2. Populate Config
        let config = unsafe { Config::load_mut(self.accounts.config)? };
        config.set_inner(
            self.instruction_data.seed,
            self.instruction_data.authority,
            self.instruction_data.mint_x,
            self.instruction_data.mint_y,
            self.instruction_data.fee,
            self.instruction_data.config_bump,
        )?;

        // 3. Create mint_lp account
        let mint_lp_seeds = [
            Seed::from(b"mint_lp"),
            Seed::from(self.accounts.config.key().as_ref()),
            Seed::from(&self.instruction_data.lp_bump),
        ];
        let mint_signer = Signer::from(&mint_lp_seeds);

        pinocchio_system::instructions::CreateAccount {
            from: self.accounts.initializer,
            to: self.accounts.mint_lp,
            lamports: rent.minimum_balance(82),
            space: 82u64,
            owner: &pinocchio_token::ID,
        }
        .invoke_signed(&[mint_signer])?;

        // 4. Initialize mint_lp with config as mint_authority
        InitializeMint2 {
            mint: self.accounts.mint_lp,
            decimals: 6,
            mint_authority: self.accounts.config.key(),
            freeze_authority: None,
        }
        .invoke()?;

        Ok(())
    }
}
