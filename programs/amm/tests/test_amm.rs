// TransactionResult is litesvm's type; its large Err variant is not ours to shrink.
#![allow(clippy::result_large_err)]

use {
    anchor_lang::{
        prelude::Pubkey,
        solana_program::{
            instruction::Instruction, program_pack::Pack, system_instruction, system_program,
        },
        AccountDeserialize, InstructionData, ToAccountMetas,
    },
    anchor_spl::{
        associated_token::{self, get_associated_token_address, spl_associated_token_account},
        token::{self, spl_token, Mint, TokenAccount},
    },
    litesvm::{types::TransactionResult, LiteSVM},
    solana_keypair::Keypair,
    solana_message::{Message, VersionedMessage},
    solana_signer::Signer,
    solana_transaction::versioned::VersionedTransaction,
};

const SEED: u64 = 42;
const FEE: u16 = 30;
const PROTOCOL_FEE: u16 = 5;
const START_BALANCE: u64 = 10_000_000;

fn send(svm: &mut LiteSVM, ixs: &[Instruction], signers: &[&Keypair]) -> TransactionResult {
    // A fresh blockhash per tx, so sending the same instruction twice is not a duplicate.
    svm.expire_blockhash();
    let blockhash = svm.latest_blockhash();
    let msg = Message::new_with_blockhash(ixs, Some(&signers[0].pubkey()), &blockhash);
    let tx = VersionedTransaction::try_new(VersionedMessage::Legacy(msg), signers).unwrap();
    svm.send_transaction(tx)
}

/// Asserts the transaction failed with the given AmmError variant.
fn assert_err(res: TransactionResult, code: &str) {
    let err = res.expect_err("transaction should have failed");
    let needle = format!("Error Code: {code}.");
    assert!(
        err.meta.logs.iter().any(|l| l.contains(&needle)),
        "expected {code}, logs: {:#?}",
        err.meta.logs
    );
}

/// Creates a 6-decimal mint with `authority` as mint authority.
fn create_mint(svm: &mut LiteSVM, authority: &Keypair) -> Pubkey {
    let mint = Keypair::new();
    let rent = svm.minimum_balance_for_rent_exemption(spl_token::state::Mint::LEN);
    let ixs = [
        system_instruction::create_account(
            &authority.pubkey(),
            &mint.pubkey(),
            rent,
            spl_token::state::Mint::LEN as u64,
            &token::ID,
        ),
        spl_token::instruction::initialize_mint2(
            &token::ID,
            &mint.pubkey(),
            &authority.pubkey(),
            None,
            6,
        )
        .unwrap(),
    ];
    send(svm, &ixs, &[authority, &mint]).unwrap();
    mint.pubkey()
}

fn create_ata(svm: &mut LiteSVM, payer: &Keypair, owner: &Pubkey, mint: &Pubkey) -> Pubkey {
    let ix = spl_associated_token_account::instruction::create_associated_token_account(
        &payer.pubkey(),
        owner,
        mint,
        &token::ID,
    );
    send(svm, &[ix], &[payer]).unwrap();
    get_associated_token_address(owner, mint)
}

fn mint_to(svm: &mut LiteSVM, authority: &Keypair, mint: &Pubkey, owner: &Pubkey, amount: u64) {
    let ata = create_ata(svm, authority, owner, mint);
    let ix =
        spl_token::instruction::mint_to(&token::ID, mint, &ata, &authority.pubkey(), &[], amount)
            .unwrap();
    send(svm, &[ix], &[authority]).unwrap();
}

/// One pool's addresses plus the VM. `admin` funds the pool and is its authority
/// in most tests; `trader` swaps. Both start with START_BALANCE of x and y.
struct Env {
    svm: LiteSVM,
    mint_x: Pubkey,
    mint_y: Pubkey,
    treasury: Pubkey,
    config: Pubkey,
    mint_lp: Pubkey,
    vault_x: Pubkey,
    vault_y: Pubkey,
    treasury_x: Pubkey,
    treasury_y: Pubkey,
}

