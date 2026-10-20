# AMM

A constant-product AMM (x * y = k) for one SPL token pair, written in Anchor.
Turbin3 Q3 2026, week 3.

Anyone can create a pool, add liquidity for LP tokens, swap one side for the
other, and burn LP tokens to take their share back out. Each swap pays an LP fee
that stays in the pool and a protocol fee that goes to a treasury. An optional
authority can lock the pool.

The curve math is written by hand in `programs/amm/src/math.rs`. There is no
dependency on a curve crate.

## Accounts

| Account | Kind | Seeds / derivation | Purpose |
|---|---|---|---|
| `config` | PDA, program owned | `["config", seed.to_le_bytes()]` | Pool settings, owns the vaults, mints LP |
| `mint_lp` | PDA, SPL mint | `["lp", config]` | LP token, 6 decimals, mint authority `config` |
| `vault_x`, `vault_y` | ATA | ATA(`config`, `mint_x` / `mint_y`) | Pool reserves |
| `treasury_x`, `treasury_y` | ATA | ATA(`treasury`, `mint_x` / `mint_y`) | Protocol fee destination |
| `user_x`, `user_y`, `user_lp` | ATA | ATA(`user`, mint) | Caller's token accounts |

`Config` stores `seed`, `authority: Option<Pubkey>`, `mint_x`, `mint_y`,
`treasury`, `fee` (bps), `protocol_fee` (bps), `locked`, and both bumps. The
`u64` seed lets the same pair have more than one pool, for example with
different fees.

## Instructions

| Instruction | Args | What it does |
|---|---|---|
| `initialize` | `seed, fee, protocol_fee, authority` | Creates `config`, `mint_lp`, both vaults, and both treasury ATAs (`init_if_needed`, so existing ones are fine). Rejects `fee + protocol_fee >= 10_000`. |
| `deposit` | `amount, max_x, max_y` | Mints `amount` LP. On an empty pool the caller deposits exactly `max_x` and `max_y` (both must be > 0) and that sets the price. After that the cost is `amount * vault / supply`, rounded up, and must fit within `max_x` / `max_y`. |
| `swap` | `is_x, amount_in, min_out` | Sells `amount_in` of x (`is_x = true`) or y for the other token. Fails if output is 0 or below `min_out`, or if either reserve is empty. Creates the caller's output ATA if missing. |
| `withdraw` | `amount, min_x, min_y` | Burns `amount` LP and pays `amount * vault / supply` of each side, rounded down. Fails below `min_x` / `min_y`. |
| `lock` / `unlock` | none | Only the signer equal to `config.authority` may call these. A locked pool rejects deposit, swap and withdraw. A pool created with `authority: None` can never be locked. |

## Fees and treasury

Two fees, both in basis points of the input amount:

- `fee` is the LP fee. It goes into the vault but is left out of the curve
  input, so it raises k and every LP token is worth a little more after each
  swap.
- `protocol_fee` is sent straight from the trader to the treasury ATA for the
  input mint. It never touches the vault.

Worked example. Pool holds 1,000,000 x and 1,000,000 y, `fee = 30`,
`protocol_fee = 5`. A trader sells 10,000 x:

| Step | Value |
|---|---|
| protocol cut = 10,000 * 5 / 10,000 | 5 x to `treasury_x` |
| LP fee = 10,000 * 30 / 10,000 | 30 x |
| sent to vault = 10,000 - 5 | 9,995 x |
| curve input = 10,000 - 5 - 30 | 9,965 x |
| out = 1,000,000 * 9,965 / (1,000,000 + 9,965) = 9,866.67 | 9,866 y |
| new reserves | 1,009,995 x, 990,134 y |
| k before / after | 1,000,000,000,000 / 1,000,030,389,330 |

The same numbers are asserted in `math.rs` and in the
`swap_x_for_y_pays_treasury_and_matches_curve` integration test.

Fees round down, so each fee can come up short by less than one base unit of
the input token. Splitting a trade into dust-sized pieces to dodge fees costs
far more in transaction fees than it saves, and the curve output still rounds
down, so k cannot shrink either way.

## CPMM math

Reserves `X`, `Y`, invariant `X * Y = k`. Selling `dx` (after fees) for `dy`
must keep the product:

