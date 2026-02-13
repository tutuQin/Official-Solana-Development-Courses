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
    instructions::{Burn, Transfer},
    state::{Mint, TokenAccount},
};

use constant_product_curve::ConstantProduct;

use crate::state::{AmmState, Config};

// ─── Accounts ───────────────────────────────────────────────────────────────

pub struct WithdrawAccounts<'a> {
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

impl<'a> TryFrom<&'a [AccountInfo]> for WithdrawAccounts<'a> {
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
pub struct WithdrawInstructionData {
    pub amount: u64,
    pub min_x: u64,
    pub min_y: u64,
    pub expiration: i64,
}

impl<'a> TryFrom<&'a [u8]> for WithdrawInstructionData {
    type Error = ProgramError;
    fn try_from(data: &'a [u8]) -> Result<Self, Self::Error> {
        if data.len() != size_of::<Self>() {
            return Err(ProgramError::InvalidInstructionData);
        }
        let result = unsafe { (data.as_ptr() as *const Self).read_unaligned() };
        if result.amount == 0 {
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

pub struct Withdraw<'a> {
    pub accounts: WithdrawAccounts<'a>,
    pub instruction_data: WithdrawInstructionData,
}

impl<'a> TryFrom<(&'a [u8], &'a [AccountInfo])> for Withdraw<'a> {
    type Error = ProgramError;
    fn try_from((data, accounts): (&'a [u8], &'a [AccountInfo])) -> Result<Self, Self::Error> {
        let accounts = WithdrawAccounts::try_from(accounts)?;
        let instruction_data = WithdrawInstructionData::try_from(data)?;
        Ok(Self {
            accounts,
            instruction_data,
        })
    }
}

impl<'a> Withdraw<'a> {
    pub const DISCRIMINATOR: &'a u8 = &2;

    pub fn process(&mut self) -> ProgramResult {
        let config = unsafe { Config::load(self.accounts.config)? };

        // Validate AMM state (allow Initialized and WithdrawOnly, reject Disabled)
        if config.state() == AmmState::Disabled as u8
            || config.state() == AmmState::Uninitialized as u8
        {
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

        // Calculate withdrawal amounts
        let (x, y) = match mint_lp.supply() == self.instruction_data.amount {
            true => (vault_x_account.amount(), vault_y_account.amount()),
            false => {
                let amounts = ConstantProduct::xy_withdraw_amounts_from_l(
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
        if !(x >= self.instruction_data.min_x && y >= self.instruction_data.min_y) {
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

        // Transfer X from vault to user
        Transfer {
            from: self.accounts.vault_x,
            to: self.accounts.user_x_ata,
            authority: self.accounts.config,
            amount: x,
        }
        .invoke_signed(&[signer.clone()])?;

        // Transfer Y from vault to user
        Transfer {
            from: self.accounts.vault_y,
            to: self.accounts.user_y_ata,
            authority: self.accounts.config,
            amount: y,
        }
        .invoke_signed(&[signer])?;

        // Burn LP tokens from user
        Burn {
            account: self.accounts.user_lp_ata,
            mint: self.accounts.mint_lp,
            authority: self.accounts.user,
            amount: self.instruction_data.amount,
        }
        .invoke()?;

        Ok(())
    }
}
