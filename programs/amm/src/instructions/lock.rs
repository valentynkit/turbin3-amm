use anchor_lang::prelude::*;

use crate::{constants::*, error::AmmError, state::Config};

#[derive(Accounts)]
pub struct Lock<'info> {
    pub authority: Signer<'info>,
    #[account(
        mut,
        seeds = [CONFIG_SEED, config.seed.to_le_bytes().as_ref()],
        bump = config.config_bump,
    )]
    pub config: Account<'info, Config>,
}

impl<'info> Lock<'info> {
    /// A pool created with `authority: None` can never be locked or unlocked.
    pub fn set_locked(&mut self, locked: bool) -> Result<()> {
        require!(
            self.config.authority == Some(self.authority.key()),
            AmmError::Unauthorized
        );
        self.config.locked = locked;
        Ok(())
    }
}
