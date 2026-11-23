//! emission_test.rs: ErgoTree evaluation tests for the emission contract v1.3.
//!
//! ErgoTree compiled offline with ergo-appkit and sigma-state (see the note at
//! EMISSION_TREE_HEX below), not through a node endpoint. This bypasses sigma-rust's limited
//! ErgoScript compiler (which can't parse nested typed-lambdas in the terminal
//! path `nftBurned` check) while still running full prove/verify in Rust.
//!
//! Targets: ergo-lib 0.28 with "arbitrary" feature, rustc 1.85+

use std::convert::TryInto;

use ergo_lib::ergo_chain_types::{Digest32, EcPoint, Header, PreHeader};
use ergo_lib::ergotree_interpreter::{
    eval::{context::Context, reduce_to_crypto},
    sigma_protocol::{
        prover::{hint::HintsBag, ContextExtension, Prover, TestProver},
        verifier::{TestVerifier, Verifier},
    },
};
use ergo_lib::ergotree_ir::{
    chain::{
        ergo_box::{
            box_value::BoxValue, ErgoBox, ErgoBoxCandidate, NonMandatoryRegisterId,
            NonMandatoryRegisters,
        },
        token::{Token, TokenAmount, TokenId},
        tx_id::TxId,
    },
    ergo_tree::ErgoTree,
    mir::constant::{Constant, TryExtractInto},
    serialization::SigmaSerializable,
    sigma_protocol::sigma_boolean::SigmaBoolean,
};
use blake2::{digest::consts::U32, Blake2b, Digest};
use sigma_test_util::force_any_val;

type Blake2b256 = Blake2b<U32>;

// ============================================================
// CONSTANTS
// ============================================================

const NANOCOIN: u64 = 1_000_000_000;
const BLOCKS_PER_HALVING: u32 = 1_051_200; // 1 year (365 d) at 30 s blocks
const INITIAL_REWARD: u64 = 75 * NANOCOIN;
const MIN_REWARD: u64 = 1_500_000_000; // 1.5 coins: tail floor
const NUM_EXPLICIT_HALVINGS: u32 = 6; // arms for halvings 0..5
const MAX_HALVINGS: u32 = 20; // chain years in the schedule total
const TREASURY_PCT: u64 = 4;
const LP_PCT: u64 = 3;

/// Total supply, locked 2026-11-09 (30 s block package): the schedule total
/// over 20 chain years. `schedule_total()` derives it;
/// `model_tests::genesis_value_matches_spec` pins the derivation.
const TOTAL_SUPPLY: u64 = 177_291_450_000_000_000;

/// Genesis treasury fund box and genesis LP fund box, 100 YOLO each
/// (decided 2026-11-12). The 200 YOLO comes out of the emission box.
const GENESIS_FUND_BOX: u64 = 100 * NANOCOIN;

/// Genesis emission box (2026-11-12): TOTAL_SUPPLY - 2 x GENESIS_FUND_BOX.
const GENESIS_EMISSION_BOX: u64 = 177_291_250_000_000_000;

/// Real chain: last normal spend (box 2.5 YOLO, successor 1.0) and the
/// terminal spend (box 1.0 YOLO, below the 1.5 reward).
const REAL_LAST_NORMAL: u32 = 21_023_915;
const REAL_TERMINAL: u32 = 21_023_916;

// ErgoTree of emission.es v1.3 (702 bytes), written to trees/emission.hex by
// 05-integration-tests/compile/gen_trees.py from the source in this crate
// (sigma-state 6.0.7, offline) with the test emission NFT id cc..cc
// (EMISSION_NFT_ID_PLACEHOLDER). `gen_trees.py --check` recompiles and compares.
// v1.4 (2026-11-19): split 93 / 4 / 3. Constants 19, 23, 25 and 28 changed
// (Long 10 -> 4 and 5 -> 3, twice); same length, same template.
// v1.3 was 702 bytes, blake2b256 72393fc5...0b25.
// v1.3 (2026-11 retest): the NFT id is constant 0 and SELF.tokens(0) must equal
// (id, 1); the miner output must hold no tokens; the normal successor must hold
// exactly one token and no R6. 41 constants: the reward script prefix and
// suffix are constants 32 and 33, the output counts 4 and 3 are constants 39
// and 40, and the terminal LE guard (0x90) is at offset 616.
// v1.2 (2026-11 review, F-009): each of the four share outputs must have no
// tokens, no registers and creation height HEIGHT (the `plainShare` lambda).
// That adds one segregated constant (Int 0 for tokens.size == 0) ahead of the
// miner script constants, so the reward script prefix and suffix are now
// constants 30 and 31 of 38, and the terminal LE guard (0x90) is at offset 552.
// History: v1.1 was 577 bytes, blake2b256 06b9279a...9a4e, LE at offset 505;
// the 455-byte tree 630f1337...d095 is v1.1 without the miner binding.
// v1.2 was 638 bytes, blake2b256 56e7731d...4a11, LE at offset 552.
// blake2b256 = 91eb745f2b44f255327a1e8720196944a34921e9e65e39bc275a406557fc0fe5
const EMISSION_TREE_HEX: &str = include_str!("../trees/emission.hex");

// Treasury/LP test script: sigmaProp(HEIGHT > 0) — compiled by same node
const TREASURY_LP_TREE_HEX: &str = "10010400d191a37300";

// LP test script: sigmaProp(HEIGHT > 1). Differs from the treasury script so
// R4 and R5 hold different hashes and a treasury/LP swap is detectable.
// (HEIGHT > 1 is only evaluated if the LP box is spent; these tests never do.)
const LP_TREE_HEX: &str = "10010402d191a37300";

// ============================================================
// MINER REWARD SCRIPT (Gap C binding, 2026-11-10)
// ============================================================
//
// Serialized ErgoTreePredef.rewardOutputScript(delay, ProveDlog(pk)), taken
// from sigma-state 6.0.7 and 6.0.2 on the JVM: prefix ++ pk(33) ++ suffix,
// 54 bytes. Only the 33 pk bytes vary between keys, and only the delay
// bytes (04 a00b = Int 720, 04 802d = Int 2880) vary between delays.

const MINER_REWARD_DELAY: u32 = 2_880; // 24 h at 30 s blocks
const MINER_SCRIPT_PREFIX_HEX: &str = "100204802d08cd"; // delay 2880
const MINER_SCRIPT_PREFIX_720_HEX: &str = "100204a00b08cd"; // Ergo mainnet delay 720
const MINER_SCRIPT_SUFFIX_HEX: &str = "ea02d192a39a8cc7a70173007301";

// Compressed secp256k1 keys k*G (k = 1, 2, 12345, 0xdeadbeefcafebabe0123456789abcdef).
const PK_A: &str = "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";
const PK_B: &str = "02c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5";
const PK_C: &str = "03f01d6b9018ab421dd410404cb869072065522bf85734008f105cf385a023a80f";
const PK_POOL: &str = "0350c28934e2d2c0db8df35775aded53bce9e367460c2d70b1f304655e0a978cb4";

/// Header key used by `build_context` unless a test picks another one.
const DEFAULT_MINER_PK: &str = PK_A;

fn reward_script_hex_with(prefix: &str, pk_hex: &str) -> String {
    format!("{}{}{}", prefix, pk_hex, MINER_SCRIPT_SUFFIX_HEX)
}

/// Expected miner output script for `pk_hex` (delay 2880).
fn reward_script_hex(pk_hex: &str) -> String {
    reward_script_hex_with(MINER_SCRIPT_PREFIX_HEX, pk_hex)
}

fn reward_tree(pk_hex: &str) -> ErgoTree {
    load_tree(&reward_script_hex(pk_hex))
}

fn ec_point(pk_hex: &str) -> EcPoint {
    EcPoint::from_base16_str(pk_hex.to_string()).expect("valid compressed point")
}

/// Correct miner output for the default header key.
fn make_miner_box(value: u64, creation_height: u32) -> ErgoBox {
    make_output_box(&reward_tree(DEFAULT_MINER_PK), value, creation_height)
}

// ============================================================
// MODEL FUNCTIONS
// ============================================================

fn block_reward(height: u32) -> u64 {
    let halvings = height / BLOCKS_PER_HALVING;
    let computed = if halvings == 0 {
        INITIAL_REWARD
    } else if halvings == 1 {
        INITIAL_REWARD / 2
    } else if halvings == 2 {
        INITIAL_REWARD / 4
    } else if halvings == 3 {
        INITIAL_REWARD / 8
    } else if halvings == 4 {
        INITIAL_REWARD / 16
    } else if halvings == 5 {
        INITIAL_REWARD / 32
    } else {
        MIN_REWARD
    };
    std::cmp::max(computed, MIN_REWARD)
}

fn split_reward(reward: u64) -> (u64, u64, u64) {
    let treasury = reward * TREASURY_PCT / 100;
    let lp = reward * LP_PCT / 100;
    let miner = reward - treasury - lp;
    (miner, treasury, lp)
}

/// Sum of every epoch's emission over MAX_HALVINGS (= 20) chain years,
/// routed through `block_reward` so it follows the contract's switch.
/// This is the total supply.
fn schedule_total() -> u64 {
    let mut total: u64 = 0;
    for h in 0..MAX_HALVINGS {
        let reward = block_reward(h * BLOCKS_PER_HALVING);
        total += reward * BLOCKS_PER_HALVING as u64;
    }
    total
}

/// Genesis emission box value (2026-11-12): the schedule total minus the
/// 100 YOLO treasury fund box and the 100 YOLO LP fund box.
fn genesis_box_value() -> u64 {
    schedule_total() - 2 * GENESIS_FUND_BOX
}

/// Rewards for every height in [0, height).
fn paid_before(height: u32) -> u64 {
    let mut paid: u64 = 0;
    let full_epochs = height / BLOCKS_PER_HALVING;
    for e in 0..full_epochs {
        paid += block_reward(e * BLOCKS_PER_HALVING) * BLOCKS_PER_HALVING as u64;
    }
    paid += block_reward(height) * (height % BLOCKS_PER_HALVING) as u64;
    paid
}

/// Schedule view: the full schedule total (total supply) minus every reward
/// for heights [0, height), same convention as
/// emission_model.emission_box_value_at(height, start=total_supply()). It
/// runs out exactly at the end of chain year 20. Not the real box: the real
/// box holds 200 YOLO less (fund boxes) and height 0 is never mined; see
/// `real_box_value`.
fn box_value_before(height: u32) -> u64 {
    schedule_total() - paid_before(height)
}

/// Real-chain emission box value immediately before the spend at `height`
/// (height >= 1): genesis box minus the rewards for heights 1 .. height-1
/// (height 0 is never mined). Panics past the terminal spend at 21,023,916.
fn real_box_value(height: u32) -> u64 {
    (genesis_box_value() + block_reward(0))
        .checked_sub(paid_before(height))
        .expect("real box already spent out at this height")
}

// ============================================================
// ERGOTREE LOADING
// ============================================================

fn hex_to_bytes(hex: &str) -> Vec<u8> {
    let hex = hex.trim();
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect()
}

fn load_tree(hex: &str) -> ErgoTree {
    let bytes = hex_to_bytes(hex);
    ErgoTree::sigma_parse_bytes(&bytes).expect("valid ErgoTree")
}

fn load_emission_tree() -> ErgoTree {
    load_tree(EMISSION_TREE_HEX)
}

fn load_treasury_tree() -> ErgoTree {
    load_tree(TREASURY_LP_TREE_HEX)
}

fn load_lp_tree() -> ErgoTree {
    load_tree(LP_TREE_HEX)
}

// ============================================================
// HELPERS
// ============================================================

fn proposition_hash(tree: &ErgoTree) -> Vec<u8> {
    let bytes = tree.sigma_serialize_bytes().expect("serialize tree");
    let mut h = Blake2b256::new();
    h.update(&bytes);
    h.finalize().to_vec()
}

