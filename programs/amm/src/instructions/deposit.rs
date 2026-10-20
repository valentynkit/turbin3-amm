use anchor_lang::prelude::*;
use anchor_spl::{
    associated_token::AssociatedToken,
    token::{mint_to, transfer_checked, Mint, MintTo, Token, TokenAccount, TransferChecked},
};

use crate::{constants::*, error::AmmError, math::mul_div_ceil, state::Config};

#[derive(Accounts)]
pub struct Deposit<'info> {
    #[account(mut)]
    pub user: Signer<'info>,
    pub mint_x: Account<'info, Mint>,
    pub mint_y: Account<'info, Mint>,
    #[account(
        has_one = mint_x,
        has_one = mint_y,
        seeds = [CONFIG_SEED, config.seed.to_le_bytes().as_ref()],
        bump = config.config_bump,
    )]
    pub config: Account<'info, Config>,
    #[account(
        mut,
        seeds = [LP_SEED, config.key().as_ref()],
        bump = config.lp_bump,
    )]
    pub mint_lp: Account<'info, Mint>,
    #[account(
        mut,
        associated_token::mint = mint_x,
        associated_token::authority = config,
    )]
    pub vault_x: Box<Account<'info, TokenAccount>>,
    #[account(
        mut,
        associated_token::mint = mint_y,
        associated_token::authority = config,
    )]
    pub vault_y: Box<Account<'info, TokenAccount>>,
    #[account(
        mut,
        associated_token::mint = mint_x,
        associated_token::authority = user,
    )]
    pub user_x: Box<Account<'info, TokenAccount>>,
    #[account(
        mut,
        associated_token::mint = mint_y,
        associated_token::authority = user,
    )]
    pub user_y: Box<Account<'info, TokenAccount>>,
    #[account(
        init_if_needed,
        payer = user,
        associated_token::mint = mint_lp,
        associated_token::authority = user,
    )]
    pub user_lp: Box<Account<'info, TokenAccount>>,
    pub token_program: Program<'info, Token>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

impl<'info> Deposit<'info> {
    /// Mint `amount` LP tokens, pulling the matching share of x and y from the user.
    /// `max_x` / `max_y` bound what the user is willing to pay (slippage).
    pub fn deposit(&mut self, amount: u64, max_x: u64, max_y: u64) -> Result<()> {
        require!(!self.config.locked, AmmError::PoolLocked);
        require!(amount > 0, AmmError::InvalidAmount);

        let supply = self.mint_lp.supply;
        let (x, y) = if supply == 0 {
            // First depositor sets the price and gets `amount` LP 1:1.
            require!(max_x > 0 && max_y > 0, AmmError::InvalidAmount);
            (max_x, max_y)
        } else {
            // Round up so the pool never hands out LP for less than its share.
            let share = |vault: u64| {
                mul_div_ceil(amount, vault, supply).ok_or(error!(AmmError::MathOverflow))
            };
            (share(self.vault_x.amount)?, share(self.vault_y.amount)?)
        };
        require!(x <= max_x && y <= max_y, AmmError::SlippageExceeded);

        self.deposit_tokens(true, x)?;
        self.deposit_tokens(false, y)?;
        self.mint_lp_tokens(amount)
    }

    fn deposit_tokens(&self, is_x: bool, amount: u64) -> Result<()> {
        let (from, to, mint) = if is_x {
            (&self.user_x, &self.vault_x, &self.mint_x)
        } else {
            (&self.user_y, &self.vault_y, &self.mint_y)
        };
        transfer_checked(
            CpiContext::new(
                self.token_program.key(),
                TransferChecked {
                    from: from.to_account_info(),
                    mint: mint.to_account_info(),
                    to: to.to_account_info(),
                    authority: self.user.to_account_info(),
                },
            ),
            amount,
            mint.decimals,
        )
    }

    fn mint_lp_tokens(&self, amount: u64) -> Result<()> {
        let seed = self.config.seed.to_le_bytes();
        let seeds: &[&[u8]] = &[CONFIG_SEED, &seed, &[self.config.config_bump]];
        mint_to(
            CpiContext::new_with_signer(
                self.token_program.key(),
                MintTo {
                    mint: self.mint_lp.to_account_info(),
                    to: self.user_lp.to_account_info(),
                    authority: self.config.to_account_info(),
                },
                &[seeds],
            ),
            amount,
        )
    }
}
