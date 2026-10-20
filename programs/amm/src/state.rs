use anchor_lang::prelude::*;

#[account]
#[derive(InitSpace)]
pub struct Config {
    pub seed: u64,
    pub authority: Option<Pubkey>,
    pub mint_x: Pubkey,
    pub mint_y: Pubkey,
    /// Receives the protocol cut of every swap, in its x and y ATAs.
    pub treasury: Pubkey,
    /// LP fee in basis points. Stays in the vault, so it accrues to LP holders.
    pub fee: u16,
    /// Protocol fee in basis points, paid to the treasury.
    pub protocol_fee: u16,
    pub locked: bool,
    pub config_bump: u8,
    pub lp_bump: u8,
}