/// Test emission NFT id compiled into the tree (EMISSION_NFT_ID_PLACEHOLDER).
fn emission_nft_id() -> TokenId {
    TokenId::from(Digest32::from([0xCCu8; 32]))
}

fn make_emission_box(
    tree: &ErgoTree,
    value: u64,
    treasury_hash: &[u8],
    lp_hash: &[u8],
    creation_height: u32,
) -> ErgoBox {
    let nft = Token {
        token_id: emission_nft_id(),
        amount: TokenAmount::try_from(1u64).unwrap(),
    };
    let mut regs = std::collections::HashMap::new();
    regs.insert(NonMandatoryRegisterId::R4, Constant::from(treasury_hash.to_vec()));
    regs.insert(NonMandatoryRegisterId::R5, Constant::from(lp_hash.to_vec()));

    let candidate = ErgoBoxCandidate {
        value: BoxValue::try_from(value).unwrap(),
        ergo_tree: tree.clone(),
        tokens: Some(vec![nft].try_into().unwrap()),
        additional_registers: NonMandatoryRegisters::new(regs).unwrap(),
        creation_height,
    };
    ErgoBox::from_box_candidate(&candidate, TxId::zero(), 0).unwrap()
}

fn make_next_emission_box(prev: &ErgoBox, new_value: u64, creation_height: u32) -> ErgoBox {
    let candidate = ErgoBoxCandidate {
        value: BoxValue::try_from(new_value).unwrap(),
        ergo_tree: prev.ergo_tree.clone(),
        tokens: prev.tokens.clone(),
        additional_registers: prev.additional_registers.clone(),
        creation_height,
    };
    ErgoBox::from_box_candidate(&candidate, TxId::zero(), 0).unwrap()
}

fn make_output_box(tree: &ErgoTree, value: u64, creation_height: u32) -> ErgoBox {
    let candidate = ErgoBoxCandidate {
        value: BoxValue::try_from(value).unwrap(),
        ergo_tree: tree.clone(),
        tokens: None,
        additional_registers: NonMandatoryRegisters::empty(),
        creation_height,
    };
    ErgoBox::from_box_candidate(&candidate, TxId::zero(), 0).unwrap()
}

fn make_output_box_with_nft(tree: &ErgoTree, value: u64, creation_height: u32) -> ErgoBox {
    let nft = Token {
        token_id: emission_nft_id(),
        amount: TokenAmount::try_from(1u64).unwrap(),
    };
    let candidate = ErgoBoxCandidate {
        value: BoxValue::try_from(value).unwrap(),
        ergo_tree: tree.clone(),
        tokens: Some(vec![nft].try_into().unwrap()),
        additional_registers: NonMandatoryRegisters::empty(),
        creation_height,
    };
    ErgoBox::from_box_candidate(&candidate, TxId::zero(), 0).unwrap()
}

/// Build Context with the default header miner key.
fn build_context(self_box: ErgoBox, outputs: Vec<ErgoBox>, height: u32) -> Context<'static> {
    build_context_pk(self_box, outputs, height, DEFAULT_MINER_PK)
}

/// Build Context. Headers and the rest of the PreHeader come from proptest
/// Arbitrary impls (the contract never reads them); the PreHeader miner key
/// is set to `miner_pk_hex`, because the contract reads CONTEXT.minerPubKey.
fn build_context_pk(
    self_box: ErgoBox,
    outputs: Vec<ErgoBox>,
    height: u32,
    miner_pk_hex: &str,
) -> Context<'static> {
    let self_ref: &'static ErgoBox = Box::leak(Box::new(self_box));
    let outputs_static: &'static [ErgoBox] = Vec::leak(outputs);
    let inputs_arr: [&'static ErgoBox; 1] = [self_ref];
    let inputs = inputs_arr.into();

    let mut pre_header = force_any_val::<PreHeader>();
    pre_header.miner_pk = Box::new(ec_point(miner_pk_hex));
    pre_header.height = height;
    let headers = force_any_val::<[Header; 10]>();

    Context {
        height,
        self_box: self_ref,
        outputs: outputs_static,
        data_inputs: None,
        inputs,
        pre_header,
        headers,
        extension: ContextExtension::empty(),
    }
}

fn evaluate(tree: &ErgoTree, ctx: &Context) -> bool {
    let prover = TestProver { secrets: vec![] };
    let message = vec![0u8; 32];

    let proof = match prover.prove(tree, ctx, message.as_slice(), &HintsBag::empty()) {
        Ok(p) => p.proof,
        Err(_) => return false,
    };

    let verifier = TestVerifier;
    match verifier.verify(tree, ctx, proof, message.as_slice()) {
        Ok(v) => v.result,
        Err(_) => false,
    }
}

/// Reduce the tree in `ctx`: Ok(b) when the script evaluates to the trivial
/// proposition `b`, Err when evaluation fails or the result is not trivial.
/// Used to tell a script-false reject from an evaluation error.
fn reduce(tree: &ErgoTree, ctx: &Context) -> Result<bool, String> {
    let expr = tree.proposition().map_err(|e| format!("tree parse: {:?}", e))?;
    match reduce_to_crypto(&expr, ctx) {
        Ok(r) => match r.sigma_prop {
            SigmaBoolean::TrivialProp(b) => Ok(b),
            other => Err(format!("non-trivial result: {:?}", other)),
        },
        Err(e) => Err(format!("evaluation error: {:?}", e)),
    }
}

/// Accept: reduces to true and the prove/verify round trip succeeds.
fn assert_accepts(tree: &ErgoTree, ctx: &Context, msg: &str) {
    assert_eq!(reduce(tree, ctx), Ok(true), "{}", msg);
    assert!(evaluate(tree, ctx), "{} (prove/verify)", msg);
}

/// Reject by the script itself: reduces to false without an evaluation error.
fn assert_script_false(tree: &ErgoTree, ctx: &Context, msg: &str) {
    assert_eq!(reduce(tree, ctx), Ok(false), "{}", msg);
    assert!(!evaluate(tree, ctx), "{} (prove/verify)", msg);
}

/// Build a valid normal-path scenario at any height.
/// Returns (emission_tree, self_box, outputs, height) ready for build_context.
fn normal_spend_scenario(h: u32, self_val: u64, creation_height: u32) -> (ErgoTree, ErgoBox, Vec<ErgoBox>, u32) {
    let emission_tree = load_emission_tree();
    let treasury_tree = load_treasury_tree();
    let lp_tree = load_lp_tree();
    let t_hash = proposition_hash(&treasury_tree);
    let l_hash = proposition_hash(&lp_tree);

    let self_box = make_emission_box(&emission_tree, self_val, &t_hash, &l_hash, creation_height);
    let reward = block_reward(h);
    let (m_amt, t_amt, l_amt) = split_reward(reward);

    let outputs = vec![
        make_next_emission_box(&self_box, self_val - reward, h),
        make_output_box(&treasury_tree, t_amt, h),
        make_output_box(&lp_tree, l_amt, h),
        make_miner_box(m_amt, h),
    ];

    (emission_tree, self_box, outputs, h)
}

// ============================================================
// TESTS
// ============================================================

#[cfg(test)]
mod model_tests {
    use super::*;

    #[test]
    fn reward_at_epoch_boundaries() {
        assert_eq!(block_reward(0), 75 * NANOCOIN);
        assert_eq!(block_reward(BLOCKS_PER_HALVING), 37_500_000_000);
        assert_eq!(block_reward(5 * BLOCKS_PER_HALVING), 2_343_750_000);
        assert_eq!(block_reward(6 * BLOCKS_PER_HALVING), MIN_REWARD);
    }

    #[test]
    fn genesis_value_matches_spec() {
        // Total supply = 6 arm years + 14 tail years, at 1_051_200 blocks/year:
        // 1_051_200 x (75 + 37.5 + 18.75 + 9.375 + 4.6875 + 2.34375)
        //   + 14 x 1_051_200 x 1.5 = 155_216_250 + 22_075_200 = 177_291_450 YOLO.
        // Since 2026-11-12 the genesis emission box holds 177_291_250 YOLO and
        // the treasury and LP fund boxes hold 100 YOLO each.
        assert_eq!(schedule_total(), TOTAL_SUPPLY);
        assert_eq!(genesis_box_value(), GENESIS_EMISSION_BOX);
        assert_eq!(GENESIS_EMISSION_BOX + 2 * GENESIS_FUND_BOX, TOTAL_SUPPLY);
        let arm_years: u64 = (0..NUM_EXPLICIT_HALVINGS)
            .map(|e| (INITIAL_REWARD >> e) * BLOCKS_PER_HALVING as u64)
            .sum();
        let tail_years: u64 =
            (MAX_HALVINGS - NUM_EXPLICIT_HALVINGS) as u64 * MIN_REWARD * BLOCKS_PER_HALVING as u64;
        assert_eq!(arm_years, 155_216_250 * NANOCOIN);
        assert_eq!(tail_years, 22_075_200 * NANOCOIN);
        assert_eq!(arm_years + tail_years, TOTAL_SUPPLY);
    }

    #[test]
    fn six_arms_then_tail_and_every_split_integral() {
        let expected: [u64; 7] = [
            75_000_000_000,
            37_500_000_000,
            18_750_000_000,
            9_375_000_000,
            4_687_500_000,
            2_343_750_000,
            1_500_000_000, // tail
        ];
        for (e, want) in expected.iter().enumerate() {
            let r = block_reward(e as u32 * BLOCKS_PER_HALVING);
            assert_eq!(r, *want, "epoch {}", e);
            // 4% / 3% split exact in nanocoins, no remainder anywhere.
            assert_eq!(r * TREASURY_PCT % 100, 0, "treasury split inexact at epoch {}", e);
            assert_eq!(r * LP_PCT % 100, 0, "LP split inexact at epoch {}", e);
            // Year total integral in whole coins.
            assert_eq!(r * BLOCKS_PER_HALVING as u64 % NANOCOIN, 0, "epoch {} year total", e);
        }
        for e in 6..(MAX_HALVINGS + 5) {
            assert_eq!(block_reward(e * BLOCKS_PER_HALVING), MIN_REWARD, "epoch {}", e);
        }
    }

    #[test]
    fn schedule_total_exhausts_exactly_at_end_of_year_20() {
        // Schedule view (total supply, height 0 paid).
        let end = MAX_HALVINGS * BLOCKS_PER_HALVING; // 21_024_000
        assert_eq!(box_value_before(end - 1), MIN_REWARD);
        assert_eq!(box_value_before(end), 0);
        // Real chain: the box holds 200 coins less (fund boxes) and height 0
        // is never mined (75 coins unpaid), so it is 125 coins short of the
        // schedule at `end` and runs out 84 blocks earlier.
        assert_eq!(paid_before(end) - block_reward(0) - genesis_box_value(), 125 * NANOCOIN);
        assert_eq!(end - REAL_TERMINAL, 84);
    }

    #[test]
    fn real_box_runs_out_at_21_023_916() {
        // 2026-11-12 genesis. Last normal spend at 21_023_915 from 2.5 coins
        // (successor 1.0); terminal spend at 21_023_916 from 1.0 coin, below
        // the 1.5 reward, split 0.04 / 0.03 / 0.93.
        assert_eq!(REAL_LAST_NORMAL + 1, REAL_TERMINAL);
        assert_eq!(real_box_value(REAL_LAST_NORMAL), 2_500_000_000);
        assert_eq!(block_reward(REAL_LAST_NORMAL), MIN_REWARD);
        assert!(real_box_value(REAL_LAST_NORMAL) > MIN_REWARD);
        assert_eq!(real_box_value(REAL_TERMINAL), 1_000_000_000);
        assert!(real_box_value(REAL_TERMINAL) < block_reward(REAL_TERMINAL));
        assert_eq!(real_box_value(1), GENESIS_EMISSION_BOX);
        let v = real_box_value(REAL_TERMINAL);
        let (t, l) = (v * TREASURY_PCT / 100, v * LP_PCT / 100);
        assert_eq!((t, l, v - t - l), (40_000_000, 30_000_000, 930_000_000));
        // Total paid on the real chain, heights 1 ..= 21_023_916, equals the
        // emission box; adding the fund boxes gives the total supply.
        let total_paid = paid_before(REAL_TERMINAL) - block_reward(0) + v;
        assert_eq!(total_paid, GENESIS_EMISSION_BOX);
        assert_eq!(total_paid + 2 * GENESIS_FUND_BOX, TOTAL_SUPPLY);
    }
}