fn setup() -> (Env, Keypair, Keypair) {
    let program_id = amm::id();
    let admin = Keypair::new();
    let trader = Keypair::new();
    let mut svm = LiteSVM::new();
    svm.add_program(
        program_id,
        include_bytes!(concat!(env!("CARGO_TARGET_TMPDIR"), "/../deploy/amm.so")),
    )
    .unwrap();
    svm.airdrop(&admin.pubkey(), 10_000_000_000).unwrap();
    svm.airdrop(&trader.pubkey(), 10_000_000_000).unwrap();

    let mint_x = create_mint(&mut svm, &admin);
    let mint_y = create_mint(&mut svm, &admin);
    for who in [admin.pubkey(), trader.pubkey()] {
        mint_to(&mut svm, &admin, &mint_x, &who, START_BALANCE);
        mint_to(&mut svm, &admin, &mint_y, &who, START_BALANCE);
    }

    let treasury = Keypair::new().pubkey();
    let config =
        Pubkey::find_program_address(&[amm::CONFIG_SEED, &SEED.to_le_bytes()], &program_id).0;
    let mint_lp = Pubkey::find_program_address(&[amm::LP_SEED, config.as_ref()], &program_id).0;
    let env = Env {
        svm,
        mint_x,
        mint_y,
        treasury,
        config,
        mint_lp,
        vault_x: get_associated_token_address(&config, &mint_x),
        vault_y: get_associated_token_address(&config, &mint_y),
        treasury_x: get_associated_token_address(&treasury, &mint_x),
        treasury_y: get_associated_token_address(&treasury, &mint_y),
    };
    (env, admin, trader)
}

/// Pool with fee 30 bps, protocol fee 5 bps, admin as authority, seeded with `x`/`y`.
fn pool(x: u64, y: u64) -> (Env, Keypair, Keypair) {
    let (mut env, admin, trader) = setup();
    env.initialize(&admin, FEE, PROTOCOL_FEE, Some(admin.pubkey()))
        .unwrap();
    env.deposit(&admin, 1_000_000, x, y).unwrap();
    (env, admin, trader)
}

impl Env {
    fn ix(&self, data: impl InstructionData, accounts: impl ToAccountMetas) -> Instruction {
        Instruction::new_with_bytes(amm::id(), &data.data(), accounts.to_account_metas(None))
    }

    fn initialize(
        &mut self,
        who: &Keypair,
        fee: u16,
        protocol_fee: u16,
        authority: Option<Pubkey>,
    ) -> TransactionResult {
        let ix = self.ix(
            amm::instruction::Initialize {
                seed: SEED,
                fee,
                protocol_fee,
                authority,
            },
            amm::accounts::Initialize {
                initializer: who.pubkey(),
                mint_x: self.mint_x,
                mint_y: self.mint_y,
                treasury: self.treasury,
                config: self.config,
                mint_lp: self.mint_lp,
                vault_x: self.vault_x,
                vault_y: self.vault_y,
                treasury_x: self.treasury_x,
                treasury_y: self.treasury_y,
                token_program: token::ID,
                associated_token_program: associated_token::ID,
                system_program: system_program::ID,
            },
        );
        send(&mut self.svm, &[ix], &[who])
    }

    fn deposit(&mut self, who: &Keypair, amount: u64, max_x: u64, max_y: u64) -> TransactionResult {
        let ix = self.ix(
            amm::instruction::Deposit {
                amount,
                max_x,
                max_y,
            },
            amm::accounts::Deposit {
                user: who.pubkey(),
                mint_x: self.mint_x,
                mint_y: self.mint_y,
                config: self.config,
                mint_lp: self.mint_lp,
                vault_x: self.vault_x,
                vault_y: self.vault_y,
                user_x: self.ata(who, self.mint_x),
                user_y: self.ata(who, self.mint_y),
                user_lp: self.ata(who, self.mint_lp),
                token_program: token::ID,
                associated_token_program: associated_token::ID,
                system_program: system_program::ID,
            },
        );
        send(&mut self.svm, &[ix], &[who])
    }

