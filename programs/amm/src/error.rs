use anchor_lang::prelude::*;

#[error_code]
pub enum AmmError {
    #[msg("Pool is locked")]
    PoolLocked,
    #[msg("Amount must be greater than zero")]
    InvalidAmount,
    #[msg("Required token amount exceeds the caller's maximum")]
    SlippageExceeded,
    #[msg("fee + protocol_fee must be below 10000 bps")]
    InvalidFee,
    #[msg("Signer is not the pool authority")]
    Unauthorized,
    #[msg("Pool has no liquidity")]
    NoLiquidity,
    #[msg("Arithmetic overflow")]
    MathOverflow,
}