#[cfg(test)]
mod phase0_load {
    use super::*;

    #[test]
    fn emission_ergotree_loads_and_has_valid_size() {
        let tree = load_emission_tree();
        let size = tree.sigma_serialize_bytes().unwrap().len();
        println!("emission contract ErgoTree size: {} bytes", size);
        assert!(size > 100, "ErgoTree too small — likely corrupt");
    }

    #[test]
    fn treasury_lp_ergotree_loads() {
        let t = load_treasury_tree();
        let l = load_lp_tree();
        assert!(t.sigma_serialize_bytes().unwrap().len() > 0);
        assert!(l.sigma_serialize_bytes().unwrap().len() > 0);
    }

    #[test]
    fn round_trip_serialization_matches() {
        let tree = load_emission_tree();
        let bytes = tree.sigma_serialize_bytes().unwrap();
        let original = hex_to_bytes(EMISSION_TREE_HEX);
        assert_eq!(bytes, original, "ErgoTree round-trip serialization mismatch");
    }

    #[test]
    fn emission_tree_proposition_parses() {
        let tree = load_emission_tree();
        tree.proposition().expect("emission tree proposition should parse");
    }

    #[test]
    fn emission_tree_pins_terminal_le_fix() {
        // 2026-11-10: `insufficient` is `SELF.value <= blockReward`. With the
        // miner binding, the F-009 share pins and the v1.3 token pins the tree
        // is 702 bytes; byte 616 is LE (0x90) applied to ValUse(4) = SELF.value
        // and ValUse(7) = blockReward, and the hash is the one quoted above
        // EMISSION_TREE_HEX.
        let bytes = hex_to_bytes(EMISSION_TREE_HEX);
        assert_eq!(bytes.len(), 702);
        assert_eq!(bytes[616], 0x90, "terminal comparison must be LE");
        assert_eq!(&bytes[617..621], &[0x72, 0x04, 0x72, 0x07]);
        // ValDef 4 = SELF.value (c1 a7), ValDef 7 = blockReward (the clamp).
        let hex = EMISSION_TREE_HEX;
        assert!(hex.contains("d604c1a7"), "ValDef 4 must be SELF.value");
        assert!(hex.contains("d60795917206731072067311"), "ValDef 7 must be the reward clamp");
        let mut h = Blake2b256::new();
        h.update(&bytes);
        let digest: String = h.finalize().iter().map(|b| format!("{:02x}", b)).collect();
        assert_eq!(digest, "91eb745f2b44f255327a1e8720196944a34921e9e65e39bc275a406557fc0fe5");
    }

    #[test]
    fn emission_tree_pins_miner_binding() {
        // The reward script prefix and suffix are segregated constants 32
        // and 33 of the tree (Coll[Byte], type 0x0e), in this order (30 and 31
        // in v1.2, before the v1.3 NFT id and token pins added constants).
        let tree = load_emission_tree();
        let c29 = format!("0e07{}", MINER_SCRIPT_PREFIX_HEX);
        let c30 = format!("0e0e{}", MINER_SCRIPT_SUFFIX_HEX);
        let both = format!("{}{}", c29, c30);
        assert!(EMISSION_TREE_HEX.contains(&both), "prefix/suffix constants missing");
        let consts = tree.get_constants().expect("constants");
        assert_eq!(consts.len(), 41);
        let as_bytes = |i: usize| -> Vec<u8> {
            consts[i].clone().v.try_extract_into::<Vec<u8>>().expect("Coll[Byte] constant")
        };
        assert_eq!(as_bytes(32), hex_to_bytes(MINER_SCRIPT_PREFIX_HEX));
        assert_eq!(as_bytes(33), hex_to_bytes(MINER_SCRIPT_SUFFIX_HEX));
        // Root: selfIsEmission && heightIncreased && minerOutBound &&
        // (normal || terminal), with selfIsEmission = SELF.tokens(0) ==
        // (ValUse(1) = constant NFT id, 1) and minerOutBound =
        // (minerOut.propositionBytes == prefix ++ CONTEXT.minerPubKey ++ suffix)
        // && minerOut.creationInfo._1 == HEIGHT && minerOut.tokens.size == 0.
        // db 65 0a fe = PropertyCall(SContext, method 10 = minerPubKey, Context).
        // ValDef 3 = OUTPUTS(OUTPUTS.size - 1) (ValDef 2 = OUTPUTS.size).
        assert!(EMISSION_TREE_HEX.contains("d6017300d602b1a5d603b2a5997202730100"));
        assert!(EMISSION_TREE_HEX.contains(
            "d1ededed93b2db6308a7731e0086027201731f91a38cc7a701eded93c27203b3b37320db650afe7321938cc7720301a393b1db630872037322ec"
        ));
        // validMiner: OUTPUTS.size == c39 (4) && minerOut.value == R - t - l.
        assert!(EMISSION_TREE_HEX.contains("ed937202732793c172039999720772137215"));
        // validTermMiner: OUTPUTS.size == c40 (3) && value == SELF.value - t - l.
        assert!(EMISSION_TREE_HEX.contains("ed937202732893c1720399997204720a7210"));
        assert_eq!(consts[39].clone().v.try_extract_into::<i32>().unwrap(), 4);
        assert_eq!(consts[40].clone().v.try_extract_into::<i32>().unwrap(), 3);
        // Constant 0 is the emission NFT id.
        assert_eq!(as_bytes(0), vec![0xCCu8; 32]);
    }

    #[test]
    fn reward_script_template_matches_scala_reward_output_script() {
        // Full 54-byte trees printed by sigma-state's
        // ErgoTreePredef.rewardOutputScript(2880, ProveDlog(k*G)) on the JVM.
        let scala_2880 = [
            (PK_A, "100204802d08cd0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798ea02d192a39a8cc7a70173007301"),
            (PK_B, "100204802d08cd02c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5ea02d192a39a8cc7a70173007301"),
            (PK_C, "100204802d08cd03f01d6b9018ab421dd410404cb869072065522bf85734008f105cf385a023a80fea02d192a39a8cc7a70173007301"),
            (PK_POOL, "100204802d08cd0350c28934e2d2c0db8df35775aded53bce9e367460c2d70b1f304655e0a978cb4ea02d192a39a8cc7a70173007301"),
        ];
        for (pk, scala) in scala_2880.iter() {
            assert_eq!(reward_script_hex(pk), *scala);
            let t = reward_tree(pk);
            assert_eq!(t.sigma_serialize_bytes().unwrap(), hex_to_bytes(scala), "round trip");
            assert_eq!(hex_to_bytes(scala).len(), 54);
            // sigma-rust reads the same two constants: Int delay, then the key.
            let consts = t.get_constants().unwrap();
            assert_eq!(consts.len(), 2);
            assert_eq!(consts[0].clone().v.try_extract_into::<i32>().unwrap(), MINER_REWARD_DELAY as i32);
            // minerPubKey as sigma-rust returns it is the same 33 bytes.
            let pk_bytes = ec_point(pk).sigma_serialize_bytes().unwrap();
            assert_eq!(pk_bytes, hex_to_bytes(pk));
            assert_eq!(pk_bytes.len(), 33);
        }
        // Same template with delay 720 is Ergo mainnet's real miner reward
        // script: the reward box of mainnet block 1,786,180 (captured from a
        // Scala node).
        let mainnet_pk = "03224c2f2388ae0741be2c50727caa49bd62654dc1f36ee72392b187b78da2c717";
        assert_eq!(
            reward_script_hex_with(MINER_SCRIPT_PREFIX_720_HEX, mainnet_pk),
            "100204a00b08cd03224c2f2388ae0741be2c50727caa49bd62654dc1f36ee72392b187b78da2c717ea02d192a39a8cc7a70173007301"
        );
        // 720 and 2880 differ only in the two delay bytes (VLQ of zigzag).
        let a = hex_to_bytes(&reward_script_hex_with(MINER_SCRIPT_PREFIX_720_HEX, PK_A));
        let b = hex_to_bytes(&reward_script_hex(PK_A));
        let diff: Vec<usize> = (0..54).filter(|&i| a[i] != b[i]).collect();
        assert_eq!(diff, vec![3, 4]);
    }

    #[test]
    fn reward_script_delay_counts_from_box_creation_height() {
        // What the delay script reads: HEIGHT >= SELF.creationInfo._1 + 2880.
        // A reward box created at `ch` is locked through ch + 2879 and opens
        // to proveDlog(pk) at ch + 2880. A box declaring creation height 0
        // would be open immediately, which is why the emission contract pins
        // the miner output's creation height to HEIGHT.
        let t = reward_tree(PK_A);
        let expr = t.proposition().unwrap();
        let ch = 500_000u32;
        let reward_box = make_output_box(&t, NANOCOIN, ch);
        let at = |h: u32, b: &ErgoBox| {
            let ctx = build_context(b.clone(), vec![make_output_box(&t, NANOCOIN, h)], h);
            reduce_to_crypto(&expr, &ctx).expect("reduces").sigma_prop
        };
        assert_eq!(at(ch + MINER_REWARD_DELAY - 1, &reward_box), SigmaBoolean::TrivialProp(false));
        match at(ch + MINER_REWARD_DELAY, &reward_box) {
            SigmaBoolean::ProofOfKnowledge(_) => {}
            other => panic!("expected proveDlog(pk) at maturity, got {:?}", other),
        }
        let backdated = make_output_box(&t, NANOCOIN, 0);
        match at(ch + 1, &backdated) {
            SigmaBoolean::ProofOfKnowledge(_) => {}
            other => panic!("backdated box should be open at once, got {:?}", other),
        }
    }
}

#[cfg(test)]
mod phase2_normal_spend {
    use super::*;

    #[test]
    fn accept_normal_spend_at_height_1() {
        let (tree, self_box, outputs, h) = normal_spend_scenario(1, genesis_box_value(), 0);
        let ctx = build_context(self_box, outputs, h);
        assert_accepts(&tree, &ctx, "normal spend at h=1 should accept");
    }

    #[test]
    fn accept_normal_spend_at_height_100() {
        let h = 100u32;
        let val = genesis_box_value() - block_reward(0) * 99; // after 99 blocks
        let (tree, self_box, outputs, h) = normal_spend_scenario(h, val, h - 1);
        let ctx = build_context(self_box, outputs, h);
        assert_accepts(&tree, &ctx, "normal spend at h=100 should accept");
    }
}

#[cfg(test)]
mod phase3_halving_boundary {
    use super::*;

    #[test]
    fn accept_spend_at_first_halving() {
        let h = BLOCKS_PER_HALVING;
        // Value after all epoch-0 rewards are spent
        let val = genesis_box_value() - INITIAL_REWARD * BLOCKS_PER_HALVING as u64;
        let (tree, self_box, outputs, h) = normal_spend_scenario(h, val, h - 1);
        let ctx = build_context(self_box, outputs, h);
        assert_accepts(&tree, &ctx, "spend at first halving boundary should accept");
    }

    #[test]
    fn accept_spend_at_second_halving() {
        let h = 2 * BLOCKS_PER_HALVING;
        let val = genesis_box_value()
            - INITIAL_REWARD * BLOCKS_PER_HALVING as u64
            - (INITIAL_REWARD / 2) * BLOCKS_PER_HALVING as u64;
        let (tree, self_box, outputs, h) = normal_spend_scenario(h, val, h - 1);
        let ctx = build_context(self_box, outputs, h);
        assert_accepts(&tree, &ctx, "spend at second halving boundary should accept");
    }
}