    fn swap(
        &mut self,
        who: &Keypair,
        is_x: bool,
        amount_in: u64,
        min_out: u64,
    ) -> TransactionResult {
        let ix = self.ix(
            amm::instruction::Swap {
                is_x,
                amount_in,
                min_out,
            },
            amm::accounts::Swap {
                user: who.pubkey(),
                mint_x: self.mint_x,
                mint_y: self.mint_y,
                config: self.config,
                treasury: self.treasury,
                vault_x: self.vault_x,
                vault_y: self.vault_y,
                treasury_x: self.treasury_x,
                treasury_y: self.treasury_y,
                user_x: self.ata(who, self.mint_x),
                user_y: self.ata(who, self.mint_y),
                token_program: token::ID,
                associated_token_program: associated_token::ID,
                system_program: system_program::ID,
            },
        );
        send(&mut self.svm, &[ix], &[who])
    }

    fn withdraw(
        &mut self,
        who: &Keypair,
        amount: u64,
        min_x: u64,
        min_y: u64,
    ) -> TransactionResult {
        let ix = self.ix(
            amm::instruction::Withdraw {
                amount,
                min_x,
                min_y,
            },
            amm::accounts::Withdraw {
                user: who.pubkey(),
                mint_x: self.mint_x,
                mint_y: self.mint_y,
                config: self.config,
                mint_lp: self.mint_lp,
                vault_x: self.vault_x,
                vault_y: self.vault_y,
                user_x: self.ata(who, self.mint_x),
                user_y: self.ata(who, self.mint_y),
                user_lp: self.ata(who, self.mint_lp),
                token_program: token::ID,
                associated_token_program: associated_token::ID,
                system_program: system_program::ID,
            },
        );
        send(&mut self.svm, &[ix], &[who])
    }

    fn set_locked(&mut self, who: &Keypair, locked: bool) -> TransactionResult {
        let accounts = amm::accounts::Lock {
            authority: who.pubkey(),
            config: self.config,
        };
        let ix = if locked {
            self.ix(amm::instruction::Lock {}, accounts)
        } else {
            self.ix(amm::instruction::Unlock {}, accounts)
        };
        send(&mut self.svm, &[ix], &[who])
    }

    fn ata(&self, who: &Keypair, mint: Pubkey) -> Pubkey {
        get_associated_token_address(&who.pubkey(), &mint)
    }

    fn balance(&self, ata: Pubkey) -> u64 {
        let data = self.svm.get_account(&ata).unwrap().data;
        TokenAccount::try_deserialize(&mut data.as_slice())
            .unwrap()
            .amount
    }

    fn lp_supply(&self) -> u64 {
        let data = self.svm.get_account(&self.mint_lp).unwrap().data;
        Mint::try_deserialize(&mut data.as_slice()).unwrap().supply
    }

    fn config(&self) -> amm::Config {
        let data = self.svm.get_account(&self.config).unwrap().data;
        amm::Config::try_deserialize(&mut data.as_slice()).unwrap()
    }

    fn reserves(&self) -> (u64, u64) {
        (self.balance(self.vault_x), self.balance(self.vault_y))
    }

    fn k(&self) -> u128 {
        let (x, y) = self.reserves();
        x as u128 * y as u128
    }
}

// ---------- initialize ----------

