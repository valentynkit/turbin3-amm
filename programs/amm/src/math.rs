//! Constant-product math, written by hand. All intermediate values are u128,
//! so `a * b` of two u64s never overflows; only the final cast back to u64 can fail.

use crate::constants::BPS_DENOMINATOR;

/// floor(a * b / c). `None` on c == 0 or if the result does not fit in u64.
pub fn mul_div_floor(a: u64, b: u64, c: u64) -> Option<u64> {
    let v = (a as u128 * b as u128).checked_div(c as u128)?;
    u64::try_from(v).ok()
}

/// ceil(a * b / c). Used where rounding down would favour the caller over the pool.
pub fn mul_div_ceil(a: u64, b: u64, c: u64) -> Option<u64> {
    if c == 0 {
        return None;
    }
    let v = (a as u128 * b as u128).div_ceil(c as u128);
    u64::try_from(v).ok()
}

#[derive(Debug, PartialEq, Eq)]
pub struct SwapQuote {
    /// Sent from the user to the treasury.
    pub protocol_cut: u64,
    /// Sent from the user to the vault (includes the LP fee).
    pub to_vault: u64,
    /// Sent from the opposite vault to the user.
    pub amount_out: u64,
}

/// Quote a swap of `amount_in` against reserves `(reserve_in, reserve_out)`.
///
/// Both fees come off the input. The LP fee is deposited but not fed into the
/// curve, so it grows k for LP holders. Output is
/// `reserve_out * in_net / (reserve_in + in_net)`, rounded down.
pub fn quote_swap(
    reserve_in: u64,
    reserve_out: u64,
    amount_in: u64,
    fee_bps: u16,
    protocol_fee_bps: u16,
) -> Option<SwapQuote> {
    let protocol_cut = mul_div_floor(amount_in, protocol_fee_bps as u64, BPS_DENOMINATOR)?;
    let lp_fee = mul_div_floor(amount_in, fee_bps as u64, BPS_DENOMINATOR)?;
    let to_vault = amount_in.checked_sub(protocol_cut)?;
    let in_net = to_vault.checked_sub(lp_fee)?;
    let amount_out = mul_div_floor(reserve_out, in_net, reserve_in.checked_add(in_net)?)?;
    Some(SwapQuote {
        protocol_cut,
        to_vault,
        amount_out,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mul_div_rounding() {
        assert_eq!(mul_div_floor(10, 10, 3), Some(33));
        assert_eq!(mul_div_ceil(10, 10, 3), Some(34));
        assert_eq!(mul_div_ceil(10, 10, 5), Some(20));
        assert_eq!(mul_div_floor(1, 1, 0), None);
        assert_eq!(mul_div_ceil(1, 1, 0), None);
        // u64::MAX * u64::MAX fits in u128, but the quotient does not fit in u64.
        assert_eq!(mul_div_floor(u64::MAX, u64::MAX, 1), None);
        assert_eq!(mul_div_floor(u64::MAX, u64::MAX, u64::MAX), Some(u64::MAX));
    }

    #[test]
    fn quote_matches_readme_example() {
        let q = quote_swap(1_000_000, 1_000_000, 10_000, 30, 5).unwrap();
        assert_eq!(q.protocol_cut, 5);
        assert_eq!(q.to_vault, 9_995);
        // 1_000_000 * 9_965 / 1_009_965 = 9_866.67 -> 9_866
        assert_eq!(q.amount_out, 9_866);
    }

    #[test]
    fn quote_without_fees_is_plain_xy_k() {
        let q = quote_swap(100, 200, 100, 0, 0).unwrap();
        assert_eq!(q.amount_out, 100);
        assert_eq!(q.protocol_cut, 0);
        assert_eq!(q.to_vault, 100);
    }

    #[test]
    fn k_never_decreases() {
        let reserves = [1, 7, 1_000, 999_983, 1_000_000_000, u64::MAX / 4];
        let amounts = [1, 3, 99, 10_000, 123_456_789];
        let fees = [(0, 0), (30, 0), (30, 5), (100, 50), (9_000, 999)];
        for &rx in &reserves {
            for &ry in &reserves {
                for &a in &amounts {
                    for &(f, p) in &fees {
                        let q = quote_swap(rx, ry, a, f, p).unwrap();
                        let k0 = rx as u128 * ry as u128;
                        let k1 = (rx + q.to_vault) as u128 * (ry - q.amount_out) as u128;
                        assert!(k1 >= k0, "k shrank: rx={rx} ry={ry} a={a} f={f} p={p}");
                        assert!(q.amount_out < ry);
                    }
                }
            }
        }
    }

    #[test]
    fn quote_overflow_is_none() {
        assert_eq!(quote_swap(u64::MAX, 1, 1, 0, 0), None);
    }
}