#[cfg(test)]
mod phase4_underpay_treasury {
    use super::*;

    #[test]
    fn reject_treasury_underpaid() {
        let h = 1u32;
        let emission_tree = load_emission_tree();
        let treasury_tree = load_treasury_tree();
        let lp_tree = load_lp_tree();
        let t_hash = proposition_hash(&treasury_tree);
        let l_hash = proposition_hash(&lp_tree);

        let self_val = genesis_box_value();
        let self_box = make_emission_box(&emission_tree, self_val, &t_hash, &l_hash, 0);
        let reward = block_reward(h);
        let (m_amt, t_amt, l_amt) = split_reward(reward);

        let outputs = vec![
            make_next_emission_box(&self_box, self_val - reward, h),
            make_output_box(&treasury_tree, t_amt - 1, h), // underpay by 1 nanoERG
            make_output_box(&lp_tree, l_amt, h),
            make_miner_box(m_amt, h),
        ];
        let ctx = build_context(self_box, outputs, h);
        assert_script_false(&emission_tree, &ctx, "underpaid treasury should reject");
    }
}

#[cfg(test)]
mod phase5_overpay_treasury {
    use super::*;

    #[test]
    fn reject_treasury_overpaid() {
        let h = 1u32;
        let emission_tree = load_emission_tree();
        let treasury_tree = load_treasury_tree();
        let lp_tree = load_lp_tree();
        let t_hash = proposition_hash(&treasury_tree);
        let l_hash = proposition_hash(&lp_tree);

        let self_val = genesis_box_value();
        let self_box = make_emission_box(&emission_tree, self_val, &t_hash, &l_hash, 0);
        let reward = block_reward(h);
        let (m_amt, t_amt, l_amt) = split_reward(reward);

        let outputs = vec![
            make_next_emission_box(&self_box, self_val - reward, h),
            make_output_box(&treasury_tree, t_amt + 1, h), // overpay by 1 nanoERG
            make_output_box(&lp_tree, l_amt, h),
            make_miner_box(m_amt, h),
        ];
        let ctx = build_context(self_box, outputs, h);
        assert_script_false(&emission_tree, &ctx, "overpaid treasury should reject (strict ==)");
    }

    #[test]
    fn reject_lp_overpaid() {
        let h = 1u32;
        let emission_tree = load_emission_tree();
        let treasury_tree = load_treasury_tree();
        let lp_tree = load_lp_tree();
        let t_hash = proposition_hash(&treasury_tree);
        let l_hash = proposition_hash(&lp_tree);

        let self_val = genesis_box_value();
        let self_box = make_emission_box(&emission_tree, self_val, &t_hash, &l_hash, 0);
        let reward = block_reward(h);
        let (m_amt, t_amt, l_amt) = split_reward(reward);

        let outputs = vec![
            make_next_emission_box(&self_box, self_val - reward, h),
            make_output_box(&treasury_tree, t_amt, h),
            make_output_box(&lp_tree, l_amt + 1, h), // overpay LP
            make_miner_box(m_amt, h),
        ];
        let ctx = build_context(self_box, outputs, h);
        assert_script_false(&emission_tree, &ctx, "overpaid LP should reject (strict ==)");
    }
}

#[cfg(test)]
mod phase6_same_block_spend {
    use super::*;

    #[test]
    fn reject_same_block_spend() {
        let h = 5u32;
        // Self box created at height 5, spending at height 5 → heightIncreased fails
        let (tree, self_box, outputs, _) = normal_spend_scenario(h, genesis_box_value(), h);
        let ctx = build_context(self_box, outputs, h);
        assert_script_false(&tree, &ctx, "same-block spend should reject (heightIncreased)");
    }

    #[test]
    fn accept_next_block_spend() {
        let h = 6u32;
        // Self box created at height 5, spending at height 6 → heightIncreased passes
        let (tree, self_box, outputs, _) = normal_spend_scenario(h, genesis_box_value(), 5);
        let ctx = build_context(self_box, outputs, h);
        assert_accepts(&tree, &ctx, "next-block spend should accept");
    }
}

#[cfg(test)]
mod phase7_wrong_successor_height {
    use super::*;

    #[test]
    fn reject_wrong_successor_creation_height() {
        let h = 10u32;
        let emission_tree = load_emission_tree();
        let treasury_tree = load_treasury_tree();
        let lp_tree = load_lp_tree();
        let t_hash = proposition_hash(&treasury_tree);
        let l_hash = proposition_hash(&lp_tree);

        let self_val = genesis_box_value();
        let self_box = make_emission_box(&emission_tree, self_val, &t_hash, &l_hash, h - 1);
        let reward = block_reward(h);
        let (m_amt, t_amt, l_amt) = split_reward(reward);

        let outputs = vec![
            make_next_emission_box(&self_box, self_val - reward, h - 1), // wrong: should be h
            make_output_box(&treasury_tree, t_amt, h),
            make_output_box(&lp_tree, l_amt, h),
            make_miner_box(m_amt, h),
        ];
        let ctx = build_context(self_box, outputs, h);
        assert_script_false(&emission_tree, &ctx, "wrong successor creation_height should reject");
    }
}

#[cfg(test)]
mod phase8_terminal_drain {
    use super::*;

    #[test]
    fn accept_terminal_path_with_nft_burned() {
        let h = 1u32;
        let emission_tree = load_emission_tree();
        let treasury_tree = load_treasury_tree();
        let lp_tree = load_lp_tree();
        let t_hash = proposition_hash(&treasury_tree);
        let l_hash = proposition_hash(&lp_tree);

        // Self value < blockReward triggers terminal path
        let remaining = 10 * NANOCOIN; // 10 coins, less than the 75-coin blockReward
        let self_box = make_emission_box(&emission_tree, remaining, &t_hash, &l_hash, 0);

        let term_treasury = remaining * TREASURY_PCT / 100; // 0.4 coin
        let term_lp = remaining * LP_PCT / 100; // 0.3 coin

        // Terminal outputs: treasury + LP + miner remainder (OUTPUTS(2), the
        // last output, bound to the miner reward script).
        let outputs = vec![
            make_output_box(&treasury_tree, term_treasury, h),
            make_output_box(&lp_tree, term_lp, h),
            make_miner_box(remaining - term_treasury - term_lp, h),
        ];
        let ctx = build_context(self_box, outputs, h);
        assert_accepts(&emission_tree, &ctx, "terminal path with NFT burned should accept");
    }
}

#[cfg(test)]
mod phase9_terminal_nft_not_burned {
    use super::*;

    #[test]
    fn reject_terminal_nft_smuggled_in_output() {
        let h = 1u32;
        let emission_tree = load_emission_tree();
        let treasury_tree = load_treasury_tree();
        let lp_tree = load_lp_tree();
        let t_hash = proposition_hash(&treasury_tree);
        let l_hash = proposition_hash(&lp_tree);

        let remaining = 10 * NANOCOIN;
        let self_box = make_emission_box(&emission_tree, remaining, &t_hash, &l_hash, 0);

        let term_treasury = remaining * TREASURY_PCT / 100;
        let term_lp = remaining * LP_PCT / 100;

        // Third output (a correct miner output otherwise) smuggles the
        // emission NFT, so only nftBurned fails
        let outputs = vec![
            make_output_box(&treasury_tree, term_treasury, h),
            make_output_box(&lp_tree, term_lp, h),
            make_output_box_with_nft(
                &reward_tree(DEFAULT_MINER_PK),
                remaining - term_treasury - term_lp,
                h,
            ),
        ];
        let ctx = build_context(self_box, outputs, h);
        assert_script_false(&emission_tree, &ctx, "terminal path with NFT not burned should reject");
    }

    #[test]
    fn reject_terminal_nft_in_treasury_output() {
        let h = 1u32;
        let emission_tree = load_emission_tree();
        let treasury_tree = load_treasury_tree();
        let lp_tree = load_lp_tree();
        let t_hash = proposition_hash(&treasury_tree);
        let l_hash = proposition_hash(&lp_tree);

        let remaining = 10 * NANOCOIN;
        let self_box = make_emission_box(&emission_tree, remaining, &t_hash, &l_hash, 0);

        let term_treasury = remaining * TREASURY_PCT / 100;
        let term_lp = remaining * LP_PCT / 100;

        // NFT smuggled into the treasury output itself
        let outputs = vec![
            make_output_box_with_nft(&treasury_tree, term_treasury, h),
            make_output_box(&lp_tree, term_lp, h),
            make_miner_box(remaining - term_treasury - term_lp, h),
        ];
        let ctx = build_context(self_box, outputs, h);
        assert_script_false(&emission_tree, &ctx, "NFT in treasury output should reject");
    }
}

// ============================================================
// 30 s RETUNE: SIXTH ARM, ARM-6 -> TAIL, END-OF-YEAR-20 EXHAUSTION
// ============================================================

/// Normal-path spend at `h` that pays `reward` (which may be wrong for `h`).
fn spend_paying(h: u32, self_val: u64, reward: u64) -> (ErgoTree, Context<'static>) {
    let emission_tree = load_emission_tree();
    let treasury_tree = load_treasury_tree();
    let lp_tree = load_lp_tree();
    let t_hash = proposition_hash(&treasury_tree);
    let l_hash = proposition_hash(&lp_tree);
    let self_box = make_emission_box(&emission_tree, self_val, &t_hash, &l_hash, h - 1);
    let (m_amt, t_amt, l_amt) = split_reward(reward);
    let outputs = vec![
        make_next_emission_box(&self_box, self_val - reward, h),
        make_output_box(&treasury_tree, t_amt, h),
        make_output_box(&lp_tree, l_amt, h),
        make_miner_box(m_amt, h),
    ];
    let ctx = build_context(self_box, outputs, h);
    (emission_tree, ctx)
}

/// Terminal-shaped spend (treasury, LP, miner remainder in OUTPUTS(2)).
fn terminal_spend(h: u32, remaining: u64) -> (ErgoTree, Context<'static>) {
    let emission_tree = load_emission_tree();
    let treasury_tree = load_treasury_tree();
    let lp_tree = load_lp_tree();
    let t_hash = proposition_hash(&treasury_tree);
    let l_hash = proposition_hash(&lp_tree);
    let self_box = make_emission_box(&emission_tree, remaining, &t_hash, &l_hash, h - 1);
    let outputs = vec![
        make_output_box(&treasury_tree, remaining * TREASURY_PCT / 100, h),
        make_output_box(&lp_tree, remaining * LP_PCT / 100, h),
        make_miner_box(remaining - remaining * TREASURY_PCT / 100 - remaining * LP_PCT / 100, h),
    ];
    let ctx = build_context(self_box, outputs, h);
    (emission_tree, ctx)
}

#[cfg(test)]
mod phase10_sixth_arm {
    use super::*;

    #[test]
    fn accept_spends_around_sixth_arm_boundary() {
        let b = 5 * BLOCKS_PER_HALVING; // 5_256_000
        assert_eq!(block_reward(b - 1), 4_687_500_000);
        assert_eq!(block_reward(b), 2_343_750_000);
        assert_eq!(block_reward(b + 1), 2_343_750_000);
        for h in [b - 1, b, b + 1] {
            let (tree, self_box, outputs, h) = normal_spend_scenario(h, real_box_value(h), h - 1);
            let ctx = build_context(self_box, outputs, h);
            assert_accepts(&tree, &ctx, &format!("spend at h={} should accept", h));
        }
    }

    #[test]
    fn reject_tail_reward_at_sixth_arm_boundary() {
        // The 20 s ".5 fix" contract dropped straight to the floor here.
        // The retuned tree must demand the 2.34375-coin arm instead.
        let b = 5 * BLOCKS_PER_HALVING;
        let (tree, ctx) = spend_paying(b, real_box_value(b), MIN_REWARD);
        assert_script_false(&tree, &ctx, "tail-floor payout at the sixth arm must reject");
    }