#[test]
fn initialize_stores_config_and_creates_accounts() {
    let (mut env, admin, _) = setup();
    env.initialize(&admin, FEE, PROTOCOL_FEE, Some(admin.pubkey()))
        .unwrap();

    let cfg = env.config();
    assert_eq!(cfg.seed, SEED);
    assert_eq!(cfg.authority, Some(admin.pubkey()));
    assert_eq!(cfg.mint_x, env.mint_x);
    assert_eq!(cfg.mint_y, env.mint_y);
    assert_eq!(cfg.treasury, env.treasury);
    assert_eq!(cfg.fee, FEE);
    assert_eq!(cfg.protocol_fee, PROTOCOL_FEE);
    assert!(!cfg.locked);
    assert_eq!(env.lp_supply(), 0);
    for ata in [env.vault_x, env.vault_y, env.treasury_x, env.treasury_y] {
        assert_eq!(env.balance(ata), 0);
    }
}

#[test]
fn initialize_rejects_fees_at_or_above_100_percent() {
    let (mut env, admin, _) = setup();
    assert_err(env.initialize(&admin, 10_000, 0, None), "InvalidFee");
    assert_err(env.initialize(&admin, 9_995, 5, None), "InvalidFee");
    assert_err(
        env.initialize(&admin, u16::MAX, u16::MAX, None),
        "InvalidFee",
    );
    // 9_999 bps total is the highest allowed.
    env.initialize(&admin, 9_998, 1, None).unwrap();
}

#[test]
fn initialize_accepts_existing_treasury_atas() {
    let (mut env, admin, _) = setup();
    let (treasury, mint_x, mint_y) = (env.treasury, env.mint_x, env.mint_y);
    create_ata(&mut env.svm, &admin, &treasury, &mint_x);
    create_ata(&mut env.svm, &admin, &treasury, &mint_y);
    env.initialize(&admin, FEE, PROTOCOL_FEE, None).unwrap();
}

// ---------- deposit ----------

#[test]
fn deposit_first_sets_price_then_follows_ratio() {
    let (mut env, admin, trader) = setup();
    env.initialize(&admin, FEE, PROTOCOL_FEE, None).unwrap();

    // First deposit takes max_x / max_y as-is and mints LP 1:1 with `amount`.
    env.deposit(&admin, 100, 200, 300).unwrap();
    assert_eq!(env.reserves(), (200, 300));
    assert_eq!(env.balance(env.ata(&admin, env.mint_lp)), 100);
    assert_eq!(
        env.balance(env.ata(&admin, env.mint_x)),
        START_BALANCE - 200
    );

    // Second depositor pays the pool ratio for 50 LP: 100 x, 150 y.
    env.deposit(&trader, 50, 100, 150).unwrap();
    assert_eq!(env.reserves(), (300, 450));
    assert_eq!(env.balance(env.ata(&trader, env.mint_lp)), 50);
    assert_eq!(env.lp_supply(), 150);
}

#[test]
fn deposit_rounds_cost_up() {
    let (mut env, admin, trader) = setup();
    env.initialize(&admin, FEE, PROTOCOL_FEE, None).unwrap();
    env.deposit(&admin, 3, 10, 10).unwrap();

    // 1 LP of 3 against 10/10 is worth 3.33 of each. Floor would charge 3; we charge 4.
    assert_err(env.deposit(&trader, 1, 3, 3), "SlippageExceeded");
    env.deposit(&trader, 1, 4, 4).unwrap();
    assert_eq!(env.reserves(), (14, 14));
}

#[test]
fn deposit_rejects_slippage_zero_and_empty_first_side() {
    let (mut env, admin, _) = setup();
    env.initialize(&admin, FEE, PROTOCOL_FEE, None).unwrap();

    assert_err(env.deposit(&admin, 100, 0, 300), "InvalidAmount");
    assert_err(env.deposit(&admin, 100, 200, 0), "InvalidAmount");
    assert_err(env.deposit(&admin, 0, 200, 300), "InvalidAmount");

    env.deposit(&admin, 100, 200, 300).unwrap();
    // 50 LP needs 100 x; caller allows 99.
    assert_err(env.deposit(&admin, 50, 99, 150), "SlippageExceeded");
    assert_err(env.deposit(&admin, 50, 100, 149), "SlippageExceeded");
    assert_eq!(env.reserves(), (200, 300));
}

