use anchor_lang::prelude::*;
use anchor_spl::{
    associated_token::AssociatedToken,
    token::{transfer_checked, Mint, Token, TokenAccount, TransferChecked},
};

use crate::{constants::*, error::AmmError, math::quote_swap, state::Config};

#[derive(Accounts)]
pub struct Swap<'info> {
    #[account(mut)]
    pub user: Signer<'info>,
    pub mint_x: Box<Account<'info, Mint>>,
    pub mint_y: Box<Account<'info, Mint>>,
    #[account(
        has_one = mint_x,
        has_one = mint_y,
        has_one = treasury,
        seeds = [CONFIG_SEED, config.seed.to_le_bytes().as_ref()],
        bump = config.config_bump,
    )]
    pub config: Box<Account<'info, Config>>,
    /// CHECK: matched against `config.treasury` by `has_one`.
    pub treasury: UncheckedAccount<'info>,
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
        associated_token::authority = treasury,
    )]
    pub treasury_x: Box<Account<'info, TokenAccount>>,
    #[account(
        mut,
        associated_token::mint = mint_y,
        associated_token::authority = treasury,
    )]
    pub treasury_y: Box<Account<'info, TokenAccount>>,
    // Either side can be the output, so both are created on demand.
    #[account(
        init_if_needed,
        payer = user,
        associated_token::mint = mint_x,
        associated_token::authority = user,
    )]
    pub user_x: Box<Account<'info, TokenAccount>>,
    #[account(
        init_if_needed,
        payer = user,
        associated_token::mint = mint_y,
        associated_token::authority = user,
    )]
    pub user_y: Box<Account<'info, TokenAccount>>,
    pub token_program: Program<'info, Token>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

impl<'info> Swap<'info> {
    /// Sell `amount_in` of x (`is_x`) or y for the other token, receiving at least `min_out`.
    pub fn swap(&mut self, is_x: bool, amount_in: u64, min_out: u64) -> Result<()> {
        require!(!self.config.locked, AmmError::PoolLocked);
        require!(amount_in > 0, AmmError::InvalidAmount);

        let (reserve_in, reserve_out) = if is_x {
            (self.vault_x.amount, self.vault_y.amount)
        } else {
            (self.vault_y.amount, self.vault_x.amount)
        };
        require!(reserve_in > 0 && reserve_out > 0, AmmError::NoLiquidity);

        let q = quote_swap(
            reserve_in,
            reserve_out,
            amount_in,
            self.config.fee,
            self.config.protocol_fee,
        )
        .ok_or(AmmError::MathOverflow)?;
        require!(q.amount_out > 0, AmmError::InvalidAmount);
        require!(q.amount_out >= min_out, AmmError::SlippageExceeded);

        let (user_in, vault_in, treasury_in, mint_in, user_out, vault_out, mint_out) = if is_x {
            (
                &self.user_x,
                &self.vault_x,
                &self.treasury_x,
                &self.mint_x,
                &self.user_y,
                &self.vault_y,
                &self.mint_y,
            )
        } else {
            (
                &self.user_y,
                &self.vault_y,
                &self.treasury_y,
                &self.mint_y,
                &self.user_x,
                &self.vault_x,
                &self.mint_x,
            )
        };

        let program = self.token_program.key();
        let user_transfer = |to: &Account<'info, TokenAccount>, amount: u64| {
            transfer_checked(
                CpiContext::new(
                    program,
                    TransferChecked {
                        from: user_in.to_account_info(),
                        mint: mint_in.to_account_info(),
                        to: to.to_account_info(),
                        authority: self.user.to_account_info(),
                    },
                ),
                amount,
                mint_in.decimals,
            )
        };
        if q.protocol_cut > 0 {
            user_transfer(treasury_in, q.protocol_cut)?;
        }
        user_transfer(vault_in, q.to_vault)?;

        let seed = self.config.seed.to_le_bytes();
        let seeds: &[&[u8]] = &[CONFIG_SEED, &seed, &[self.config.config_bump]];
        transfer_checked(
            CpiContext::new_with_signer(
                program,
                TransferChecked {
                    from: vault_out.to_account_info(),
                    mint: mint_out.to_account_info(),
                    to: user_out.to_account_info(),
                    authority: self.config.to_account_info(),
                },
                &[seeds],
            ),
            q.amount_out,
            mint_out.decimals,
        )
    }
}