    #[test]
    fn reject_fifth_arm_reward_at_sixth_arm_boundary() {
        let b = 5 * BLOCKS_PER_HALVING;
        let (tree, ctx) = spend_paying(b, real_box_value(b), 4_687_500_000);
        assert_script_false(&tree, &ctx, "un-halved payout at the sixth arm must reject");
    }
}

#[cfg(test)]
mod phase11_arm6_to_tail {
    use super::*;

    #[test]
    fn accept_spends_around_tail_boundary() {
        let b = 6 * BLOCKS_PER_HALVING; // 6_307_200
        assert_eq!(block_reward(b - 1), 2_343_750_000);
        assert_eq!(block_reward(b), MIN_REWARD);
        assert_eq!(block_reward(b + 1), MIN_REWARD);
        for h in [b - 1, b, b + 1] {
            let (tree, self_box, outputs, h) = normal_spend_scenario(h, real_box_value(h), h - 1);
            let ctx = build_context(self_box, outputs, h);
            assert_accepts(&tree, &ctx, &format!("spend at h={} should accept", h));
        }
    }

    #[test]
    fn reject_sixth_arm_reward_after_tail_boundary() {
        let b = 6 * BLOCKS_PER_HALVING;
        let (tree, ctx) = spend_paying(b, real_box_value(b), 2_343_750_000);
        assert_script_false(&tree, &ctx, "sixth-arm payout in the tail must reject");
    }

    #[test]
    fn reject_seventh_halving_below_floor() {
        // A pure shift would give 75 / 64 = 1.171875 coins at epoch 6;
        // the contract floors at 1.5 instead.
        let b = 6 * BLOCKS_PER_HALVING;
        let (tree, ctx) = spend_paying(b, real_box_value(b), INITIAL_REWARD / 64);
        assert_script_false(&tree, &ctx, "sub-floor payout must reject");
    }
}

#[cfg(test)]
mod phase12_year20_exhaustion {
    use super::*;

    const END: u32 = MAX_HALVINGS * BLOCKS_PER_HALVING; // 21_024_000

    #[test]
    fn accept_last_two_spends_of_year_20_model_view() {
        // Schedule view (total supply, heights 0..END all paid): the box
        // holds 2 tail rewards before END-2 and 1 before END-1. The END-1
        // spend would leave a 0-value successor, which cannot be built as an
        // ErgoBox, so the accept is checked at END-2. Contract test: the
        // real box is spent out at 21_023_916 (see real_chain_* below).
        assert_eq!(box_value_before(END - 2), 2 * MIN_REWARD);
        let (tree, self_box, outputs, h) =
            normal_spend_scenario(END - 2, box_value_before(END - 2), END - 3);
        let ctx = build_context(self_box, outputs, h);
        assert_accepts(&tree, &ctx, "spend at END-2 should accept");
    }

    #[test]
    fn accept_tail_spends_after_year_20() {
        // Contract test: the script has no end height, so tail spends past
        // END accept. The real box never gets here (spent out at
        // 21_023_916); 75 coins at END is the schedule view plus the unmined
        // height-0 reward, the real-chain value before the 2026-11-12 split.
        let at_end = box_value_before(END) + block_reward(0);
        assert_eq!(at_end, 75 * NANOCOIN);
        let (tree, ctx) = spend_paying(END, at_end, MIN_REWARD);
        assert_accepts(&tree, &ctx, "first post-year-20 tail block should accept");
        // 48 tail blocks later the box holds 3 coins; that spend leaves 1.5.
        let h = END + 48;
        let v = at_end - 48 * MIN_REWARD;
        assert_eq!(v, 2 * MIN_REWARD);
        let (tree, ctx) = spend_paying(h, v, MIN_REWARD);
        assert_accepts(&tree, &ctx, "spend at END+48 should accept");
    }

    /// Terminal coinbase: treasury, LP, then a miner output
    /// holding the remainder in OUTPUTS(2), bound to the miner reward script
    /// of the header key. Returns the tree, the context and the three output
    /// values.
    fn terminal_with_miner(
        h: u32,
        remaining: u64,
        nft_to_miner: bool,
    ) -> (ErgoTree, Context<'static>, (u64, u64, u64)) {
        let emission_tree = load_emission_tree();
        let treasury_tree = load_treasury_tree();
        let lp_tree = load_lp_tree();
        let t_hash = proposition_hash(&treasury_tree);
        let l_hash = proposition_hash(&lp_tree);
        let self_box = make_emission_box(&emission_tree, remaining, &t_hash, &l_hash, h - 1);
        let t = remaining * TREASURY_PCT / 100;
        let l = remaining * LP_PCT / 100;
        let m = remaining - t - l;
        let miner = if nft_to_miner {
            make_output_box_with_nft(&reward_tree(DEFAULT_MINER_PK), m, h)
        } else {
            make_miner_box(m, h)
        };
        let outputs = vec![
            make_output_box(&treasury_tree, t, h),
            make_output_box(&lp_tree, l, h),
            miner,
        ];
        let ctx = build_context(self_box, outputs, h);
        (emission_tree, ctx, (t, l, m))
    }

    #[test]
    fn terminal_accepts_when_value_equals_reward() {
        // 2026-11-10 fix, contract test at a tail height. A box holding
        // exactly one reward was unspendable with `<` (terminal rejected,
        // normal path needs a 0-value successor). With `<=` the terminal
        // path accepts. Before the 2026-11-12 genesis split this was the
        // real-chain terminal block (END + 49); the real box now never holds
        // exactly one reward (real_chain_terminal_spend_at_21_023_916).
        let h = END + 49;
        let at_end = box_value_before(END) + block_reward(0);
        let v = at_end - 49 * MIN_REWARD;
        assert_eq!(v, MIN_REWARD);
        assert_eq!(block_reward(h), MIN_REWARD);

        let (tree, ctx, (t, l, m)) = terminal_with_miner(h, v, false);
        assert_accepts(&tree, &ctx, "terminal path must accept when value == reward");
        // Splits: treasury 4%, LP 3%, miner remainder, nothing left over.
        assert_eq!(t, 60_000_000);
        assert_eq!(l, 45_000_000);
        assert_eq!(m, 1_395_000_000);
        assert_eq!(t + l + m, v);
        // Equals the normal-path split of the same reward: no new extraction.
        assert_eq!((m, t, l), split_reward(MIN_REWARD));
        // Emission NFT: no output may carry it (the path burns it).
        for o in ctx.outputs.iter() {
            let carries = o
                .tokens
                .as_ref()
                .map_or(false, |ts| ts.iter().any(|tk| tk.token_id == emission_nft_id()));
            assert!(!carries, "accepted terminal tx must not carry the emission NFT");
        }
    }

    #[test]
    fn terminal_at_value_equals_reward_rejects_nft_kept_by_miner() {
        let (tree, ctx, _) = terminal_with_miner(END + 49, MIN_REWARD, true);
        assert_script_false(&tree, &ctx, "miner keeping the emission NFT must reject");
    }

    #[test]
    fn terminal_at_value_equals_reward_still_strict_on_splits() {
        let h = END + 49;
        let treasury_tree = load_treasury_tree();
        let lp_tree = load_lp_tree();
        let t_hash = proposition_hash(&treasury_tree);
        let l_hash = proposition_hash(&lp_tree);
        let (t, l, m) = (60_000_000u64, 45_000_000u64, 1_395_000_000u64);
        for (dt, dl) in [(-1i64, 0i64), (1, 0), (0, -1), (0, 1)] {
            let emission_tree = load_emission_tree();
            let self_box = make_emission_box(&emission_tree, MIN_REWARD, &t_hash, &l_hash, h - 1);
            let outputs = vec![
                make_output_box(&treasury_tree, (t as i64 + dt) as u64, h),
                make_output_box(&lp_tree, (l as i64 + dl) as u64, h),
                make_miner_box(m, h),
            ];
            let ctx = build_context(self_box, outputs, h);
            assert_script_false(&emission_tree, &ctx, &format!("split off by ({}, {}) must reject", dt, dl));
        }
    }

    #[test]
    fn terminal_rejects_one_nano_above_reward() {
        // Cannot fire early: value == reward + 1 is still a normal-path box.
        let (tree, ctx, _) = terminal_with_miner(END + 49, MIN_REWARD + 1, false);
        assert_script_false(&tree, &ctx, "terminal must reject at value == reward + 1");
        let (tree, ctx) = terminal_spend(END + 49, MIN_REWARD + 1);
        assert_script_false(&tree, &ctx, "terminal must reject at value == reward + 1");
    }

    #[test]
    fn terminal_rejects_at_twice_reward() {
        // A tail box holding 2 rewards: the normal path pays one.
        let (tree, ctx, _) = terminal_with_miner(END + 48, 2 * MIN_REWARD, false);
        assert_script_false(&tree, &ctx, "terminal must reject at value == 2 x reward");
        // Same at an arm height (epoch 0, 75-coin reward).
        let (tree, ctx, _) = terminal_with_miner(2, 2 * INITIAL_REWARD, false);
        assert_script_false(&tree, &ctx, "terminal must reject at 2 x reward in epoch 0");
    }

    #[test]
    fn terminal_at_value_equals_reward_matches_normal_split_every_level() {
        // At every reward level the terminal split of a box holding exactly
        // one reward equals the normal split, is integral, and accepts.
        for e in 0..(NUM_EXPLICIT_HALVINGS + 1) {
            let h = e * BLOCKS_PER_HALVING + 1;
            let r = block_reward(h);
            assert_eq!(r * TREASURY_PCT % 100, 0, "treasury inexact at epoch {}", e);
            assert_eq!(r * LP_PCT % 100, 0, "LP inexact at epoch {}", e);
            let (tree, ctx, (t, l, m)) = terminal_with_miner(h, r, false);
            assert_accepts(&tree, &ctx, &format!("terminal at value == reward must accept, epoch {}", e));
            assert_eq!((m, t, l), split_reward(r), "split differs from normal at epoch {}", e);
            assert_eq!(t + l + m, r);
        }
    }

    #[test]
    fn terminal_extra_emission_box_token_may_be_dropped_not_sent_to_miner() {
        // Second-order token check. Since v1.3 the normal successor must hold
        // exactly one token, so no donated token can be carried along, and
        // the miner output must hold no tokens. On the terminal path a token
        // other than the NFT on SELF (only possible from a hand-built box)
        // may be dropped (burned); it cannot go to the miner output.
        // Rewritten for v1.3: the miner-output case was an accept.
        let h = END + 49;
        let emission_tree = load_emission_tree();
        let treasury_tree = load_treasury_tree();
        let lp_tree = load_lp_tree();
        let t_hash = proposition_hash(&treasury_tree);
        let l_hash = proposition_hash(&lp_tree);
        let extra = Token {
            token_id: TokenId::from(Digest32::from([0xABu8; 32])),
            amount: TokenAmount::try_from(7u64).unwrap(),
        };
        let base = make_emission_box(&emission_tree, MIN_REWARD, &t_hash, &l_hash, h - 1);
        let mut toks = base.tokens.clone().unwrap().to_vec();
        toks.push(extra.clone());
        let candidate = ErgoBoxCandidate {
            value: base.value,
            ergo_tree: base.ergo_tree.clone(),
            tokens: Some(toks.try_into().unwrap()),
            additional_registers: base.additional_registers.clone(),
            creation_height: h - 1,
        };
        let self_box = ErgoBox::from_box_candidate(&candidate, TxId::zero(), 0).unwrap();
        let (t, l, m) = (60_000_000u64, 45_000_000u64, 1_395_000_000u64);

        // Extra token dropped.
        let outputs = vec![
            make_output_box(&treasury_tree, t, h),
            make_output_box(&lp_tree, l, h),
            make_miner_box(m, h),
        ];
        let ctx = build_context(self_box.clone(), outputs, h);
        assert_accepts(&emission_tree, &ctx, "terminal with extra token dropped accepts");

        // Extra token to the miner output.
        let miner = ErgoBoxCandidate {
            value: BoxValue::try_from(m).unwrap(),
            ergo_tree: reward_tree(DEFAULT_MINER_PK),
            tokens: Some(vec![extra].try_into().unwrap()),
            additional_registers: NonMandatoryRegisters::empty(),
            creation_height: h,
        };
        let outputs = vec![
            make_output_box(&treasury_tree, t, h),
            make_output_box(&lp_tree, l, h),
            ErgoBox::from_box_candidate(&miner, TxId::zero(), 0).unwrap(),
        ];
        let ctx = build_context(self_box, outputs, h);
        // Expected FALSE (minerOut.tokens.size == 0), not an evaluation error.
        assert_script_false(&emission_tree, &ctx, "terminal with extra token to miner rejects");
    }