// ---------- swap ----------

#[test]
fn swap_x_for_y_pays_treasury_and_matches_curve() {
    let (mut env, _, trader) = pool(1_000_000, 1_000_000);
    let k_before = env.k();
    let trader_y = env.balance(env.ata(&trader, env.mint_y));

    // 10_000 in: protocol cut 5, LP fee 30, curve input 9_965.
    // out = 1_000_000 * 9_965 / 1_009_965 = 9_866.67 -> 9_866
    env.swap(&trader, true, 10_000, 9_866).unwrap();

    assert_eq!(env.balance(env.treasury_x), 5);
    assert_eq!(env.balance(env.treasury_y), 0);
    assert_eq!(env.reserves(), (1_000_000 + 9_995, 1_000_000 - 9_866));
    assert_eq!(
        env.balance(env.ata(&trader, env.mint_x)),
        START_BALANCE - 10_000
    );
    assert_eq!(env.balance(env.ata(&trader, env.mint_y)), trader_y + 9_866);
    assert!(env.k() > k_before);
}

#[test]
fn swap_y_for_x_pays_treasury_and_matches_curve() {
    let (mut env, _, trader) = pool(1_000_000, 2_000_000);

    // 50_000 in: protocol cut 25, LP fee 150, curve input 49_825.
    // out = 1_000_000 * 49_825 / 2_049_825 = 24_306.95 -> 24_306
    env.swap(&trader, false, 50_000, 24_306).unwrap();

    assert_eq!(env.balance(env.treasury_y), 25);
    assert_eq!(env.balance(env.treasury_x), 0);
    assert_eq!(env.reserves(), (1_000_000 - 24_306, 2_000_000 + 49_975));
    assert_eq!(
        env.balance(env.ata(&trader, env.mint_x)),
        START_BALANCE + 24_306
    );
}

#[test]
fn swap_creates_missing_output_ata() {
    let (mut env, admin, _) = pool(1_000_000, 1_000_000);
    let newcomer = Keypair::new();
    env.svm.airdrop(&newcomer.pubkey(), 1_000_000_000).unwrap();
    let mint_x = env.mint_x;
    mint_to(&mut env.svm, &admin, &mint_x, &newcomer.pubkey(), 10_000);
    assert!(env
        .svm
        .get_account(&env.ata(&newcomer, env.mint_y))
        .is_none());

    env.swap(&newcomer, true, 10_000, 1).unwrap();
    assert_eq!(env.balance(env.ata(&newcomer, env.mint_y)), 9_866);
}

#[test]
fn swap_k_never_decreases() {
    let (mut env, _, trader) = pool(1_000_000, 3_000_000);
    let mut k = env.k();
    for (i, amount) in [1, 7, 333, 10_000, 250_000, 3, 99_999, 1_234_567]
        .into_iter()
        .enumerate()
    {
        let is_x = i % 2 == 0;
        env.swap(&trader, is_x, amount, 0).unwrap();
        let k_now = env.k();
        assert!(k_now >= k, "k shrank after swap {i}: {k} -> {k_now}");
        k = k_now;
    }
}

#[test]
fn swap_rejects_slippage_zero_and_empty_pool() {
    let (mut env, admin, trader) = setup();
    env.initialize(&admin, FEE, PROTOCOL_FEE, None).unwrap();
    assert_err(env.swap(&trader, true, 10_000, 0), "NoLiquidity");

    env.deposit(&admin, 1_000_000, 1_000_000, 1_000_000)
        .unwrap();
    assert_err(env.swap(&trader, true, 10_000, 9_867), "SlippageExceeded");
    assert_err(env.swap(&trader, true, 0, 0), "InvalidAmount");
    // 1 unit in pays no fee (both round to 0) and 1 * 1e6 / (1e6 + 1) rounds to 0 out.
    assert_err(env.swap(&trader, true, 1, 0), "InvalidAmount");
    assert_eq!(env.reserves(), (1_000_000, 1_000_000));
    assert_eq!(env.balance(env.treasury_x), 0);
}

