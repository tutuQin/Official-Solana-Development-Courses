use core::mem::size_of;

use pinocchio::{
    account_info::AccountInfo,
    instruction::{Seed, Signer},
    program_error::ProgramError,
    pubkey::find_program_address,
    sysvars::{clock::Clock, Sysvar},
    ProgramResult,
};
use pinocchio_token::{
    instructions::{MintTo, Transfer},
    state::{Mint, TokenAccount},
};

use constant_product_curve::ConstantProduct;

use crate::state::{AmmState, Config};

// ─── Accounts ───────────────────────────────────────────────────────────────

pub struct DepositAccounts<'a> {
    pub user: &'a AccountInfo,
    pub mint_lp: &'a AccountInfo,
    pub vault_x: &'a AccountInfo,
    pub vault_y: &'a AccountInfo,
    pub user_x_ata: &'a AccountInfo,
    pub user_y_ata: &'a AccountInfo,
    pub user_lp_ata: &'a AccountInfo,
    pub config: &'a AccountInfo,
    pub token_program: &'a AccountInfo,
}

impl<'a> TryFrom<&'a [AccountInfo]> for DepositAccounts<'a> {
    type Error = ProgramError;
    fn try_from(accounts: &'a [AccountInfo]) -> Result<Self, Self::Error> {
        let [user, mint_lp, vault_x, vault_y, user_x_ata, user_y_ata, user_lp_ata, config, token_program, ..] =
            accounts
        else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };
        Ok(Self {
            user,
            mint_lp,
            vault_x,
            vault_y,
            user_x_ata,
            user_y_ata,
            user_lp_ata,
            config,
            token_program,
        })
    }
}

// ─── Instruction Data ───────────────────────────────────────────────────────

#[repr(C, packed)]
pub struct DepositInstructionData {
    pub amount: u64,
    pub max_x: u64,
    pub max_y: u64,
    pub expiration: i64,
}

impl<'a> TryFrom<&'a [u8]> for DepositInstructionData {
    type Error = ProgramError;
    fn try_from(data: &'a [u8]) -> Result<Self, Self::Error> {
        if data.len() != size_of::<Self>() {
            return Err(ProgramError::InvalidInstructionData);
        }
        let result = unsafe { (data.as_ptr() as *const Self).read_unaligned() };
        if result.amount == 0 || result.max_x == 0 || result.max_y == 0 {
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

pub struct Deposit<'a> {
    pub accounts: DepositAccounts<'a>,
    pub instruction_data: DepositInstructionData,
}

impl<'a> TryFrom<(&'a [u8], &'a [AccountInfo])> for Deposit<'a> {
    type Error = ProgramError;
    fn try_from((data, accounts): (&'a [u8], &'a [AccountInfo])) -> Result<Self, Self::Error> {
        let accounts = DepositAccounts::try_from(accounts)?;
        let instruction_data = DepositInstructionData::try_from(data)?;
        Ok(Self {
            accounts,
            instruction_data,
        })
    }
}

impl<'a> Deposit<'a> {
    pub const DISCRIMINATOR: &'a u8 = &1;

    pub fn process(&mut self) -> ProgramResult {
        let config = unsafe { Config::load(self.accounts.config)? };

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
        let mint_lp = unsafe { Mint::from_account_info_unchecked(self.accounts.mint_lp)? };
        let vault_x_account =
            unsafe { TokenAccount::from_account_info_unchecked(self.accounts.vault_x)? };
        let vault_y_account =
            unsafe { TokenAccount::from_account_info_unchecked(self.accounts.vault_y)? };

        // Grab the amounts to deposit
        let (x, y) = match mint_lp.supply() == 0
            && vault_x_account.amount() == 0
            && vault_y_account.amount() == 0
        {
            true => (self.instruction_data.max_x, self.instruction_data.max_y),
            false => {
                let amounts = ConstantProduct::xy_deposit_amounts_from_l(
                    vault_x_account.amount(),
                    vault_y_account.amount(),
                    mint_lp.supply(),
                    self.instruction_data.amount,
                    6,
                )
                .map_err(|_| ProgramError::InvalidArgument)?;
                (amounts.x, amounts.y)
            }
        };

        // Check for slippage
        if !(x <= self.instruction_data.max_x && y <= self.instruction_data.max_y) {
            return Err(ProgramError::InvalidArgument);
        }

        // Transfer X tokens from user to vault
        Transfer {
            from: self.accounts.user_x_ata,
            to: self.accounts.vault_x,
            authority: self.accounts.user,
            amount: x,
        }
        .invoke()?;

        // Transfer Y tokens from user to vault
        Transfer {
            from: self.accounts.user_y_ata,
            to: self.accounts.vault_y,
            authority: self.accounts.user,
            amount: y,
        }
        .invoke()?;

        // Mint LP tokens to user
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

        MintTo {
            mint: self.accounts.mint_lp,
            account: self.accounts.user_lp_ata,
            mint_authority: self.accounts.config,
            amount: self.instruction_data.amount,
        }
        .invoke_signed(&[signer])?;

        Ok(())
    }
}