    #[test]
    fn accept_terminal_after_year_20_with_sub_reward_remainder() {
        let (tree, ctx) = terminal_spend(END + 50, MIN_REWARD - 1);
        assert_accepts(&tree, &ctx, "terminal path should accept when value < reward");
    }

    #[test]
    fn real_chain_last_normal_spend_at_21_023_915() {
        // 2026-11-12 genesis: the box spent at 21_023_915 holds 2.5 coins.
        // The normal path pays the 1.5 reward and leaves a 1.0-coin
        // successor; the terminal path rejects (2.5 > 1.5).
        let h = REAL_LAST_NORMAL;
        let v = real_box_value(h);
        assert_eq!(v, 2_500_000_000);
        assert_eq!(block_reward(h), MIN_REWARD);
        let (tree, self_box, outputs, h) = normal_spend_scenario(h, v, h - 1);
        assert_eq!(*outputs[0].value.as_u64(), 1_000_000_000);
        let ctx = build_context(self_box, outputs, h);
        assert_accepts(&tree, &ctx, "real-chain last normal spend should accept");
        let (tree, ctx, _) = terminal_with_miner(h, v, false);
        assert_script_false(&tree, &ctx, "terminal must reject at 2.5 coins (> reward)");
        let (tree, ctx) = terminal_spend(h, v);
        assert_script_false(&tree, &ctx, "terminal must reject at 2.5 coins (> reward)");
    }

    #[test]
    fn real_chain_terminal_spend_at_21_023_916() {
        // 2026-11-12 genesis: the box spent at 21_023_916 holds 1.0 coin,
        // less than the 1.5 reward, so only the terminal path can spend it.
        let h = REAL_TERMINAL;
        let v = real_box_value(h);
        assert_eq!(v, 1_000_000_000);
        assert!(v < block_reward(h));
        let (tree, ctx, (t, l, m)) = terminal_with_miner(h, v, false);
        assert_accepts(&tree, &ctx, "real-chain terminal spend should accept");
        assert_eq!((t, l, m), (40_000_000, 30_000_000, 930_000_000));
        assert_eq!(t + l + m, v);
        for o in ctx.outputs.iter() {
            let carries = o
                .tokens
                .as_ref()
                .map_or(false, |ts| ts.iter().any(|tk| tk.token_id == emission_nft_id()));
            assert!(!carries, "accepted terminal tx must not carry the emission NFT");
        }
        let (tree, ctx, _) = terminal_with_miner(h, v, true);
        assert_script_false(&tree, &ctx, "miner keeping the emission NFT must reject");
        // The equality-case split (of a 1.5-coin box) must reject on the 1.0 box.
        let emission_tree = load_emission_tree();
        let treasury_tree = load_treasury_tree();
        let lp_tree = load_lp_tree();
        let t_hash = proposition_hash(&treasury_tree);
        let l_hash = proposition_hash(&lp_tree);
        let self_box = make_emission_box(&emission_tree, v, &t_hash, &l_hash, h - 1);
        let outputs = vec![
            make_output_box(&treasury_tree, 60_000_000, h),
            make_output_box(&lp_tree, 45_000_000, h),
            make_miner_box(1_395_000_000, h),
        ];
        let ctx = build_context(self_box, outputs, h);
        assert_script_false(&emission_tree, &ctx, "1.5-coin split on a 1.0-coin box must reject");
    }
}

// ============================================================
// GAP C: MINER OUTPUT BINDING (2026-11-10)
// ============================================================
//
// Every reject below is checked as script-false (the tree reduces to false
// without an evaluation error) and has an accepting twin built the same way
// with the one field restored. The one exception, a 2-output terminal tx,
// fails evaluation and is asserted as such.

#[cfg(test)]
mod phase13_miner_binding {
    use super::*;

    const END: u32 = MAX_HALVINGS * BLOCKS_PER_HALVING; // 21_024_000
    // Tail height for the value == reward terminal case (a contract case;
    // the real box is spent out at REAL_TERMINAL with 1.0 coin).
    const LAST: u32 = END + 49;
    const KEYS: [&str; 4] = [PK_A, PK_B, PK_C, PK_POOL];

    /// Canonical normal spend at `h` from a box holding `v`, miner output for
    /// `pk`: [successor, treasury, LP, miner]. Returns the miner share too.
    fn normal_parts(h: u32, v: u64, pk: &str) -> (ErgoTree, ErgoBox, Vec<ErgoBox>, u64) {
        let (tree, self_box, mut outputs, _) = normal_spend_scenario(h, v, h - 1);
        let (m, _, _) = split_reward(block_reward(h));
        outputs[3] = make_output_box(&reward_tree(pk), m, h);
        (tree, self_box, outputs, m)
    }

    /// Canonical terminal spend at `h` from a box holding `v` (<= reward):
    /// [treasury, LP, miner]. Returns the miner share too.
    fn terminal_parts(h: u32, v: u64, pk: &str) -> (ErgoTree, ErgoBox, Vec<ErgoBox>, u64) {
        let emission_tree = load_emission_tree();
        let treasury_tree = load_treasury_tree();
        let lp_tree = load_lp_tree();
        let t_hash = proposition_hash(&treasury_tree);
        let l_hash = proposition_hash(&lp_tree);
        let self_box = make_emission_box(&emission_tree, v, &t_hash, &l_hash, h - 1);
        let t = v * TREASURY_PCT / 100;
        let l = v * LP_PCT / 100;
        let m = v - t - l;
        let outputs = vec![
            make_output_box(&treasury_tree, t, h),
            make_output_box(&lp_tree, l, h),
            make_output_box(&reward_tree(pk), m, h),
        ];
        (emission_tree, self_box, outputs, m)
    }

    fn accepts(tree: &ErgoTree, s: &ErgoBox, outs: Vec<ErgoBox>, h: u32, pk: &str, msg: &str) {
        let ctx = build_context_pk(s.clone(), outs, h, pk);
        assert_accepts(tree, &ctx, msg);
    }

    fn rejects(tree: &ErgoTree, s: &ErgoBox, outs: Vec<ErgoBox>, h: u32, pk: &str, msg: &str) {
        let ctx = build_context_pk(s.clone(), outs, h, pk);
        assert_script_false(tree, &ctx, msg);
    }

    /// Normal case at h = 1000 and terminal case at value == reward (tail
    /// height LAST), each with its canonical outputs, miner share and miner
    /// index.
    fn both_paths(pk: &str) -> Vec<(&'static str, ErgoTree, ErgoBox, Vec<ErgoBox>, u32, u64, usize)> {
        let hn = 1_000u32;
        let (tn, sn, on, mn) = normal_parts(hn, real_box_value(hn), pk);
        let (tt, st, ot, mt) = terminal_parts(LAST, MIN_REWARD, pk);
        vec![("normal", tn, sn, on, hn, mn, 3), ("terminal", tt, st, ot, LAST, mt, 2)]
    }

    fn miner_box_with(tree: &ErgoTree, value: u64, h: u32) -> ErgoBox {
        make_output_box(tree, value, h)
    }

    // ---------- treasury/LP swap ----------

    #[test]
    fn reject_swapped_treasury_and_lp_scripts_normal() {
        let h = 1_000u32;
        let (tree, s, outs, _) = normal_parts(h, real_box_value(h), PK_A);
        accepts(&tree, &s, outs.clone(), h, PK_A, "normal twin accepts");
        let mut swapped = outs.clone();
        swapped[1] = make_output_box(&load_lp_tree(), outs[1].value.as_u64().clone(), h);
        swapped[2] = make_output_box(&load_treasury_tree(), outs[2].value.as_u64().clone(), h);
        rejects(&tree, &s, swapped, h, PK_A, "normal swapped treasury/LP scripts");
    }

    #[test]
    fn reject_swapped_treasury_and_lp_scripts_terminal() {
        let (tree, s, outs, _) = terminal_parts(LAST, MIN_REWARD, PK_A);
        accepts(&tree, &s, outs.clone(), LAST, PK_A, "terminal twin accepts");
        let mut swapped = outs.clone();
        swapped[0] = make_output_box(&load_lp_tree(), outs[0].value.as_u64().clone(), LAST);
        swapped[1] = make_output_box(&load_treasury_tree(), outs[1].value.as_u64().clone(), LAST);
        rejects(&tree, &s, swapped, LAST, PK_A, "terminal swapped treasury/LP scripts");
    }

    // ---------- accepts ----------

    #[test]
    fn accept_correct_miner_output_normal_all_reward_levels() {
        // Seven reward levels, three heights inside each period, header key
        // rotating through four keys (a pool is just another header key).
        let mut n = 0;
        for e in 0..7u32 {
            let base = e * BLOCKS_PER_HALVING;
            for h in [base.max(1), base + BLOCKS_PER_HALVING / 2, base + BLOCKS_PER_HALVING - 1] {
                let pk = KEYS[n % KEYS.len()];
                n += 1;
                let (tree, s, outs, m) = normal_parts(h, real_box_value(h), pk);
                assert_eq!(m, split_reward(block_reward(h)).0);
                accepts(&tree, &s, outs, h, pk, &format!("normal accept e={} h={}", e, h));
            }
        }
        // Tail heights late in the chain: schedule view and contract-only
        // heights past the real box's end, then the real-chain last normal
        // spend (2.5 coins at 21_023_915).
        for (h, v) in [
            (END - 2, 2 * MIN_REWARD),
            (END, 75 * NANOCOIN),
            (END + 48, 2 * MIN_REWARD),
            (REAL_LAST_NORMAL, real_box_value(REAL_LAST_NORMAL)),
        ] {
            let (tree, s, outs, _) = normal_parts(h, v, PK_POOL);
            accepts(&tree, &s, outs, h, PK_POOL, &format!("tail accept h={}", h));
        }
    }

    #[test]
    fn accept_correct_miner_output_terminal_all_reward_levels() {
        for e in 0..7u32 {
            let h = e * BLOCKS_PER_HALVING + 1;
            let r = block_reward(h);
            let pk = KEYS[e as usize % KEYS.len()];
            // value == reward: the terminal split equals the normal split.
            let (tree, s, outs, m) = terminal_parts(h, r, pk);
            assert_eq!(m, split_reward(r).0);
            accepts(&tree, &s, outs, h, pk, &format!("terminal value == reward e={}", e));
            // value < reward (a multiple of 31,250,000, the gcd of the
            // genesis box and the reward levels; 93,750,000 before 2026-11-12).
            let v = r - 93_750_000;
            let (tree, s, outs, _) = terminal_parts(h, v, pk);
            accepts(&tree, &s, outs, h, pk, &format!("terminal value < reward e={}", e));
        }
        // value == reward at a tail height (contract case).
        let (tree, s, outs, m) = terminal_parts(LAST, MIN_REWARD, PK_C);
        assert_eq!(m, 1_395_000_000);
        accepts(&tree, &s, outs, LAST, PK_C, "terminal at value == reward, tail height");
        // Real-chain terminal spend: 1.0 coin at 21_023_916.
        let (tree, s, outs, m) = terminal_parts(REAL_TERMINAL, real_box_value(REAL_TERMINAL), PK_C);
        assert_eq!(m, 930_000_000);
        accepts(&tree, &s, outs.clone(), REAL_TERMINAL, PK_C, "real-chain terminal at 21,023,916");
        let mut bad = outs;
        bad[2] = make_output_box(&reward_tree(PK_A), m, REAL_TERMINAL);
        rejects(&tree, &s, bad, REAL_TERMINAL, PK_C, "real-chain terminal, miner script of another key");
    }

