use core::mem::size_of;

use pinocchio::{
    account_info::AccountInfo,
    instruction::{Seed, Signer},
    program_error::ProgramError,
    pubkey::find_program_address,
    sysvars::{clock::Clock, Sysvar},
    ProgramResult,
};
use pinocchio_token::{instructions::Transfer, state::TokenAccount};

use constant_product_curve::{ConstantProduct, LiquidityPair};

use crate::state::{AmmState, Config};

// ─── Accounts ───────────────────────────────────────────────────────────────

pub struct SwapAccounts<'a> {
    pub user: &'a AccountInfo,
    pub user_x_ata: &'a AccountInfo,
    pub user_y_ata: &'a AccountInfo,
    pub vault_x: &'a AccountInfo,
    pub vault_y: &'a AccountInfo,
    pub config: &'a AccountInfo,
    pub token_program: &'a AccountInfo,
}

impl<'a> TryFrom<&'a [AccountInfo]> for SwapAccounts<'a> {
    type Error = ProgramError;
    fn try_from(accounts: &'a [AccountInfo]) -> Result<Self, Self::Error> {
        let [user, user_x_ata, user_y_ata, vault_x, vault_y, config, token_program, ..] = accounts
        else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };
        Ok(Self {
            user,
            user_x_ata,
            user_y_ata,
            vault_x,
            vault_y,
            config,
            token_program,
        })
    }
}

// ─── Instruction Data ───────────────────────────────────────────────────────

#[repr(C, packed)]
pub struct SwapInstructionData {
    pub is_x: u8,
    pub amount: u64,
    pub min: u64,
    pub expiration: i64,
}

impl<'a> TryFrom<&'a [u8]> for SwapInstructionData {
    type Error = ProgramError;
    fn try_from(data: &'a [u8]) -> Result<Self, Self::Error> {
        if data.len() != size_of::<Self>() {
            return Err(ProgramError::InvalidInstructionData);
        }
        let result = unsafe { (data.as_ptr() as *const Self).read_unaligned() };
        if result.amount == 0 || result.min == 0 {
            return Err(ProgramError::InvalidInstructionData);
        }
        let clock = Clock::get()?;
        if clock.unix_timestamp > result.expiration {
            return Err(ProgramError::InvalidInstructionData);
        }
        Ok(result)
    }
}

// ─── Instruction ────────────────────────────────────────────────────────────

pub struct Swap<'a> {
    pub accounts: SwapAccounts<'a>,
    pub instruction_data: SwapInstructionData,
}

impl<'a> TryFrom<(&'a [u8], &'a [AccountInfo])> for Swap<'a> {
    type Error = ProgramError;
    fn try_from((data, accounts): (&'a [u8], &'a [AccountInfo])) -> Result<Self, Self::Error> {
        let accounts = SwapAccounts::try_from(accounts)?;
        let instruction_data = SwapInstructionData::try_from(data)?;
        Ok(Self {
            accounts,
            instruction_data,
        })
    }
}

impl<'a> Swap<'a> {
    pub const DISCRIMINATOR: &'a u8 = &3;

    pub fn process(&mut self) -> ProgramResult {
        let config = unsafe { Config::load(self.accounts.config)? };
        let is_x = self.instruction_data.is_x != 0;

        // Validate AMM state
        if config.state() != AmmState::Initialized as u8 {
            return Err(ProgramError::InvalidAccountData);
        }

        // Check vault derivations
        let (vault_x, _) = find_program_address(
            &[
                self.accounts.config.key(),
                self.accounts.token_program.key(),
                config.mint_x(),
            ],
            &pinocchio_associated_token_account::ID,
        );
        if vault_x.ne(self.accounts.vault_x.key()) {
            return Err(ProgramError::InvalidAccountData);
        }

        let (vault_y, _) = find_program_address(
            &[
                self.accounts.config.key(),
                self.accounts.token_program.key(),
                config.mint_y(),
            ],
            &pinocchio_associated_token_account::ID,
        );
        if vault_y.ne(self.accounts.vault_y.key()) {
            return Err(ProgramError::InvalidAccountData);
        }

        // Deserialize token accounts
        let vault_x_account =
            unsafe { TokenAccount::from_account_info_unchecked(self.accounts.vault_x)? };
        let vault_y_account =
            unsafe { TokenAccount::from_account_info_unchecked(self.accounts.vault_y)? };

        // Swap calculations
        let mut curve = ConstantProduct::init(
            vault_x_account.amount(),
            vault_y_account.amount(),
            vault_x_account.amount(),
            config.fee(),
            None,
        )
        .map_err(|_| ProgramError::Custom(1))?;

        let p = match is_x {
            true => LiquidityPair::X,
            false => LiquidityPair::Y,
        };

        let swap_result = curve
            .swap(p, self.instruction_data.amount, self.instruction_data.min)
            .map_err(|_| ProgramError::Custom(1))?;

        if swap_result.deposit == 0 || swap_result.withdraw == 0 {
            return Err(ProgramError::InvalidArgument);
        }

        // Build config signer seeds
        let seed_binding = config.seed().to_le_bytes();
        let config_bump = config.config_bump();
        let config_seeds = [
            Seed::from(b"config"),
            Seed::from(&seed_binding),
            Seed::from(config.mint_x().as_ref()),
            Seed::from(config.mint_y().as_ref()),
            Seed::from(&config_bump),
        ];
        let signer = Signer::from(&config_seeds);

        // Execute transfers based on swap direction
        if is_x {
            // User sends X to vault, receives Y from vault
            Transfer {
                from: self.accounts.user_x_ata,
                to: self.accounts.vault_x,
                authority: self.accounts.user,
                amount: swap_result.deposit,
            }
            .invoke()?;

            Transfer {
                from: self.accounts.vault_y,
                to: self.accounts.user_y_ata,
                authority: self.accounts.config,
                amount: swap_result.withdraw,
            }
            .invoke_signed(&[signer])?;
        } else {
            // User sends Y to vault, receives X from vault
            Transfer {
                from: self.accounts.user_y_ata,
                to: self.accounts.vault_y,
                authority: self.accounts.user,
                amount: swap_result.deposit,
            }
            .invoke()?;

            Transfer {
                from: self.accounts.vault_x,
                to: self.accounts.user_x_ata,
                authority: self.accounts.config,
                amount: swap_result.withdraw,
            }
            .invoke_signed(&[signer])?;
        }

        Ok(())
    }
}