```
(X + dx) * (Y - dy) = X * Y
Y - dy = X * Y / (X + dx)
dy     = Y - X * Y / (X + dx)
       = Y * dx / (X + dx)
```

The program computes `dy = floor(Y * dx_net / (X + dx_net))` where
`dx_net = amount_in - protocol_cut - lp_fee`. All products are done in `u128`
(two `u64`s multiplied cannot overflow it), with checked add/sub/div, and the
result is converted back to `u64` with a checked cast.

Rounding always goes against the caller:

- swap output rounds down, so the pool pays out at most the exact curve value.
  Since `floor(Y * dx / (X + dx)) <= Y * dx / (X + dx)`, the new product
  `(X + dx) * (Y - dy)` is at least `X * Y`. The vault also receives the LP fee
  on top of `dx_net`, so k strictly grows on any trade with a fee.
- deposit cost rounds up, so a depositor can never get LP tokens for less than
  their share.
- withdraw payout rounds down, so a withdrawer can never take more than their
  share.

If any of these rounded the other way, a caller could repeat tiny operations
and pull 1 unit out of the pool each time. Rounding toward the pool leaves the
dust with the remaining LPs.

`math.rs` has unit tests for the rounding helpers, the worked example, overflow,
and a grid check that k never decreases over a range of reserves, amounts, and
fee settings.

## Build and test

Requires Anchor CLI 1.2.x and Rust 1.97 (pinned in `rust-toolchain.toml`).

```sh
anchor test
```

`anchor test` builds the program (SBPF v3, Anchor's default) and runs
`cargo test`, as set in `Anchor.toml`. The tests use LiteSVM 0.16, which
loads v3 programs; LiteSVM 0.10 fails `add_program` on them with
`InvalidAccountData`.

The integration tests in `programs/amm/tests/test_amm.rs` load
`target/deploy/amm.so` into LiteSVM and cover every instruction: happy paths
with exact expected balances, plus slippage, zero amounts, empty pool,
overdraw, bad fees, locked pool, and non-authority lock attempts. Failures are
checked by Anchor error name, not just "it failed".

Full output of the last run: [`docs/test-output.txt`](docs/test-output.txt).

![All tests passing](docs/tests-passing.png)

## Mitigating downtime

What can go wrong is either the cluster stops, my infrastructure stops, or the
pool's admin stops. The design tries to make each of those harmless.

- **No keeper or oracle.** Prices come from the reserves alone. Nothing has to
  run on a schedule and no external feed can go stale, so the pool cannot drift
  into a bad state while an off-chain service is down.
- **Users carry their own bounds.** Every state change takes slippage limits
  (`min_out`, `max_x`/`max_y`, `min_x`/`min_y`). A transaction that lands late,
  after the price moved, fails instead of filling at a bad rate. A client should
  also use a recent blockhash as a deadline (it expires after about 150 slots),
  or a durable nonce plus an explicit expiry slot in the instruction if longer
  lived orders are ever needed.
- **Landing transactions under load.** Set a compute unit limit and a priority
  fee from recent fee data, simulate first, and retry with a fresh blockhash on
  expiry. Slippage bounds make retries safe.
- **More than one RPC.** Clients should fail over between at least two RPC
  providers. A single provider outage looks like downtime to users even when
  the chain is fine.
- **Lock as a circuit breaker.** If a bug is found, the authority can freeze
  deposits, swaps and withdrawals in one transaction. The trade-off is that the
  same key can freeze user funds, and if it is lost a locked pool stays locked
  forever. Use a multisig (for example Squads) as the authority, or create the
  pool with `authority: None` when there is no admin at all. A safer variant
  would keep `withdraw` open while locked so LPs can always exit; this program
  follows the assignment and blocks it too.
- **No admin liveness dependency.** Nothing in the normal path needs the
  authority to act. Fees accrue to the treasury ATAs automatically, so the
  treasury key only matters when someone wants to spend from it.
- **If the cluster halts.** All state lives in accounts and nothing in the
  program reads the clock. Nothing expires, no position gets liquidated, and
  when the chain restarts the pool is exactly where it was. In-flight user
  transactions simply expire with their blockhash and can be resubmitted with
  the same slippage bounds.