    #[test]
    fn binding_holds_at_every_halving_boundary() {
        for e in 1..=6u32 {
            let b = e * BLOCKS_PER_HALVING;
            for h in [b - 1, b, b + 1] {
                let (tree, s, outs, m) = normal_parts(h, real_box_value(h), PK_B);
                accepts(&tree, &s, outs.clone(), h, PK_B, &format!("boundary accept h={}", h));
                let mut bad = outs.clone();
                bad[3] = make_output_box(&reward_tree(PK_C), m, h);
                rejects(&tree, &s, bad, h, PK_B, &format!("boundary wrong key h={}", h));
                // The miner share of the neighbouring level must reject.
                let other = if h < b { block_reward(b) } else { block_reward(b - 1) };
                let mut bad = outs.clone();
                bad[3] = make_output_box(&reward_tree(PK_B), split_reward(other).0, h);
                rejects(&tree, &s, bad, h, PK_B, &format!("boundary neighbour share h={}", h));
            }
        }
    }

    // ---------- script rejects ----------

    #[test]
    fn reject_wrong_pk_in_miner_script() {
        for (name, tree, s, outs, h, m, i) in both_paths(PK_A) {
            accepts(&tree, &s, outs.clone(), h, PK_A, &format!("{} twin", name));
            let mut bad = outs.clone();
            bad[i] = miner_box_with(&reward_tree(PK_B), m, h);
            rejects(&tree, &s, bad, h, PK_A, &format!("{}: script for another key", name));
        }
    }

    #[test]
    fn reject_reward_script_of_any_other_key() {
        // Every ordered pair (header key, script key) with distinct keys,
        // both paths. A tx pre-built for one header key is invalid in a block
        // whose header names another, so nobody can redirect the share.
        for hk in KEYS.iter() {
            for sk in KEYS.iter() {
                for (name, tree, s, outs, h, m, i) in both_paths(hk) {
                    let mut o = outs.clone();
                    o[i] = miner_box_with(&reward_tree(sk), m, h);
                    if hk == sk {
                        accepts(&tree, &s, o, h, hk, &format!("{} own key", name));
                    } else {
                        rejects(&tree, &s, o, h, hk, &format!("{} header {} script {}", name, hk, sk));
                    }
                }
            }
        }
    }

    #[test]
    fn reject_p2pk_of_miner_without_delay() {
        let p2pk = load_tree(&format!("0008cd{}", PK_A));
        for (name, tree, s, outs, h, m, i) in both_paths(PK_A) {
            accepts(&tree, &s, outs.clone(), h, PK_A, &format!("{} twin", name));
            let mut bad = outs.clone();
            bad[i] = miner_box_with(&p2pk, m, h);
            rejects(&tree, &s, bad, h, PK_A, &format!("{}: plain P2PK of the miner", name));
        }
    }

    #[test]
    fn reject_delay_720_instead_of_2880() {
        let t720 = load_tree(&reward_script_hex_with(MINER_SCRIPT_PREFIX_720_HEX, PK_A));
        for (name, tree, s, outs, h, m, i) in both_paths(PK_A) {
            accepts(&tree, &s, outs.clone(), h, PK_A, &format!("{} twin", name));
            let mut bad = outs.clone();
            bad[i] = miner_box_with(&t720, m, h);
            rejects(&tree, &s, bad, h, PK_A, &format!("{}: Ergo 720-block script", name));
        }
    }

    #[test]
    fn reject_other_delays() {
        // Int constant 04 <vlq(zigzag(d))>: 0, 1, 2879, 2881, and 2880 (twin).
        for d_hex in ["0400", "0402", "04fe2c", "04822d", "04802d"] {
            let prefix = format!("1002{}08cd", d_hex);
            let t = load_tree(&reward_script_hex_with(&prefix, PK_A));
            for (name, tree, s, outs, h, m, i) in both_paths(PK_A) {
                let mut o = outs.clone();
                o[i] = miner_box_with(&t, m, h);
                if d_hex == "04802d" {
                    // 2880 itself: the twin.
                    accepts(&tree, &s, o, h, PK_A, &format!("{} delay 2880", name));
                } else {
                    rejects(&tree, &s, o, h, PK_A, &format!("{} delay {}", name, d_hex));
                }
            }
        }
    }

    #[test]
    fn reject_arbitrary_contract_script() {
        // sigmaProp(HEIGHT > 0), and the treasury-hash script, as miner outputs.
        let anyone = load_tree(TREASURY_LP_TREE_HEX);
        for (name, tree, s, outs, h, m, i) in both_paths(PK_A) {
            accepts(&tree, &s, outs.clone(), h, PK_A, &format!("{} twin", name));
            let mut bad = outs.clone();
            bad[i] = miner_box_with(&anyone, m, h);
            rejects(&tree, &s, bad, h, PK_A, &format!("{}: contract as miner output", name));
        }
    }

    #[test]
    fn reject_miner_value_one_nano_high_or_low() {
        for (name, tree, s, outs, h, m, i) in both_paths(PK_A) {
            accepts(&tree, &s, outs.clone(), h, PK_A, &format!("{} twin", name));
            for v in [m - 1, m + 1] {
                let mut bad = outs.clone();
                bad[i] = make_miner_box(v, h);
                rejects(&tree, &s, bad, h, PK_A, &format!("{}: miner value {} vs {}", name, v, m));
            }
        }
    }

    #[test]
    fn reject_miner_creation_height_not_height() {
        // Creation height 0 would make the 2880-block lock open at once
        // (see reward_script_delay_counts_from_box_creation_height).
        for (name, tree, s, outs, h, m, i) in both_paths(PK_A) {
            accepts(&tree, &s, outs.clone(), h, PK_A, &format!("{} twin", name));
            for ch in [0, h - 1, h + 1, h - MINER_REWARD_DELAY.min(h)] {
                let mut bad = outs.clone();
                bad[i] = make_miner_box(m, ch);
                rejects(&tree, &s, bad, h, PK_A, &format!("{}: miner creation height {}", name, ch));
            }
        }
    }

    #[test]
    fn reject_miner_output_at_wrong_index() {
        let h = 1_000u32;
        let (tree, s, outs, _) = normal_parts(h, real_box_value(h), PK_A);
        accepts(&tree, &s, outs.clone(), h, PK_A, "normal twin");
        // [succ, t, miner, LP] and [succ, miner, t, LP]
        let o = vec![outs[0].clone(), outs[1].clone(), outs[3].clone(), outs[2].clone()];
        rejects(&tree, &s, o, h, PK_A, "normal: miner at index 2");
        let o = vec![outs[0].clone(), outs[3].clone(), outs[1].clone(), outs[2].clone()];
        rejects(&tree, &s, o, h, PK_A, "normal: miner at index 1");
        let o = vec![outs[3].clone(), outs[1].clone(), outs[2].clone(), outs[0].clone()];
        rejects(&tree, &s, o, h, PK_A, "normal: miner at index 0");

        let (tree, s, outs, _) = terminal_parts(LAST, MIN_REWARD, PK_A);
        accepts(&tree, &s, outs.clone(), LAST, PK_A, "terminal twin");
        let o = vec![outs[2].clone(), outs[0].clone(), outs[1].clone()];
        rejects(&tree, &s, o, LAST, PK_A, "terminal: miner at index 0");
        let o = vec![outs[0].clone(), outs[2].clone(), outs[1].clone()];
        rejects(&tree, &s, o, LAST, PK_A, "terminal: miner at index 1");
    }

    #[test]
    fn reject_missing_miner_output() {
        // Normal path, 3 outputs: the last output is the LP box, not a miner
        // output, so the script is false.
        let (tree, s, outs, _) = normal_parts(1_000, real_box_value(1_000), PK_A);
        accepts(&tree, &s, outs.clone(), 1_000, PK_A, "normal twin");
        rejects(&tree, &s, outs[..3].to_vec(), 1_000, PK_A, "normal: no miner output");

        // Terminal path, miner output replaced by a non-miner box: false.
        let (tree, s, outs, m) = terminal_parts(LAST, MIN_REWARD, PK_A);
        accepts(&tree, &s, outs.clone(), LAST, PK_A, "terminal twin");
        let mut o = outs.clone();
        o[2] = make_output_box(&load_treasury_tree(), m, LAST);
        rejects(&tree, &s, o, LAST, PK_A, "terminal: third output not a miner output");

        // Terminal path, 2 outputs: the tree binds OUTPUTS(2) in a top-level
        // ValDef (used by the normal path's LP check), so evaluation fails.
        // Still a reject, but by error rather than false. Unreachable with
        // one input anyway: treasury + LP is 7% of SELF.value.
        let ctx = build_context_pk(s.clone(), outs[..2].to_vec(), LAST, PK_A);
        let r = reduce(&tree, &ctx);
        assert!(r.is_err(), "2-output terminal must fail evaluation, got {:?}", r);
        assert!(!evaluate(&tree, &ctx));
    }

    #[test]
    fn reject_extra_output_siphoning_value() {
        for (name, tree, s, outs, h, m, i) in both_paths(PK_A) {
            accepts(&tree, &s, outs.clone(), h, PK_A, &format!("{} twin", name));
            let x = 100_000_000u64; // 0.1 coin
            let siphon = make_output_box(&load_tree(&format!("0008cd{}", PK_B)), x, h);
            // Siphon before the miner output, miner short by x.
            let mut o = outs[..i].to_vec();
            o.push(siphon.clone());
            o.push(make_miner_box(m - x, h));
            rejects(&tree, &s, o, h, PK_A, &format!("{}: siphon, miner short", name));
            // Siphon before the miner output, miner exact (value would have
            // to come from an extra input): output count rejects it.
            let mut o = outs[..i].to_vec();
            o.push(siphon.clone());
            o.push(make_miner_box(m, h));
            rejects(&tree, &s, o, h, PK_A, &format!("{}: siphon, miner exact", name));
            // Siphon after the miner output.
            let mut o = outs.clone();
            o.push(siphon.clone());
            rejects(&tree, &s, o, h, PK_A, &format!("{}: siphon last", name));
            // Siphon is another reward-script box of the same miner.
            let mut o = outs[..i].to_vec();
            o.push(make_miner_box(x, h));
            o.push(make_miner_box(m, h));
            rejects(&tree, &s, o, h, PK_A, &format!("{}: extra reward box", name));
        }
    }

    #[test]
    fn reject_miner_share_split_across_two_outputs() {
        for (name, tree, s, outs, h, m, i) in both_paths(PK_A) {
            accepts(&tree, &s, outs.clone(), h, PK_A, &format!("{} twin", name));
            let mut o = outs[..i].to_vec();
            o.push(make_miner_box(m / 2, h));
            o.push(make_miner_box(m - m / 2, h));
            rejects(&tree, &s, o, h, PK_A, &format!("{}: miner share split", name));
        }
    }