// ---------- withdraw ----------

#[test]
fn withdraw_is_proportional_and_drains_cleanly() {
    let (mut env, admin, trader) = pool(1_000_000, 1_000_000);
    env.swap(&trader, true, 10_000, 0).unwrap();
    assert_eq!(env.reserves(), (1_009_995, 990_134));
    let admin_x = env.balance(env.ata(&admin, env.mint_x));
    let admin_y = env.balance(env.ata(&admin, env.mint_y));

    // 40% of LP: floor(0.4 * 1_009_995), floor(0.4 * 990_134)
    env.withdraw(&admin, 400_000, 403_998, 396_053).unwrap();
    assert_eq!(env.reserves(), (1_009_995 - 403_998, 990_134 - 396_053));
    assert_eq!(env.balance(env.ata(&admin, env.mint_x)), admin_x + 403_998);
    assert_eq!(env.balance(env.ata(&admin, env.mint_y)), admin_y + 396_053);
    assert_eq!(env.lp_supply(), 600_000);

    // The last LP out takes everything that is left.
    env.withdraw(&admin, 600_000, 605_997, 594_081).unwrap();
    assert_eq!(env.reserves(), (0, 0));
    assert_eq!(env.lp_supply(), 0);
}

#[test]
fn withdraw_rejects_slippage_zero_and_overdraw() {
    let (mut env, admin, trader) = pool(1_000_000, 1_000_000);
    assert_err(
        env.withdraw(&admin, 100_000, 100_001, 0),
        "SlippageExceeded",
    );
    assert_err(
        env.withdraw(&admin, 100_000, 0, 100_001),
        "SlippageExceeded",
    );
    assert_err(env.withdraw(&admin, 0, 0, 0), "InvalidAmount");
    // More LP than the admin holds: the burn fails.
    assert!(env.withdraw(&admin, 1_000_001, 0, 0).is_err());
    // Trader has no LP account at all.
    assert!(env.withdraw(&trader, 1, 0, 0).is_err());
    assert_eq!(env.reserves(), (1_000_000, 1_000_000));
    assert_eq!(env.lp_supply(), 1_000_000);
}

// ---------- lock / unlock ----------

#[test]
fn lock_blocks_pool_ops_until_unlocked() {
    let (mut env, admin, trader) = pool(1_000_000, 1_000_000);

    env.set_locked(&admin, true).unwrap();
    assert!(env.config().locked);
    assert_err(env.deposit(&admin, 1_000, 1_000, 1_000), "PoolLocked");
    assert_err(env.swap(&trader, true, 10_000, 0), "PoolLocked");
    assert_err(env.withdraw(&admin, 1_000, 0, 0), "PoolLocked");

    env.set_locked(&admin, false).unwrap();
    assert!(!env.config().locked);
    env.deposit(&admin, 1_000, 1_000, 1_000).unwrap();
    env.swap(&trader, true, 10_000, 0).unwrap();
    env.withdraw(&admin, 1_000, 0, 0).unwrap();
}

#[test]
fn lock_rejects_non_authority() {
    let (mut env, admin, trader) = pool(1_000_000, 1_000_000);
    assert_err(env.set_locked(&trader, true), "Unauthorized");
    assert!(!env.config().locked);

    env.set_locked(&admin, true).unwrap();
    assert_err(env.set_locked(&trader, false), "Unauthorized");
    assert!(env.config().locked);
}

#[test]
fn pool_without_authority_cannot_be_locked() {
    let (mut env, admin, _) = setup();
    env.initialize(&admin, FEE, PROTOCOL_FEE, None).unwrap();
    assert_err(env.set_locked(&admin, true), "Unauthorized");
    assert_err(env.set_locked(&admin, false), "Unauthorized");
    assert!(!env.config().locked);
}
