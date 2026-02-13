use pinocchio::{
    account_info::AccountInfo,
    instruction::{Seed, Signer},
    program_error::ProgramError,
    pubkey::find_program_address,
    sysvars::{rent::Rent, Sysvar},
    ProgramResult,
};
use pinocchio_associated_token_account::instructions::{Create, CreateIdempotent};

// ─── SignerAccount ──────────────────────────────────────────────────────────

pub struct SignerAccount;

impl SignerAccount {
    #[inline(always)]
    pub fn check(account: &AccountInfo) -> Result<(), ProgramError> {
        if !account.is_signer() {
            return Err(ProgramError::MissingRequiredSignature);
        }
        Ok(())
    }
}

// ─── MintInterface ──────────────────────────────────────────────────────────

pub struct MintInterface;

impl MintInterface {
    #[inline(always)]
    pub fn check(account: &AccountInfo) -> Result<(), ProgramError> {
        if !account.is_owned_by(&pinocchio_token::ID) {
            return Err(ProgramError::InvalidAccountOwner);
        }
        Ok(())
    }
}

// ─── AssociatedTokenAccount ─────────────────────────────────────────────────

pub struct AssociatedTokenAccount;

impl AssociatedTokenAccount {
    #[inline(always)]
    pub fn check(
        ata: &AccountInfo,
        owner: &AccountInfo,
        mint: &AccountInfo,
        token_program: &AccountInfo,
    ) -> Result<(), ProgramError> {
        if !ata.is_owned_by(token_program.key()) {
            return Err(ProgramError::InvalidAccountOwner);
        }
        let (expected_key, _) = find_program_address(
            &[owner.key(), token_program.key(), mint.key()],
            &pinocchio_associated_token_account::ID,
        );
        if ata.key() != &expected_key {
            return Err(ProgramError::InvalidAccountData);
        }
        Ok(())
    }

    #[inline(always)]
    pub fn init(
        ata: &AccountInfo,
        mint: &AccountInfo,
        payer: &AccountInfo,
        authority: &AccountInfo,
        system_program: &AccountInfo,
        token_program: &AccountInfo,
    ) -> ProgramResult {
        Create {
            funding_account: payer,
            account: ata,
            wallet: authority,
            mint,
            system_program,
            token_program,
        }
        .invoke()?;
        Ok(())
    }

    #[inline(always)]
    pub fn init_if_needed(
        ata: &AccountInfo,
        mint: &AccountInfo,
        payer: &AccountInfo,
        authority: &AccountInfo,
        system_program: &AccountInfo,
        token_program: &AccountInfo,
    ) -> ProgramResult {
        CreateIdempotent {
            funding_account: payer,
            account: ata,
            wallet: authority,
            mint,
            system_program,
            token_program,
        }
        .invoke()?;
        Ok(())
    }
}

// ─── ProgramAccount ────────────────────────────────────────────────────────

pub struct ProgramAccount;

impl ProgramAccount {
    #[inline(always)]
    pub fn check(account: &AccountInfo) -> Result<(), ProgramError> {
        if !account.is_owned_by(&crate::ID) {
            return Err(ProgramError::InvalidAccountOwner);
        }
        Ok(())
    }

    #[inline(always)]
    pub fn init<T>(
        payer: &AccountInfo,
        account: &AccountInfo,
        seeds: &[Seed],
        data_len: usize,
    ) -> ProgramResult {
        let rent = Rent::get()?;
        let lamports = rent.minimum_balance(data_len);

        let signer = Signer::from(seeds);
        pinocchio_system::instructions::CreateAccount {
            from: payer,
            to: account,
            lamports,
            space: data_len as u64,
            owner: &crate::ID,
        }
        .invoke_signed(&[signer])?;

        Ok(())
    }

    #[inline(always)]
    pub fn close(account: &AccountInfo, destination: &AccountInfo) -> ProgramResult {
        let lamports = account.lamports();
        unsafe {
            *account.borrow_mut_lamports_unchecked() = 0;
            *destination.borrow_mut_lamports_unchecked() += lamports;
        }

        let mut data = account.try_borrow_mut_data()?;
        let len = data.len();
        for byte in data.as_mut()[..len].iter_mut() {
            *byte = 0;
        }

        unsafe {
            account.assign(&pinocchio_system::ID);
        }

        Ok(())
    }
}