    #[test]
    fn miner_output_tokens_and_nft_interaction() {
        // Terminal: a correct miner output carrying the emission NFT still
        // rejects (nftBurned); the same output without it accepts.
        let (tree, s, outs, m) = terminal_parts(LAST, MIN_REWARD, PK_A);
        accepts(&tree, &s, outs.clone(), LAST, PK_A, "terminal twin");
        let mut o = outs.clone();
        o[2] = make_output_box_with_nft(&reward_tree(PK_A), m, LAST);
        rejects(&tree, &s, o, LAST, PK_A, "terminal: NFT in miner output");
        // Normal: the NFT must stay in the successor; moving it to the miner
        // output rejects (nftPreserved).
        let (tree, s, outs, m) = normal_parts(1_000, real_box_value(1_000), PK_A);
        let mut o = outs.clone();
        let succ = ErgoBoxCandidate {
            value: o[0].value,
            ergo_tree: o[0].ergo_tree.clone(),
            tokens: None,
            additional_registers: o[0].additional_registers.clone(),
            creation_height: 1_000,
        };
        o[0] = ErgoBox::from_box_candidate(&succ, TxId::zero(), 0).unwrap();
        o[3] = make_output_box_with_nft(&reward_tree(PK_A), m, 1_000);
        rejects(&tree, &s, o, 1_000, PK_A, "normal: NFT moved to miner output");
    }
}

// ============================================================
// F-009: SHARE OUTPUTS CARRY ONLY VALUE (2026-11 review)
// ============================================================
//
// Each of the four share outputs (treasury and LP, normal and terminal path)
// must have no tokens, no registers and creation height HEIGHT. Every reject
// below has an accepting twin and is asserted as script-false.

#[cfg(test)]
mod phase14_share_outputs_f009 {
    use super::*;

    // Tail height for the value == reward terminal case (contract case).
    const LAST: u32 = MAX_HALVINGS * BLOCKS_PER_HALVING + 49;

    /// Copy of `b` with tokens, registers and creation height replaced.
    fn with(b: &ErgoBox, tokens: Vec<Token>, regs: std::collections::HashMap<NonMandatoryRegisterId, Constant>, ch: u32) -> ErgoBox {
        let c = ErgoBoxCandidate {
            value: b.value,
            ergo_tree: b.ergo_tree.clone(),
            tokens: if tokens.is_empty() { None } else { Some(tokens.try_into().unwrap()) },
            additional_registers: NonMandatoryRegisters::new(regs).unwrap(),
            creation_height: ch,
        };
        ErgoBox::from_box_candidate(&c, TxId::zero(), 0).unwrap()
    }

    fn other_token() -> Token {
        Token { token_id: TokenId::from(Digest32::from([0x5Au8; 32])), amount: TokenAmount::try_from(1_000u64).unwrap() }
    }

    /// (name, tree, SELF, outputs, height, share output indices)
    fn cases() -> Vec<(&'static str, ErgoTree, ErgoBox, Vec<ErgoBox>, u32, [usize; 2])> {
        let h = 1_000u32;
        let (tn, sn, on, _) = normal_spend_scenario(h, real_box_value(h), h - 1);
        let emission_tree = load_emission_tree();
        let t_hash = proposition_hash(&load_treasury_tree());
        let l_hash = proposition_hash(&load_lp_tree());
        let st = make_emission_box(&emission_tree, MIN_REWARD, &t_hash, &l_hash, LAST - 1);
        let (m, t, l) = split_reward(MIN_REWARD);
        let ot = vec![
            make_output_box(&load_treasury_tree(), t, LAST),
            make_output_box(&load_lp_tree(), l, LAST),
            make_miner_box(m, LAST),
        ];
        vec![("normal", tn, sn, on, h, [1, 2]), ("terminal", emission_tree, st, ot, LAST, [0, 1])]
    }

    #[test]
    fn f009_twins_accept() {
        for (name, tree, s, outs, h, _) in cases() {
            let ctx = build_context(s, outs, h);
            assert_accepts(&tree, &ctx, &format!("{} twin", name));
        }
    }

    #[test]
    fn f009_reject_token_on_share_output() {
        for (name, tree, s, outs, h, idx) in cases() {
            for i in idx {
                let mut o = outs.clone();
                o[i] = with(&outs[i], vec![other_token()], Default::default(), h);
                assert_script_false(&tree, &build_context(s.clone(), o, h), &format!("{}: token on output {}", name, i));
            }
        }
    }

    #[test]
    fn f009_reject_registers_on_share_output() {
        for (name, tree, s, outs, h, idx) in cases() {
            for i in idx {
                // 3,000 bytes of padding in R4 (the rent attack of F-009).
                let mut o = outs.clone();
                let pad = std::collections::HashMap::from([(NonMandatoryRegisterId::R4, Constant::from(vec![0u8; 3_000]))]);
                o[i] = with(&outs[i], vec![], pad, h);
                assert_script_false(&tree, &build_context(s.clone(), o, h), &format!("{}: R4 padding on output {}", name, i));
                // R4 of another type. sigma-rust 0.28 does not check the declared
                // register type (ExtractRegisterAs returns the value as is), so
                // R4[Coll[Byte]] is defined and the check is FALSE here; the Scala
                // interpreter throws on the type mismatch instead. Both reject.
                let mut o = outs.clone();
                let long = std::collections::HashMap::from([(NonMandatoryRegisterId::R4, Constant::from(7i64))]);
                o[i] = with(&outs[i], vec![], long, h);
                assert_script_false(&tree, &build_context(s.clone(), o, h), &format!("{}: Long R4 on output {}", name, i));
            }
        }
    }

    #[test]
    fn f009_reject_share_creation_height_not_height() {
        for (name, tree, s, outs, h, idx) in cases() {
            for i in idx {
                for ch in [0, h - 1, h + 1] {
                    let mut o = outs.clone();
                    o[i] = with(&outs[i], vec![], Default::default(), ch);
                    assert_script_false(&tree, &build_context(s.clone(), o, h),
                        &format!("{}: output {} creation height {}", name, i, ch));
                }
            }
        }
    }
}

// ============================================================
// v1.3 (2026-11 retest): EMISSION NFT AND TOKEN PINS
// ============================================================
//
// The NFT id is a constant and SELF.tokens(0) must equal (id, 1); the miner
// output holds no tokens on both paths; the normal successor holds exactly
// one token and no R6. Every reject below has an accepting twin and is
// asserted as script-false (FALSE, not an evaluation error).

#[cfg(test)]
mod phase15_v13_token_pins {
    use super::*;

    // Tail height for the value == reward terminal case (contract case).
    const LAST: u32 = MAX_HALVINGS * BLOCKS_PER_HALVING + 49;

    fn tok(id: u8, amount: u64) -> Token {
        Token { token_id: TokenId::from(Digest32::from([id; 32])), amount: TokenAmount::try_from(amount).unwrap() }
    }

    /// Copy of `b` with tokens and registers replaced.
    fn with(b: &ErgoBox, tokens: Vec<Token>, regs: Option<std::collections::HashMap<NonMandatoryRegisterId, Constant>>) -> ErgoBox {
        let c = ErgoBoxCandidate {
            value: b.value,
            ergo_tree: b.ergo_tree.clone(),
            tokens: if tokens.is_empty() { None } else { Some(tokens.try_into().unwrap()) },
            additional_registers: match regs {
                Some(r) => NonMandatoryRegisters::new(r).unwrap(),
                None => b.additional_registers.clone(),
            },
            creation_height: b.creation_height,
        };
        ErgoBox::from_box_candidate(&c, TxId::zero(), 0).unwrap()
    }

    fn regs_of(b: &ErgoBox) -> std::collections::HashMap<NonMandatoryRegisterId, Constant> {
        let mut m = std::collections::HashMap::new();
        for id in [NonMandatoryRegisterId::R4, NonMandatoryRegisterId::R5] {
            m.insert(id, b.additional_registers.get_constant(id).unwrap().unwrap());
        }
        m
    }

    /// Honest normal spend at height 1,000 and honest terminal spend at LAST.
    fn normal() -> (ErgoTree, ErgoBox, Vec<ErgoBox>, u32) {
        normal_spend_scenario(1_000, real_box_value(1_000), 999)
    }

    fn terminal() -> (ErgoTree, ErgoBox, Vec<ErgoBox>, u32) {
        let tree = load_emission_tree();
        let t_hash = proposition_hash(&load_treasury_tree());
        let l_hash = proposition_hash(&load_lp_tree());
        let s = make_emission_box(&tree, MIN_REWARD, &t_hash, &l_hash, LAST - 1);
        let (m, t, l) = split_reward(MIN_REWARD);
        let outs = vec![
            make_output_box(&load_treasury_tree(), t, LAST),
            make_output_box(&load_lp_tree(), l, LAST),
            make_miner_box(m, LAST),
        ];
        (tree, s, outs, LAST)
    }

    #[test]
    fn v13_honest_normal_and_terminal_accept() {
        let (tree, s, o, h) = normal();
        assert_accepts(&tree, &build_context(s, o, h), "honest normal spend");
        let (tree, s, o, h) = terminal();
        assert_accepts(&tree, &build_context(s, o, h), "honest terminal spend");
    }

    /// F-022 closed: a lookalike box at this script holding another token at
    /// tokens(0), with a successor that preserves that token, is FALSE.
    #[test]
    fn v13_reject_lookalike_with_other_token_id() {
        for (name, (tree, s, o, h)) in [("normal", normal()), ("terminal", terminal())] {
            let look = with(&s, vec![tok(0x42, 1)], None);
            let mut o2 = o.clone();
            if name == "normal" {
                o2[0] = with(&o[0], vec![tok(0x42, 1)], None);
            }
            assert_script_false(&tree, &build_context(look, o2, h), &format!("{}: lookalike token id", name));
        }
    }

    /// A box holding 2 units of the emission NFT id is FALSE (amount pinned to 1).
    #[test]
    fn v13_reject_emission_box_with_two_units() {
        for (name, (tree, s, o, h)) in [("normal", normal()), ("terminal", terminal())] {
            let two = with(&s, vec![tok(0xCC, 2)], None);
            assert_script_false(&tree, &build_context(two, o, h), &format!("{}: SELF with 2 units", name));
        }
    }

    /// A token on the miner output (for example one minted with id INPUTS(0).id)
    /// is FALSE on both paths.
    #[test]
    fn v13_reject_token_on_miner_output_both_paths() {
        for (name, (tree, s, o, h)) in [("normal", normal()), ("terminal", terminal())] {
            let mut o2 = o.clone();
            let i = o.len() - 1;
            let minted = TokenId::from(Digest32::from(<[u8; 32]>::try_from(s.box_id().as_ref()).unwrap()));
            o2[i] = with(&o[i], vec![Token { token_id: minted, amount: TokenAmount::try_from(1u64).unwrap() }], None);
            assert_script_false(&tree, &build_context(s.clone(), o2, h), &format!("{}: minted token on miner output", name));
            let mut o3 = o.clone();
            o3[i] = with(&o[i], vec![tok(0x5A, 1_000)], None);
            assert_script_false(&tree, &build_context(s, o3, h), &format!("{}: other token on miner output", name));
        }
    }

    /// The normal successor must hold exactly one token and no R6.
    #[test]
    fn v13_reject_extra_token_or_r6_on_successor() {
        let (tree, s, o, h) = normal();
        let nft = s.tokens.clone().unwrap().to_vec();
        let mut extra = nft.clone();
        extra.push(tok(0x5A, 1_000));
        let mut o2 = o.clone();
        o2[0] = with(&o[0], extra, None);
        assert_script_false(&tree, &build_context(s.clone(), o2, h), "extra token on the successor");
        let mut r = regs_of(&o[0]);
        r.insert(NonMandatoryRegisterId::R6, Constant::from(vec![0u8; 2_000]));
        let mut o3 = o.clone();
        o3[0] = with(&o[0], nft.clone(), Some(r));
        assert_script_false(&tree, &build_context(s.clone(), o3, h), "R6 padding on the successor");
        // R6 of another type (Long): sigma-rust returns it as defined (FALSE
        // here); the Scala interpreter throws on the type. Both reject.
        let mut r = regs_of(&o[0]);
        r.insert(NonMandatoryRegisterId::R6, Constant::from(7i64));
        let mut o4 = o.clone();
        o4[0] = with(&o[0], nft, Some(r));
        assert_script_false(&tree, &build_context(s, o4, h), "Long R6 on the successor");
    }
}
