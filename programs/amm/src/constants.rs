use anchor_lang::prelude::*;

#[constant]
pub const LP_SEED: &[u8] = b"lp";

#[constant]
pub const CONFIG_SEED: &[u8] = b"config";

pub const BPS_DENOMINATOR: u64 = 10_000;
