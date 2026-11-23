//! lp_fund_test.rs: ErgoTree evaluation tests for the LP fund contracts v2.2
//! (fund v2.2, accumulation v2.2).
//!
//! Trees: `trees/lp_fund.hex` and `trees/lp_accumulation.hex`, written by
//! `05-integration-tests/compile/gen_trees.py` from the .es sources in this crate
//! with the test NFT id bb..bb (LP_NFT_ID_PLACEHOLDER). Run
//! `python3 gen_trees.py --check` there to confirm they match a fresh compile.
//!
//! Every assertion names the exact outcome of script reduction:
//!   TRUE   the script reduced to true (accumulation script, no signature)
//!   FALSE  the script reduced to false
//!   ERROR  evaluation failed
//!   SIGNED the boolean part held and only the 2-of-3 signature remains; an
//!          accept is SIGNED plus a 2-of-3 prove/verify that succeeds.
//! The whitelist register is Coll[(Coll[Byte], Long)]: (destination hash, height added).
//! Tests use deterministic keypairs (public test keys, F-013).
//!
//! Targets: ergo-lib 0.28 with "arbitrary" feature, rustc 1.85+

use std::collections::HashMap;
use std::sync::Arc;
use blake2::{digest::consts::U32, Blake2b, Digest};
use ergo_lib::ergo_chain_types::{Digest32, Header, PreHeader};
use ergo_lib::ergotree_interpreter::{
    eval::{context::Context, reduce_to_crypto},
    sigma_protocol::{
        prover::{hint::HintsBag, ContextExtension, Prover, TestProver},
        verifier::{TestVerifier, Verifier},
        private_input::{DlogProverInput, PrivateInput},
    },
};
use ergo_lib::ergotree_ir::{
    chain::{
        ergo_box::{
            box_value::BoxValue, ErgoBox, ErgoBoxCandidate, NonMandatoryRegisterId as R,
            NonMandatoryRegisters,
        },
        token::{Token, TokenAmount, TokenId},
        tx_id::TxId,
    },
    ergo_tree::ErgoTree,
    mir::{constant::{Constant, Literal}, value::CollKind},
    serialization::SigmaSerializable,
    sigma_protocol::sigma_boolean::SigmaBoolean,
    types::{stuple::STuple, stype::SType},
};
use sigma_test_util::force_any_val;

type Blake2b256 = Blake2b<U32>;

// ============================================================
// TREES (fresh compile, see gen_trees.py)
// ============================================================

const GOVERNANCE_TREE_HEX: &str = include_str!("../trees/lp_fund.hex");
const ACCUMULATION_TREE_HEX: &str = include_str!("../trees/lp_accumulation.hex");

/// blake2b256 of the two trees above (re-pinned 2026-11-12, v2.2, test NFT id).
const GOVERNANCE_TREE_HASH: &str = "d5b9ff015760812797db3f2fcd02c837f5758645ecd3247f6d9d7a21d5add2f1";
const ACCUMULATION_TREE_HASH: &str = "f1816cc630c3dec827c5ef0c1c1263603bb0729b7752275a2e45762a55394a03";

/// ErgoTreePredef.feeProposition(720): the standard miner fee script.
const MINER_FEE_HEX: &str = "1005040004000e36100204a00b08cd0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798ea02d192a39a8cc7a701730073011001020402d19683030193a38cc7b2a57300000193c2b2a57301007473027303830108cdeeac93b1a57304";
/// A whitelisted destination (stand-in for a DEX pool): P2PK of k*G, k = 2.
const POOL_TREE_HEX: &str = "0008cd02c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5";
/// A second destination, added later in tests: P2PK of k*G, k = 1.
const POOL2_TREE_HEX: &str = "0008cd0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";
/// sigmaProp(HEIGHT > 0): an unlisted script.
const DUMMY_TREE_HEX: &str = "10010400d191a37300";
/// sigmaProp(true): a register-free migration target.
const TARGET_TREE_HEX: &str = "10010101d17300";

// ============================================================
// DETERMINISTIC SIGNER KEYS (test-only, never on mainnet)
// Same keys as treasury: contracts share the same signers.
// ============================================================

const SIGNER_SECRET_BYTES: [[u8; 32]; 3] = [
    [
        0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF,
        0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF,
        0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF,
        0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF,
    ],
    [
        0xFE, 0xDC, 0xBA, 0x98, 0x76, 0x54, 0x32, 0x10,
        0xFE, 0xDC, 0xBA, 0x98, 0x76, 0x54, 0x32, 0x10,
        0xFE, 0xDC, 0xBA, 0x98, 0x76, 0x54, 0x32, 0x10,
        0xFE, 0xDC, 0xBA, 0x98, 0x76, 0x54, 0x32, 0x10,
    ],
    [
        0xAA, 0xBB, 0xCC, 0xDD, 0x11, 0x22, 0x33, 0x44,
        0xAA, 0xBB, 0xCC, 0xDD, 0x11, 0x22, 0x33, 0x44,
        0xAA, 0xBB, 0xCC, 0xDD, 0x11, 0x22, 0x33, 0x44,
        0xAA, 0xBB, 0xCC, 0xDD, 0x11, 0x22, 0x33, 0x44,
    ],
];

const NANOCOIN: u64 = 1_000_000_000;
const TIMELOCK_BLOCKS: i64 = 8_640; // 72 h at 30 s blocks (was 12_960 at 20 s)
const WINDOW_BLOCKS: i64 = 20_160; // 7 days at 30 s blocks (F-007)
const BLOCK_TIME_SECONDS: i64 = 30;
const TX_FEE: u64 = 1_000_000; // migration value slack
const SPEND_FEE_CAP: u64 = 10_000_000; // F-017
const COOLDOWN: u32 = 20_160; // accumulation re-merge cooldown (v2.1, was 720)
const FUND_FLOOR: u64 = 100 * NANOCOIN; // fundFloor (v2.1)
const SIGN_WINDOW: u32 = 720; // successor freshness, signWindow (v2.2, was 10)
const MAX_WHITELIST: usize = 64; // whitelist entry cap (v2.1)
/// Height for accumulation merge tests: boxes created at 100 or 200 are past the cooldown.
const MH: u32 = 50_000;
/// Default height for tests: well past the delay of genesis entries (added at 0).
const H: u32 = 20_000;
/// Test NFT id compiled into both trees (LP_NFT_ID_PLACEHOLDER).
const LP_NFT: [u8; 32] = [0xBB; 32];

// ============================================================
// HELPERS
// ============================================================

fn hex_to_bytes(hex: &str) -> Vec<u8> {
    let hex = hex.trim();
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect()
}

fn load_tree(hex: &str) -> ErgoTree {
    ErgoTree::sigma_parse_bytes(&hex_to_bytes(hex)).expect("valid ErgoTree")
}

fn load_governance_tree() -> ErgoTree { load_tree(GOVERNANCE_TREE_HEX) }
fn load_accumulation_tree() -> ErgoTree { load_tree(ACCUMULATION_TREE_HEX) }
fn fee_tree() -> ErgoTree { load_tree(MINER_FEE_HEX) }
fn target_tree() -> ErgoTree { load_tree(TARGET_TREE_HEX) }

fn blake(bytes: &[u8]) -> Vec<u8> {
    let mut h = Blake2b256::new();
    h.update(bytes);
    h.finalize().to_vec()
}

fn proposition_hash(tree: &ErgoTree) -> Vec<u8> { blake(&tree.sigma_serialize_bytes().expect("serialize tree")) }

fn hex_of(b: &[u8]) -> String { b.iter().map(|x| format!("{:02x}", x)).collect() }

fn signer_keys() -> Vec<DlogProverInput> {
    SIGNER_SECRET_BYTES
        .iter()
        .map(|b| DlogProverInput::from_bytes(b).expect("valid scalar"))
        .collect()
}

fn prover_2_of_3() -> TestProver {
    let keys = signer_keys();
    TestProver {
        secrets: vec![
            PrivateInput::DlogProverInput(keys[0].clone()),
            PrivateInput::DlogProverInput(keys[1].clone()),
        ],
    }
}

fn prover_1_of_3() -> TestProver {
    let keys = signer_keys();
    TestProver { secrets: vec![PrivateInput::DlogProverInput(keys[0].clone())] }
}

/// Context with several inputs; SELF = inputs[self_idx].
fn build_context_multi(inputs: Vec<ErgoBox>, self_idx: usize, outputs: Vec<ErgoBox>, height: u32) -> Context<'static> {
    let ins: Vec<&'static ErgoBox> = inputs.into_iter().map(|b| &*Box::leak(Box::new(b))).collect();
    let self_ref = ins[self_idx];
    Context {
        height,
        self_box: self_ref,
        outputs: Vec::leak(outputs),
        data_inputs: None,
        inputs: ins.try_into().unwrap(),
        pre_header: force_any_val::<PreHeader>(),
        headers: force_any_val::<[Header; 10]>(),
        extension: ContextExtension::empty(),
    }
}

fn build_context(self_box: ErgoBox, outputs: Vec<ErgoBox>, height: u32) -> Context<'static> {
    build_context_multi(vec![self_box], 0, outputs, height)
}

fn evaluate(tree: &ErgoTree, ctx: &Context, prover: &TestProver) -> bool {
    let message = vec![0u8; 32];
    let proof = match prover.prove(tree, ctx, message.as_slice(), &HintsBag::empty()) {
        Ok(p) => p.proof,
        Err(_) => return false,
    };
    match TestVerifier.verify(tree, ctx, proof, message.as_slice()) {
        Ok(v) => v.result,
        Err(_) => false,
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Outcome { True, False, Signed, Error }

fn outcome(tree: &ErgoTree, ctx: &Context) -> Outcome {
    let expr = tree.proposition().expect("tree parses");
    match reduce_to_crypto(&expr, ctx) {
        Ok(r) => match r.sigma_prop {
            SigmaBoolean::TrivialProp(true) => Outcome::True,
            SigmaBoolean::TrivialProp(false) => Outcome::False,
            _ => Outcome::Signed,
        },
        Err(_) => Outcome::Error,
    }
}

fn assert_signed(tree: &ErgoTree, ctx: &Context, msg: &str) {
    assert_eq!(outcome(tree, ctx), Outcome::Signed, "{}", msg);
    assert!(evaluate(tree, ctx, &prover_2_of_3()), "{} (2-of-3 prove/verify)", msg);
    assert!(!evaluate(tree, ctx, &prover_1_of_3()), "{} (1-of-3 must not prove)", msg);
}

fn assert_true(tree: &ErgoTree, ctx: &Context, msg: &str) {
    assert_eq!(outcome(tree, ctx), Outcome::True, "{}", msg);
    assert!(evaluate(tree, ctx, &TestProver { secrets: vec![] }), "{} (prove/verify)", msg);
}

fn assert_false(tree: &ErgoTree, ctx: &Context, msg: &str) {
    assert_eq!(outcome(tree, ctx), Outcome::False, "{}", msg);
    assert!(!evaluate(tree, ctx, &prover_2_of_3()), "{} (must not prove)", msg);
}

fn assert_error(tree: &ErgoTree, ctx: &Context, msg: &str) {
    assert_eq!(outcome(tree, ctx), Outcome::Error, "{}", msg);
    assert!(!evaluate(tree, ctx, &prover_2_of_3()), "{} (must not prove)", msg);
}

// ============================================================
// BOX CONSTRUCTION
// ============================================================

type Whitelist = Vec<(Vec<u8>, i64)>;

/// Coll[(Coll[Byte], Long)] constant for R4.
fn whitelist_const(wl: &[(Vec<u8>, i64)]) -> Constant {
    let pair = SType::STuple(STuple::pair(SType::SColl(Arc::new(SType::SByte)), SType::SLong));
    let items: Vec<Literal> = wl
        .iter()
        .map(|(h, a)| {
            let c1: Constant = h.clone().into();
            let c2: Constant = (*a).into();
            Literal::Tup(vec![c1.v, c2.v].try_into().unwrap())
        })
        .collect();
    Constant {
        tpe: SType::SColl(Arc::new(pair.clone())),
        v: Literal::Coll(CollKind::WrappedColl { elem_tpe: pair, items: items.into() }),
    }
}

fn token(id: [u8; 32], amount: u64) -> Token {
    Token { token_id: TokenId::from(Digest32::from(id)), amount: TokenAmount::try_from(amount).unwrap() }
}

fn make_nft_token() -> Token { token(LP_NFT, 1) }

fn make_box(tree: &ErgoTree, value: u64, tokens: Vec<Token>, regs: HashMap<R, Constant>, creation_height: u32) -> ErgoBox {
    ErgoBox::from_box_candidate(
        &ErgoBoxCandidate {
            value: BoxValue::try_from(value).unwrap(),
            ergo_tree: tree.clone(),
            tokens: if tokens.is_empty() { None } else { Some(tokens.try_into().unwrap()) },
            additional_registers: NonMandatoryRegisters::new(regs).unwrap(),
            creation_height,
        },
        TxId::zero(), 0,
    ).unwrap()
}

fn make_output_box(tree: &ErgoTree, value: u64, creation_height: u32) -> ErgoBox {
    make_box(tree, value, vec![], HashMap::new(), creation_height)
}

fn fee_box(value: u64, h: u32) -> ErgoBox { make_output_box(&fee_tree(), value, h) }

fn lp_regs(whitelist: &Whitelist, state_flag: i64, migration_height: i64, target: Vec<u8>) -> HashMap<R, Constant> {
    HashMap::from([
        (R::R4, whitelist_const(whitelist)),
        (R::R5, Constant::from(state_flag)),
        (R::R6, Constant::from(migration_height)),
        (R::R7, Constant::from(target)),
    ])
}

/// LP fund box: NFT, R4 whitelist, R5 flag, R6 migration height, R7 migration target.
fn make_lp_box(tree: &ErgoTree, value: u64, whitelist: &Whitelist, state_flag: i64, migration_height: i64, target: Vec<u8>, creation_height: u32) -> ErgoBox {
    make_box(tree, value, vec![make_nft_token()], lp_regs(whitelist, state_flag, migration_height, target), creation_height)
}

/// Genesis R7 (v2.1): 32 zero bytes, which reserves the room a migration
/// approval needs. Inert: migration execute needs R5 = 2.
fn genesis_r7() -> Vec<u8> { vec![0u8; 32] }

/// Shorthand with the genesis R7 (no migration ever approved; was empty before v2.1).
fn make_governance_box(tree: &ErgoTree, value: u64, whitelist: Whitelist, state_flag: i64, migration_height: i64, creation_height: u32) -> ErgoBox {
    make_lp_box(tree, value, &whitelist, state_flag, migration_height, genesis_r7(), creation_height)
}

/// Same registers at another script (for migration outputs).
fn make_governance_box_other_script(tree: &ErgoTree, value: u64, whitelist: Whitelist, state_flag: i64, migration_height: i64, creation_height: u32) -> ErgoBox {
    make_governance_box(tree, value, whitelist, state_flag, migration_height, creation_height)
}

fn make_idle_governance_box(tree: &ErgoTree, value: u64, whitelist: Whitelist, creation_height: u32) -> ErgoBox {
    make_governance_box(tree, value, whitelist, 0i64, 0i64, creation_height)
}

/// Migration-approved box for `target` at height `a`.
fn make_migration_box(tree: &ErgoTree, value: u64, wl: &Whitelist, target: &ErgoTree, a: i64, ch: u32) -> ErgoBox {
    make_lp_box(tree, value, wl, 2, a, proposition_hash(target), ch)
}

/// Whitelist with one genesis entry: the pool script, added at 0.
fn sample_whitelist() -> Whitelist { vec![(proposition_hash(&whitelisted_tree()), 0)] }

/// Whitelist with two entries: the pool and a dummy entry added at 0.
fn sample_whitelist_2() -> Whitelist {
    let mut wl = sample_whitelist();
    wl.push((vec![0xFFu8; 32], 0));
    wl
}

fn whitelisted_tree() -> ErgoTree { load_tree(POOL_TREE_HEX) }

fn accum_box(value: u64, ch: u32) -> ErgoBox { make_output_box(&load_accumulation_tree(), value, ch) }


// ============================================================
// TESTS
// ============================================================

#[cfg(test)]
mod phase0_load {
    use super::*;

    #[test]
    fn governance_ergotree_loads_and_has_valid_size() {
        let tree = load_governance_tree();
        let size = tree.sigma_serialize_bytes().unwrap().len();
        println!("LP fund governance v2.2 ErgoTree size: {} bytes", size);
        assert!(size > 100, "ErgoTree too small");
    }

    #[test]
    fn accumulation_ergotree_loads_and_has_valid_size() {
        let tree = load_accumulation_tree();
        let size = tree.sigma_serialize_bytes().unwrap().len();
        assert!(size > 10, "ErgoTree too small");
    }

    #[test]
    fn governance_round_trip_serialization() {
        let tree = load_governance_tree();
        let bytes = tree.sigma_serialize_bytes().unwrap();
        assert_eq!(bytes, hex_to_bytes(GOVERNANCE_TREE_HEX), "governance round-trip mismatch");
    }

    #[test]
    fn accumulation_round_trip_serialization() {
        let tree = load_accumulation_tree();
        let bytes = tree.sigma_serialize_bytes().unwrap();
        assert_eq!(bytes, hex_to_bytes(ACCUMULATION_TREE_HEX), "accumulation round-trip mismatch");
    }

    #[test]
    fn governance_proposition_parses() {
        load_governance_tree().proposition().expect("should parse");
    }

    #[test]
    fn accumulation_proposition_parses() {
        load_accumulation_tree().proposition().expect("should parse");
    }

    #[test]
    fn tree_hashes_are_pinned() {
        assert_eq!(hex_of(&blake(&hex_to_bytes(GOVERNANCE_TREE_HEX))), GOVERNANCE_TREE_HASH);
        assert_eq!(hex_of(&blake(&hex_to_bytes(ACCUMULATION_TREE_HEX))), ACCUMULATION_TREE_HASH);
    }

    /// The whitelist register round-trips through box serialization.
    #[test]
    fn whitelist_register_round_trips() {
        let b = make_governance_box(&load_governance_tree(), NANOCOIN, sample_whitelist_2(), 0, 0, 1);
        let bytes = b.sigma_serialize_bytes().unwrap();
        let back = ErgoBox::sigma_parse_bytes(&bytes).unwrap();
        assert_eq!(back.additional_registers, b.additional_registers);
    }
}

#[cfg(test)]
mod phase1_threshold {
    use super::*;

    #[test]
    fn accept_2_of_3_threshold() {
        let tree = load_governance_tree();
        let wl = sample_whitelist();
        let value = 200 * NANOCOIN; // change stays above the floor (v2.1)

        let self_box = make_idle_governance_box(&tree, value, wl.clone(), H - 1);
        let out0 = make_governance_box(&tree, value - NANOCOIN, wl.clone(), 0, 0, H);
        let out1 = make_output_box(&whitelisted_tree(), NANOCOIN, H);
        let ctx = build_context(self_box, vec![out0, out1], H);
        assert_signed(&tree, &ctx, "2-of-3 threshold should accept");
    }

    #[test]
    fn reject_1_of_3_threshold() {
        let tree = load_governance_tree();
        let wl = sample_whitelist();
        let value = 200 * NANOCOIN; // change stays above the floor (v2.1)

        let self_box = make_idle_governance_box(&tree, value, wl.clone(), H - 1);
        let out0 = make_governance_box(&tree, value - NANOCOIN, wl.clone(), 0, 0, H);
        let out1 = make_output_box(&whitelisted_tree(), NANOCOIN, H);
        let ctx = build_context(self_box, vec![out0, out1], H);
        assert!(!evaluate(&tree, &ctx, &prover_1_of_3()), "1-of-3 should reject");
    }
}

#[cfg(test)]
mod phase2_spend {
    use super::*;

    /// SELF holds 200 YOLO so the change stays above the 100 YOLO floor (v2.1; was 100).
    fn spend(wl: Whitelist, h: u32, dest: &ErgoTree, extra: Vec<ErgoBox>) -> (ErgoTree, Context<'static>) {
        let tree = load_governance_tree();
        let value = 200 * NANOCOIN;
        let spend_amount = 10 * NANOCOIN;
        let self_box = make_idle_governance_box(&tree, value, wl.clone(), h - 1);
        let mut outs = vec![
            make_governance_box(&tree, value - spend_amount, wl, 0, 0, h),
            make_output_box(dest, spend_amount, h),
        ];
        outs.extend(extra);
        let ctx = build_context(self_box, outs, h);
        (tree, ctx)
    }

    #[test]
    fn accept_spend_to_whitelisted_destination() {
        let (tree, ctx) = spend(sample_whitelist(), H, &whitelisted_tree(), vec![]);
        assert_signed(&tree, &ctx, "whitelisted spend should accept");
    }

    #[test]
    fn reject_spend_to_non_whitelisted_destination() {
        let (tree, ctx) = spend(sample_whitelist(), H, &load_tree(DUMMY_TREE_HEX), vec![]);
        assert_false(&tree, &ctx, "non-whitelisted spend should reject");
    }

    #[test]
    fn reject_spend_when_frozen() {
        let tree = load_governance_tree();
        let wl = sample_whitelist();
        let value = 200 * NANOCOIN; // change stays above the floor (v2.1)
        let spend_amount = 10 * NANOCOIN;

        let self_box = make_governance_box(&tree, value, wl.clone(), 1, 0, H - 1); // frozen
        let out0 = make_governance_box(&tree, value - spend_amount, wl.clone(), 1, 0, H);
        let out1 = make_output_box(&whitelisted_tree(), spend_amount, H);
        let ctx = build_context(self_box, vec![out0, out1], H);
        assert_false(&tree, &ctx, "spend when frozen should reject");
    }

    #[test]
    fn reject_spend_whitelist_not_preserved() {
        let tree = load_governance_tree();
        let wl = sample_whitelist();
        let wl2 = sample_whitelist_2(); // different whitelist in output
        let value = 200 * NANOCOIN; // change stays above the floor (v2.1)
        let spend_amount = 10 * NANOCOIN;

        let self_box = make_idle_governance_box(&tree, value, wl.clone(), H - 1);
        let out0 = make_governance_box(&tree, value - spend_amount, wl2, 0, 0, H);
        let out1 = make_output_box(&whitelisted_tree(), spend_amount, H);
        let ctx = build_context(self_box, vec![out0, out1], H);
        assert_false(&tree, &ctx, "whitelist change during spend should reject");
    }

    #[test]
    fn reject_spend_flag_not_preserved() {
        let tree = load_governance_tree();
        let wl = sample_whitelist();
        let value = 200 * NANOCOIN; // change stays above the floor (v2.1)
        let spend_amount = 10 * NANOCOIN;

        let self_box = make_idle_governance_box(&tree, value, wl.clone(), H - 1);
        // Change state flag to 2 during spend (sneaky migration bypass)
        let out0 = make_governance_box(&tree, value - spend_amount, wl.clone(), 2, 0, H);
        let out1 = make_output_box(&whitelisted_tree(), spend_amount, H);
        let ctx = build_context(self_box, vec![out0, out1], H);
        assert_false(&tree, &ctx, "flag change during spend should reject");
    }

    #[test]
    fn reject_spend_migration_height_not_preserved() {
        let tree = load_governance_tree();
        let wl = sample_whitelist();
        let value = 200 * NANOCOIN; // change stays above the floor (v2.1)
        let spend_amount = 10 * NANOCOIN;

        let self_box = make_idle_governance_box(&tree, value, wl.clone(), H - 1);
        // Change migration height during spend (audit fix test)
        let out0 = make_governance_box(&tree, value - spend_amount, wl.clone(), 0, 999, H);
        let out1 = make_output_box(&whitelisted_tree(), spend_amount, H);
        let ctx = build_context(self_box, vec![out0, out1], H);
        assert_false(&tree, &ctx, "migration height change during spend should reject");
    }

    #[test]
    fn reject_spend_script_not_preserved() {
        let tree = load_governance_tree();
        let wl = sample_whitelist();
        let value = 200 * NANOCOIN; // change stays above the floor (v2.1)
        let spend_amount = 10 * NANOCOIN;

        let self_box = make_idle_governance_box(&tree, value, wl.clone(), H - 1);
        // Change box goes to the accumulation tree instead of the fund script
        let out0 = make_governance_box_other_script(&load_accumulation_tree(), value - spend_amount, wl.clone(), 0, 0, H);
        let out1 = make_output_box(&whitelisted_tree(), spend_amount, H);
        let ctx = build_context(self_box, vec![out0, out1], H);
        assert_false(&tree, &ctx, "script change in change box should reject");
    }

    /// F-017: one output at the miner fee script with value <= 10,000,000 is allowed.
    #[test]
    fn f017_spend_with_fee_output() {
        let (tree, ctx) = spend(sample_whitelist(), H, &whitelisted_tree(), vec![fee_box(SPEND_FEE_CAP, H)]);
        assert_signed(&tree, &ctx, "fee at the cap");
        let (tree, ctx) = spend(sample_whitelist(), H, &whitelisted_tree(), vec![fee_box(SPEND_FEE_CAP + 1, H)]);
        assert_false(&tree, &ctx, "fee one nano above the cap");
        let (tree, ctx) = spend(sample_whitelist(), H, &whitelisted_tree(), vec![fee_box(TX_FEE, H), fee_box(TX_FEE, H)]);
        assert_false(&tree, &ctx, "two fee outputs");
        // A fee output alone (no destination) is also accepted.
        let tree = load_governance_tree();
        let s = make_idle_governance_box(&tree, 200 * NANOCOIN, sample_whitelist(), H - 1);
        let o = vec![make_governance_box(&tree, 200 * NANOCOIN - TX_FEE, sample_whitelist(), 0, 0, H), fee_box(TX_FEE, H)];
        assert_signed(&tree, &build_context(s, o, H), "fee only");
    }

    /// F-005: a genesis entry (added at 0) is usable only after block 8,640.
    #[test]
    fn f005_genesis_entry_waits_the_delay() {
        let first = TIMELOCK_BLOCKS as u32 + 1;
        let (tree, ctx) = spend(sample_whitelist(), first, &whitelisted_tree(), vec![]);
        assert_signed(&tree, &ctx, "block 8,641");
        let (tree, ctx) = spend(sample_whitelist(), first - 1, &whitelisted_tree(), vec![]);
        assert_false(&tree, &ctx, "block 8,640");
    }
}

#[cfg(test)]
mod phase3_whitelist_update {
    use super::*;

    fn update(old: Whitelist, new: Whitelist, h: u32) -> (ErgoTree, Context<'static>) {
        let tree = load_governance_tree();
        let value = 100 * NANOCOIN;
        let self_box = make_idle_governance_box(&tree, value, old, h - 1);
        let out0 = make_governance_box(&tree, value, new, 0, 0, h);
        let ctx = build_context(self_box, vec![out0], h);
        (tree, ctx)
    }

    fn with_new(h_added: i64) -> Whitelist {
        let mut wl = sample_whitelist();
        wl.push((vec![0xFFu8; 32], h_added));
        wl
    }

    #[test]
    fn accept_whitelist_add_entry() {
        let (tree, ctx) = update(sample_whitelist(), with_new(H as i64), H);
        assert_signed(&tree, &ctx, "whitelist addition should accept (one output, F-024)");
    }

    #[test]
    fn reject_whitelist_remove_entry() {
        let (tree, ctx) = update(sample_whitelist_2(), sample_whitelist(), H);
        assert_false(&tree, &ctx, "whitelist removal should reject");
    }

    #[test]
    fn reject_whitelist_replace_entry() {
        let replaced = vec![(vec![0xAAu8; 32], H as i64), (vec![0xCCu8; 32], H as i64)];
        let (tree, ctx) = update(sample_whitelist(), replaced, H);
        assert_false(&tree, &ctx, "whitelist replacement should reject");
    }

    #[test]
    fn reject_whitelist_update_value_stolen() {
        let tree = load_governance_tree();
        let wl = sample_whitelist();
        let value = 100 * NANOCOIN;

        let self_box = make_idle_governance_box(&tree, value, wl.clone(), H - 1);
        let out0 = make_governance_box(&tree, value - NANOCOIN, with_new(H as i64), 0, 0, H);
        let dummy = make_output_box(&tree, NANOCOIN, H);
        let ctx = build_context(self_box, vec![out0, dummy], H);
        assert_false(&tree, &ctx, "value decrease during whitelist update should reject");
    }

    #[test]
    fn reject_whitelist_update_when_frozen() {
        let tree = load_governance_tree();
        let wl = sample_whitelist();
        let value = 100 * NANOCOIN;

        let self_box = make_governance_box(&tree, value, wl.clone(), 1, 0, H - 1); // frozen
        let out0 = make_governance_box(&tree, value, with_new(H as i64), 1, 0, H);
        let ctx = build_context(self_box, vec![out0], H);
        assert_false(&tree, &ctx, "whitelist update when frozen should reject");
    }

    /// F-005 / F-016: new entries carry a height in [HEIGHT - 10, HEIGHT], > 0.
    #[test]
    fn f005_new_entry_height_is_forced() {
        let (tree, ctx) = update(sample_whitelist(), with_new(H as i64 - 10), H);
        assert_signed(&tree, &ctx, "h-10");
        for (what, a) in [("h-11", H as i64 - 11), ("h+1", H as i64 + 1), ("0", 0), ("old", 1)] {
            let (tree, ctx) = update(sample_whitelist(), with_new(a), H);
            assert_false(&tree, &ctx, what);
        }
        let (tree, ctx) = update(sample_whitelist(), with_new(0), 5);
        assert_false(&tree, &ctx, "height 0 at a low HEIGHT");
        // Old entries cannot be re-stamped and new hashes must be 32 bytes.
        let mut restamped = with_new(H as i64);
        restamped[0].1 = -100_000;
        let (tree, ctx) = update(sample_whitelist(), restamped, H);
        assert_false(&tree, &ctx, "existing entry height changed");
        let mut short = sample_whitelist();
        short.push((vec![0xFFu8; 31], H as i64));
        let (tree, ctx) = update(sample_whitelist(), short, H);
        assert_false(&tree, &ctx, "31-byte hash");
    }

    /// F-005: an entry added now cannot be paid in the same block, only after the delay.
    #[test]
    fn f005_added_entry_paid_only_after_delay() {
        let tree = load_governance_tree();
        let dest = load_tree(POOL2_TREE_HEX);
        let old = sample_whitelist();
        let mut new = old.clone();
        new.push((proposition_hash(&dest), H as i64));
        // Step 1: add it.
        let (t, ctx) = update(old.clone(), new.clone(), H);
        assert_signed(&t, &ctx, "add entry at H");
        // Step 2: spend everything above the fund floor to it at once (v2.1: the
        // fund keeps 100 YOLO; this case used to leave it at 1,000,000 nano).
        let pay = |h: u32| {
            let s = make_idle_governance_box(&tree, 600 * NANOCOIN, new.clone(), H);
            let outs = vec![make_governance_box(&tree, FUND_FLOOR, new.clone(), 0, 0, h), make_output_box(&dest, 500 * NANOCOIN, h)];
            build_context(s, outs, h)
        };
        assert_false(&tree, &pay(H), "same block");
        assert_false(&tree, &pay(H + TIMELOCK_BLOCKS as u32), "at H + 8,640");
        assert_signed(&tree, &pay(H + TIMELOCK_BLOCKS as u32 + 1), "at H + 8,641");
        // The long-listed entry is still payable at once after the addition.
        let s = make_idle_governance_box(&tree, 500 * NANOCOIN, new.clone(), H);
        let outs = vec![make_governance_box(&tree, 400 * NANOCOIN, new.clone(), 0, 0, H), make_output_box(&whitelisted_tree(), 100 * NANOCOIN, H)];
        assert_signed(&tree, &build_context(s, outs, H), "old entry right after the addition");
    }
}

#[cfg(test)]
mod phase4_consolidation {
    use super::*;

    #[test]
    fn accept_consolidation_value_increases() {
        let tree = load_governance_tree();
        let wl = sample_whitelist();
        let value = 50 * NANOCOIN;

        let self_box = make_idle_governance_box(&tree, value, wl.clone(), H - 1);
        let out0 = make_governance_box(&tree, value + 10 * NANOCOIN, wl.clone(), 0, 0, H);
        let ctx = build_context(self_box, vec![out0], H);
        assert_signed(&tree, &ctx, "consolidation should accept (one output, F-024)");
    }

    #[test]
    fn reject_consolidation_value_decreases() {
        let tree = load_governance_tree();
        let wl = sample_whitelist();
        let value = 50 * NANOCOIN;

        let self_box = make_idle_governance_box(&tree, value, wl.clone(), H - 1);
        let out0 = make_governance_box(&tree, value - NANOCOIN, wl.clone(), 0, 0, H);
        // The value can still leave through the spend path only to listed destinations.
        let dummy = make_output_box(&load_tree(DUMMY_TREE_HEX), NANOCOIN, H);
        let ctx = build_context(self_box, vec![out0, dummy], H);
        assert_false(&tree, &ctx, "value decrease in consolidation should reject");
    }

    #[test]
    fn reject_consolidation_when_frozen() {
        let tree = load_governance_tree();
        let wl = sample_whitelist();
        let value = 50 * NANOCOIN;

        let self_box = make_governance_box(&tree, value, wl.clone(), 1, 0, H - 1); // frozen
        let out0 = make_governance_box(&tree, value + 10 * NANOCOIN, wl.clone(), 1, 0, H);
        let ctx = build_context(self_box, vec![out0], H);
        assert_false(&tree, &ctx, "consolidation when frozen should reject");
    }

    #[test]
    fn reject_consolidation_whitelist_changed() {
        let tree = load_governance_tree();
        let wl = sample_whitelist();
        // Same-size but different whitelist (not an append, so the update path fails too)
        let wl_different = vec![(vec![0xDDu8; 32], 0)];
        let value = 50 * NANOCOIN;

        let self_box = make_idle_governance_box(&tree, value, wl.clone(), H - 1);
        let out0 = make_governance_box(&tree, value + 10 * NANOCOIN, wl_different, 0, 0, H);
        let ctx = build_context(self_box, vec![out0], H);
        assert_false(&tree, &ctx, "whitelist change during consolidation should reject");
    }

    #[test]
    fn reject_consolidation_migration_height_changed() {
        let tree = load_governance_tree();
        let wl = sample_whitelist();
        let value = 50 * NANOCOIN;

        let self_box = make_idle_governance_box(&tree, value, wl.clone(), H - 1);
        // Attempt to change migration height during consolidation (audit fix test)
        let out0 = make_governance_box(&tree, value + 10 * NANOCOIN, wl.clone(), 0, 500, H);
        let ctx = build_context(self_box, vec![out0], H);
        assert_false(&tree, &ctx, "migration height change during consolidation should reject");
    }

    /// F-006: the migration target (R7) is preserved by consolidation.
    #[test]
    fn f006_consolidation_preserves_migration_target() {
        let tree = load_governance_tree();
        let wl = sample_whitelist();
        let s = make_migration_box(&tree, 50 * NANOCOIN, &wl, &target_tree(), 100, H - 1);
        let ok = make_migration_box(&tree, 60 * NANOCOIN, &wl, &target_tree(), 100, H);
        assert_signed(&tree, &build_context(s.clone(), vec![ok], H), "target kept");
        let other = make_migration_box(&tree, 60 * NANOCOIN, &wl, &load_tree(DUMMY_TREE_HEX), 100, H);
        assert_false(&tree, &build_context(s, vec![other], H), "target rewritten");
    }
}

#[cfg(test)]
mod phase5_freeze {
    use super::*;

    #[test]
    fn accept_freeze() {
        let tree = load_governance_tree();
        let wl = sample_whitelist();
        let value = 100 * NANOCOIN;

        let self_box = make_idle_governance_box(&tree, value, wl.clone(), H - 1);
        let out0 = make_governance_box(&tree, value, wl.clone(), 1, 0, H);
        let ctx = build_context(self_box, vec![out0], H);
        assert_signed(&tree, &ctx, "freeze should accept (one output, F-024)");
    }

    #[test]
    fn reject_freeze_already_frozen() {
        let tree = load_governance_tree();
        let wl = sample_whitelist();
        let value = 100 * NANOCOIN;

        let self_box = make_governance_box(&tree, value, wl.clone(), 1, 0, H - 1);
        let out0 = make_governance_box(&tree, value, wl.clone(), 1, 0, H);
        let ctx = build_context(self_box, vec![out0], H);
        assert_false(&tree, &ctx, "already frozen should reject");
    }

    /// F-020 (header said R6 is kept on every non-migration path; freeze did not check
    /// it): freeze now keeps R6 and R7.
    #[test]
    fn f020_freeze_preserves_r6_and_r7() {
        let tree = load_governance_tree();
        let wl = sample_whitelist();
        let s = make_migration_box(&tree, 100 * NANOCOIN, &wl, &target_tree(), 100, H - 1);
        let ok = make_lp_box(&tree, 100 * NANOCOIN, &wl, 1, 100, proposition_hash(&target_tree()), H);
        assert_signed(&tree, &build_context(s.clone(), vec![ok], H), "freeze from state 2");
        let r6 = make_lp_box(&tree, 100 * NANOCOIN, &wl, 1, 0, proposition_hash(&target_tree()), H);
        assert_false(&tree, &build_context(s.clone(), vec![r6], H), "freeze resetting R6");
        let r7 = make_lp_box(&tree, 100 * NANOCOIN, &wl, 1, 100, vec![], H);
        assert_false(&tree, &build_context(s, vec![r7], H), "freeze resetting R7");
    }

    /// F-012: unfreeze returns the flag to 0 and keeps everything else.
    #[test]
    fn f012_unfreeze_path() {
        let tree = load_governance_tree();
        let wl = sample_whitelist();
        let frozen = make_lp_box(&tree, 100 * NANOCOIN, &wl, 1, 100, proposition_hash(&target_tree()), H - 1);
        let ok = make_lp_box(&tree, 100 * NANOCOIN, &wl, 0, 100, proposition_hash(&target_tree()), H);
        assert_signed(&tree, &build_context(frozen.clone(), vec![ok], H), "unfreeze");
        let to2 = make_lp_box(&tree, 100 * NANOCOIN, &wl, 2, 100, proposition_hash(&target_tree()), H);
        assert_false(&tree, &build_context(frozen.clone(), vec![to2], H), "unfreeze into state 2");
        let wl2 = make_lp_box(&tree, 100 * NANOCOIN, &sample_whitelist_2(), 0, 100, proposition_hash(&target_tree()), H);
        assert_false(&tree, &build_context(frozen.clone(), vec![wl2], H), "unfreeze changing the whitelist");
        let less = make_lp_box(&tree, 99 * NANOCOIN, &wl, 0, 100, proposition_hash(&target_tree()), H);
        assert_false(&tree, &build_context(frozen, vec![less], H), "unfreeze losing value");
    }
}

#[cfg(test)]
mod phase6_migration {
    use super::*;

    #[test]
    fn accept_migration_approval() {
        let tree = load_governance_tree();
        let wl = sample_whitelist();
        let value = 100 * NANOCOIN;

        let self_box = make_idle_governance_box(&tree, value, wl.clone(), H - 1);
        // Output: R5=2 (migration flag), R6=HEIGHT, R7 = target hash
        let out0 = make_migration_box(&tree, value, &wl, &target_tree(), H as i64, H);
        let ctx = build_context(self_box, vec![out0], H);
        assert_signed(&tree, &ctx, "migration approval should accept");
    }

    #[test]
    fn reject_migration_approval_when_frozen() {
        let tree = load_governance_tree();
        let wl = sample_whitelist();
        let value = 100 * NANOCOIN;

        let self_box = make_governance_box(&tree, value, wl.clone(), 1, 0, H - 1);
        let out0 = make_migration_box(&tree, value, &wl, &target_tree(), H as i64, H);
        let ctx = build_context(self_box, vec![out0], H);
        assert_false(&tree, &ctx, "migration approval when frozen should reject");
    }

    #[test]
    fn reject_migration_approval_not_normal_state() {
        let tree = load_governance_tree();
        let wl = sample_whitelist();
        let value = 100 * NANOCOIN;

        // Already in migration state (R5=2)
        let self_box = make_migration_box(&tree, value, &wl, &target_tree(), 50, H - 1);
        let out0 = make_migration_box(&tree, value, &wl, &target_tree(), H as i64, H);
        let ctx = build_context(self_box, vec![out0], H);
        assert_false(&tree, &ctx, "migration approval from non-normal state should reject");
    }

    #[test]
    fn accept_migration_execute_after_timelock() {
        let tree = load_governance_tree();
        // v2.1: the example target is no longer the accumulation script, which
        // would hand the NFT to anyone (R4); a separate test covers that case.
        let new_contract = target_tree();
        let approval_h = 100i64;
        let exec_h = (approval_h + TIMELOCK_BLOCKS + 1) as u32;
        let wl = sample_whitelist();
        let value = 100 * NANOCOIN;

        let self_box = make_migration_box(&tree, value, &wl, &new_contract, approval_h, exec_h - 1);
        // Target with typed registers of the fund layout, R5 = 0 and R6 = 0: accepted
        // (v2.1 reads only R5 and R6 of a target at another script).
        let out0 = make_governance_box_other_script(&new_contract, value - TX_FEE, wl.clone(), 0, 0, exec_h);
        let ctx = build_context(self_box, vec![out0, fee_box(TX_FEE, exec_h)], exec_h);
        assert_signed(&tree, &ctx, "migration execute should accept");
    }

    #[test]
    fn reject_migration_execute_before_timelock() {
        let tree = load_governance_tree();
        let approval_h = 100i64;
        let exec_h = (approval_h + TIMELOCK_BLOCKS) as u32; // exact boundary, not past
        let wl = sample_whitelist();
        let value = 100 * NANOCOIN;

        let self_box = make_migration_box(&tree, value, &wl, &target_tree(), approval_h, exec_h - 1);
        let out0 = make_box(&target_tree(), value - TX_FEE, vec![make_nft_token()], HashMap::new(), exec_h);
        let ctx = build_context(self_box, vec![out0], exec_h);
        assert_false(&tree, &ctx, "migration before timelock should reject");
    }

    #[test]
    fn reject_migration_execute_without_flag() {
        let tree = load_governance_tree();
        let exec_h = 20000u32;
        let wl = sample_whitelist();
        let value = 100 * NANOCOIN;

        // R5=0, not migration approved (R6, R7 set anyway)
        let self_box = make_lp_box(&tree, value, &wl, 0, 100, proposition_hash(&target_tree()), exec_h - 1);
        let out0 = make_box(&target_tree(), value - TX_FEE, vec![make_nft_token()], HashMap::new(), exec_h);
        let ctx = build_context(self_box, vec![out0], exec_h);
        assert_false(&tree, &ctx, "migration without flag should reject");
    }

    #[test]
    fn full_migration_flow() {
        let tree = load_governance_tree();
        let wl = sample_whitelist();
        let value = 100 * NANOCOIN;

        // Step 1: Migration approval (R5: 0 -> 2, R6 = HEIGHT - 2, R7 = target hash)
        let h1 = 100u32;
        let self1 = make_idle_governance_box(&tree, value, wl.clone(), h1 - 1);
        let out1 = make_migration_box(&tree, value, &wl, &target_tree(), h1 as i64 - 2, h1);
        let ctx1 = build_context(self1, vec![out1.clone()], h1);
        assert_signed(&tree, &ctx1, "migration approval step should accept");

        // Step 2: Migration execute after timelock, register-free target, one output
        let h2 = (h1 as i64 - 2 + TIMELOCK_BLOCKS + 1) as u32;
        let out2 = make_box(&target_tree(), value, vec![make_nft_token()], HashMap::new(), h2);
        let ctx2 = build_context(out1, vec![out2], h2);
        assert_signed(&tree, &ctx2, "migration execute step should accept");
    }

    /// F-016: migration approval height in [HEIGHT - 10, HEIGHT], > 0.
    #[test]
    fn f016_migration_approval_height_range() {
        let tree = load_governance_tree();
        let wl = sample_whitelist();
        let s = make_idle_governance_box(&tree, 100 * NANOCOIN, wl.clone(), H - 1);
        let at = |a: i64, h: u32| build_context(s.clone(), vec![make_migration_box(&tree, 100 * NANOCOIN, &wl, &target_tree(), a, h)], h);
        assert_signed(&tree, &at(H as i64 - 10, H), "h-10");
        assert_false(&tree, &at(H as i64 - 11, H), "h-11");
        assert_false(&tree, &at(H as i64 + 1, H), "h+1");
        assert_false(&tree, &at(0, 5), "0 at a low HEIGHT");
        // Target hash must be 32 bytes.
        let short = make_lp_box(&tree, 100 * NANOCOIN, &wl, 2, H as i64, vec![1u8; 31], H);
        assert_false(&tree, &build_context(s.clone(), vec![short], H), "31-byte target");
    }

    /// F-006: execute checks the recorded target hash and a value floor.
    #[test]
    fn f006_migration_execute_checks_target_and_floor() {
        let tree = load_governance_tree();
        let wl = sample_whitelist();
        let a = 100i64;
        let h = (a + TIMELOCK_BLOCKS + 1) as u32;
        let s = make_migration_box(&tree, 100 * NANOCOIN, &wl, &target_tree(), a, h - 1);
        let wrong = make_box(&load_tree(DUMMY_TREE_HEX), 100 * NANOCOIN, vec![make_nft_token()], HashMap::new(), h);
        assert_false(&tree, &build_context(s.clone(), vec![wrong], h), "unreviewed target");
        let thin = make_box(&target_tree(), 100 * NANOCOIN - TX_FEE - 1, vec![make_nft_token()], HashMap::new(), h);
        let rest = make_output_box(&load_tree(DUMMY_TREE_HEX), TX_FEE + 1, h);
        assert_false(&tree, &build_context(s.clone(), vec![thin, rest], h), "below the floor");
        let floor = make_box(&target_tree(), 100 * NANOCOIN - TX_FEE, vec![make_nft_token()], HashMap::new(), h);
        assert_signed(&tree, &build_context(s, vec![floor, fee_box(TX_FEE, h)], h), "at the floor");
    }

    /// F-006: a migrate-to-self arrives neutral with the whitelist unchanged.
    #[test]
    fn f006_migrate_to_self_arrives_neutral() {
        let tree = load_governance_tree();
        let wl = sample_whitelist();
        let a = 100i64;
        let h = (a + TIMELOCK_BLOCKS + 1) as u32;
        let s = make_migration_box(&tree, 100 * NANOCOIN, &wl, &tree, a, h - 1);
        let neutral = make_lp_box(&tree, 100 * NANOCOIN, &wl, 0, 0, proposition_hash(&tree), h);
        assert_signed(&tree, &build_context(s.clone(), vec![neutral], h), "neutral");
        let armed = make_lp_box(&tree, 100 * NANOCOIN, &wl, 2, 1, proposition_hash(&load_tree(DUMMY_TREE_HEX)), h);
        assert_false(&tree, &build_context(s.clone(), vec![armed], h), "armed for another target");
        // A new whitelist of old (instantly usable) entries cannot ride in on a migrate-to-self.
        let mut big = wl.clone();
        big.push((proposition_hash(&load_tree(DUMMY_TREE_HEX)), 0));
        let smuggled = make_lp_box(&tree, 100 * NANOCOIN, &big, 0, 0, proposition_hash(&tree), h);
        assert_false(&tree, &build_context(s.clone(), vec![smuggled], h), "whitelist with an old entry");
        let rearmed = make_lp_box(&tree, 100 * NANOCOIN, &wl, 2, 1, proposition_hash(&tree), h);
        assert_false(&tree, &build_context(s.clone(), vec![rearmed], h), "re-armed with R6 = 1");
        let half = make_lp_box(&tree, 100 * NANOCOIN, &wl, 2, 0, proposition_hash(&tree), h);
        assert_false(&tree, &build_context(s, vec![half], h), "R5 = 2 with R6 = 0");
    }

    /// F-007: migration execute must happen inside the window; the LP fund can cancel.
    #[test]
    fn f007_window_and_cancel() {
        let tree = load_governance_tree();
        let wl = sample_whitelist();
        let a = 100i64;
        let last = (a + TIMELOCK_BLOCKS + WINDOW_BLOCKS) as u32;
        let s = make_migration_box(&tree, 100 * NANOCOIN, &wl, &target_tree(), a, 200);
        let out = |h: u32| make_box(&target_tree(), 100 * NANOCOIN, vec![make_nft_token()], HashMap::new(), h);
        assert_signed(&tree, &build_context(s.clone(), vec![out(last)], last), "last block");
        assert_false(&tree, &build_context(s.clone(), vec![out(last + 1)], last + 1), "after the window");
        // Cancel: R5 -> 0, R6 -> 0, whitelist, R7, value, NFT kept.
        let cancel = make_lp_box(&tree, 100 * NANOCOIN, &wl, 0, 0, proposition_hash(&target_tree()), last + 1);
        assert_signed(&tree, &build_context(s.clone(), vec![cancel.clone()], last + 1), "cancel after expiry");
        let half = make_lp_box(&tree, 100 * NANOCOIN, &wl, 0, a, proposition_hash(&target_tree()), last + 1);
        assert_false(&tree, &build_context(s.clone(), vec![half], last + 1), "cancel keeping R6");
        let early = make_lp_box(&tree, 100 * NANOCOIN, &wl, 0, 0, proposition_hash(&target_tree()), 300);
        assert_signed(&tree, &build_context(s.clone(), vec![early], 300), "cancel before the delay ends");
        // After the cancel, a new approval is possible.
        let again = make_migration_box(&tree, 100 * NANOCOIN, &wl, &target_tree(), (last + 2) as i64, last + 2);
        assert_signed(&tree, &build_context(cancel, vec![again], last + 2), "re-approve");
        // Cancel requires state 2. Rewritten (v2.1): the old case rewrote R7, which
        // cancel forbids in any state. Here a state-0 box with a stale R6 = 100 and
        // R7 = t (as after an unfreeze from state 2) moves to R5 0, R6 0, R7 t:
        // only the state check of cancel rejects it (FALSE).
        let stale = make_lp_box(&tree, 100 * NANOCOIN, &wl, 0, 100, proposition_hash(&target_tree()), 300);
        let reset = make_lp_box(&tree, 100 * NANOCOIN, &wl, 0, 0, proposition_hash(&target_tree()), 301);
        assert_false(&tree, &build_context(stale, vec![reset], 301), "cancel from state 0");
    }

    /// F-007 / F-004 analogue: R5 = 2 with R6 = 0 cannot migrate.
    #[test]
    fn f007_migration_execute_needs_positive_height() {
        let tree = load_governance_tree();
        let wl = sample_whitelist();
        let s = make_lp_box(&tree, 100 * NANOCOIN, &wl, 2, 0, proposition_hash(&target_tree()), 100);
        let out0 = make_box(&target_tree(), 100 * NANOCOIN, vec![make_nft_token()], HashMap::new(), 10_000);
        assert_false(&tree, &build_context(s, vec![out0], 10_000), "R6 = 0");
    }

    /// F-024: migration to a register-free or differently typed target, one output.
    /// v2.1 reads R5 and R6 of the target as Long (getOrElse(0)). Rewritten for
    /// v2.1: a target whose R5 is not a Long is now rejected (was accepted).
    #[test]
    fn f024_migration_to_register_free_or_differently_typed_target() {
        let tree = load_governance_tree();
        let wl = sample_whitelist();
        let a = 100i64;
        let h = (a + TIMELOCK_BLOCKS + 1) as u32;
        let s = make_migration_box(&tree, 100 * NANOCOIN, &wl, &target_tree(), a, h - 1);
        let bare = make_box(&target_tree(), 100 * NANOCOIN, vec![make_nft_token()], HashMap::new(), h);
        assert_signed(&tree, &build_context(s.clone(), vec![bare], h), "register-free target");
        // R4 of another type is not read: accepted.
        let odd_r4 = make_box(&target_tree(), 100 * NANOCOIN, vec![make_nft_token()],
            HashMap::from([(R::R4, Constant::from(vec![9u8; 4]))]), h);
        assert_signed(&tree, &build_context(s.clone(), vec![odd_r4], h), "R4 of another type");
        // R5 of another type (Coll[Byte]) is read as Long. sigma-rust 0.28 does not
        // check the declared type and the comparison with 0L is FALSE here; the
        // Scala interpreter throws on the type mismatch. Both reject.
        let odd = make_box(&target_tree(), 100 * NANOCOIN, vec![make_nft_token()],
            HashMap::from([(R::R4, Constant::from(vec![9u8; 4])), (R::R5, Constant::from(vec![1u8]))]), h);
        assert_false(&tree, &build_context(s, vec![odd], h), "R5 of another type");
    }
}

#[cfg(test)]
mod phase7_accumulation {
    use super::*;

    #[test]
    fn accept_accum_consolidation() {
        let tree = load_accumulation_tree();
        let a = accum_box(10 * NANOCOIN, 100);
        let b = accum_box(5 * NANOCOIN, 200);
        let out0 = make_output_box(&tree, 15 * NANOCOIN, MH);
        for i in 0..2 {
            let ctx = build_context_multi(vec![a.clone(), b.clone()], i, vec![out0.clone()], MH);
            assert_true(&tree, &ctx, "accum consolidation should accept");
        }
    }

    /// Rewritten (F-001, F-003): honest forward into the NFT holder.
    #[test]
    fn accept_accum_transfer_to_governance() {
        let accum_tree = load_accumulation_tree();
        let gov_tree = load_governance_tree();
        let wl = sample_whitelist();
        let gov = make_idle_governance_box(&gov_tree, 50 * NANOCOIN, wl.clone(), H - 1);
        let acc = accum_box(10 * NANOCOIN, H - 1);
        let succ = make_idle_governance_box(&gov_tree, 60 * NANOCOIN, wl, H);
        let ins = vec![gov, acc];
        assert_true(&accum_tree, &build_context_multi(ins.clone(), 1, vec![succ.clone()], H), "forward into the NFT holder");
        assert_signed(&gov_tree, &build_context_multi(ins, 0, vec![succ], H), "fund consolidation of the forward");
        let old = build_context(accum_box(10 * NANOCOIN, H - 1), vec![make_output_box(&gov_tree, 10 * NANOCOIN, H)], H);
        assert_false(&accum_tree, &old, "tokenless box at the LP fund script");
    }

    #[test]
    fn reject_accum_wrong_destination() {
        let accum_tree = load_accumulation_tree();
        let wrong_tree = load_tree(DUMMY_TREE_HEX);
        let self_box = accum_box(10 * NANOCOIN, 100);
        let out0 = make_output_box(&wrong_tree, 10 * NANOCOIN, MH);
        let ctx = build_context(self_box, vec![out0], MH);
        assert_false(&accum_tree, &ctx, "wrong destination should reject");
    }

    #[test]
    fn reject_accum_value_decrease() {
        let tree = load_accumulation_tree();
        let self_box = accum_box(10 * NANOCOIN, 100);
        let out0 = make_output_box(&tree, 10 * NANOCOIN - 1, MH);
        let ctx = build_context(self_box, vec![out0], MH);
        assert_false(&tree, &ctx, "value decrease should reject");
    }

    /// F-001: dust forward, and the NFT holder must receive old value + sum.
    #[test]
    fn f001_reject_dust_forward() {
        let tree = load_accumulation_tree();
        let gov = load_governance_tree();
        let a = accum_box(100 * NANOCOIN, 100);
        let b = accum_box(100 * NANOCOIN, 100);
        let outs = vec![make_output_box(&gov, TX_FEE, H), make_output_box(&load_tree(DUMMY_TREE_HEX), 200 * NANOCOIN - TX_FEE, H)];
        for i in 0..2 {
            assert_false(&tree, &build_context_multi(vec![a.clone(), b.clone()], i, outs.clone(), H), "dust forward");
        }
        let g = make_idle_governance_box(&gov, 50 * NANOCOIN, sample_whitelist(), 100);
        let short = make_idle_governance_box(&gov, 250 * NANOCOIN - 1, sample_whitelist(), H);
        assert_false(&tree, &build_context_multi(vec![g.clone(), a.clone(), b.clone()], 1, vec![short], H), "one nano short");
        let exact = make_idle_governance_box(&gov, 250 * NANOCOIN, sample_whitelist(), H);
        assert_true(&tree, &build_context_multi(vec![g, a, b], 2, vec![exact], H), "old value + sum");
    }

    /// F-001: INPUTS(0) must hold the NFT, and so must O0 (each checked alone).
    #[test]
    fn f001_reject_transfer_without_nft_holder() {
        let tree = load_accumulation_tree();
        let gov = load_governance_tree();
        let wl = sample_whitelist();
        let acc = accum_box(10 * NANOCOIN, 100);
        let g = make_idle_governance_box(&gov, 50 * NANOCOIN, wl.clone(), 100);
        let succ = make_idle_governance_box(&gov, 60 * NANOCOIN, wl.clone(), H);
        // NFT holder is not INPUTS(0); O0 still holds the NFT.
        assert_false(&tree, &build_context_multi(vec![acc.clone(), g.clone()], 0, vec![succ], H), "NFT holder at INPUTS(1)");
        // O0 lacks the NFT (it went to OUTPUTS(1)); INPUTS(0) holds it.
        let o0 = make_box(&gov, 60 * NANOCOIN, vec![], lp_regs(&wl, 0, 0, vec![]), H);
        let o1 = make_box(&gov, NANOCOIN, vec![make_nft_token()], lp_regs(&wl, 0, 0, vec![]), H);
        assert_false(&tree, &build_context_multi(vec![g, acc], 1, vec![o0, o1], H), "O0 without the NFT");
    }

    /// F-001: the treasury NFT does not count; only the LP NFT does (separate constant).
    #[test]
    fn f001_lp_script_ignores_treasury_nft() {
        let tree = load_accumulation_tree();
        let other = load_tree(DUMMY_TREE_HEX);
        let tnft = token([0xAA; 32], 1);
        let holder = make_box(&other, 50 * NANOCOIN, vec![tnft.clone()], HashMap::new(), 100);
        let succ = make_box(&other, 60 * NANOCOIN, vec![tnft], HashMap::new(), H);
        let ctx = build_context_multi(vec![holder, accum_box(10 * NANOCOIN, 100)], 1, vec![succ], H);
        assert_false(&tree, &ctx, "treasury NFT holder");
    }

    /// F-002: merge returning the largest input.
    #[test]
    fn f002_reject_merge_returning_largest_input() {
        let tree = load_accumulation_tree();
        let ins = vec![accum_box(100 * NANOCOIN, 100), accum_box(60 * NANOCOIN, 100), accum_box(40 * NANOCOIN, 100)];
        let outs = vec![make_output_box(&tree, 100 * NANOCOIN, MH), make_output_box(&load_tree(DUMMY_TREE_HEX), 100 * NANOCOIN, MH)];
        for i in 0..3 {
            assert_false(&tree, &build_context_multi(ins.clone(), i, outs.clone(), MH), "output = largest input");
        }
        let honest = vec![make_output_box(&tree, 200 * NANOCOIN, MH)];
        for i in 0..3 {
            assert_true(&tree, &build_context_multi(ins.clone(), i, honest.clone(), MH), "honest merge");
        }
    }

    /// F-003: tokenless box at the LP fund script: not a forward target, not spendable.
    #[test]
    fn f003_tokenless_fund_box() {
        let tree = load_accumulation_tree();
        let gov = load_governance_tree();
        let wl = sample_whitelist();
        assert_false(&tree, &build_context(accum_box(10 * NANOCOIN, 100), vec![make_output_box(&gov, 10 * NANOCOIN, H)], H), "forward into a tokenless box");
        let regs_only = make_box(&gov, 10 * NANOCOIN, vec![], lp_regs(&wl, 0, 0, vec![]), H - 1);
        let succ = make_box(&gov, 10 * NANOCOIN, vec![], lp_regs(&wl, 0, 0, vec![]), H);
        assert_false(&gov, &build_context(regs_only, vec![succ.clone()], H), "tokenless box with typed registers");
        let bare = make_output_box(&gov, 10 * NANOCOIN, H - 1);
        assert_error(&gov, &build_context(bare, vec![succ], H), "tokenless box without registers");
    }

    /// F-019: co-spent accumulation value must reach the fund successor.
    #[test]
    fn f019_reject_co_spend_skim() {
        let la = load_accumulation_tree();
        let lf = load_governance_tree();
        let wl = sample_whitelist();
        let gb = make_idle_governance_box(&lf, 500 * NANOCOIN, wl.clone(), 100);
        let ins = vec![gb, accum_box(100 * NANOCOIN, 100)];
        let outs = vec![make_idle_governance_box(&lf, 500 * NANOCOIN, wl.clone(), H), make_output_box(&load_tree(DUMMY_TREE_HEX), 100 * NANOCOIN, H)];
        assert_false(&la, &build_context_multi(ins.clone(), 1, outs, H), "accumulation side refuses");
        // Skim through the spend path to a listed destination is refused as well.
        let outs = vec![make_idle_governance_box(&lf, 500 * NANOCOIN, wl.clone(), H), make_output_box(&whitelisted_tree(), 100 * NANOCOIN, H)];
        assert_signed(&lf, &build_context_multi(ins.clone(), 0, outs.clone(), H), "fund side is a valid spend");
        assert_false(&la, &build_context_multi(ins.clone(), 1, outs, H), "but the accumulation box refuses");
        let honest = vec![make_idle_governance_box(&lf, 600 * NANOCOIN, wl, H)];
        assert_true(&la, &build_context_multi(ins, 1, honest, H), "successor carries the sum");
    }

    /// F-023 (LP shape): a lookalike box at the fund script with another token.
    #[test]
    fn f023_lookalike_fund_box() {
        let la = load_accumulation_tree();
        let lf = load_governance_tree();
        let fake = token([42u8; 32], 1);
        let mut wl = sample_whitelist();
        wl.push((proposition_hash(&load_tree(DUMMY_TREE_HEX)), 0));
        let look = make_box(&lf, 200 * NANOCOIN, vec![fake.clone()], lp_regs(&wl, 0, 0, vec![]), 100);
        let ctx = build_context_multi(vec![accum_box(200 * NANOCOIN, 100)], 0, vec![look.clone()], H);
        assert_false(&la, &ctx, "forward into the lookalike");
        let outs = vec![make_box(&lf, TX_FEE, vec![fake], lp_regs(&wl, 0, 0, vec![]), H), make_output_box(&load_tree(DUMMY_TREE_HEX), 200 * NANOCOIN - TX_FEE, H)];
        assert_false(&lf, &build_context(look, outs, H), "spend from the lookalike");
    }

    /// F-014: SELF must hold the constant LP NFT id, amount 1, at tokens(0).
    #[test]
    fn f014_self_must_hold_constant_nft() {
        let lf = load_governance_tree();
        let wl = sample_whitelist();
        for (what, toks) in [("other id", vec![token([0x42; 32], 1)]), ("amount 2", vec![token(LP_NFT, 2)]),
                             ("not at index 0", vec![token([0x42; 32], 5), make_nft_token()])] {
            let s = make_box(&lf, 10 * NANOCOIN, toks, lp_regs(&wl, 0, 0, vec![]), H - 1);
            let o = make_box(&lf, 10 * NANOCOIN, vec![make_nft_token()], lp_regs(&wl, 0, 0, vec![]), H);
            assert_false(&lf, &build_context(s, vec![o], H), what);
        }
        // Lookalike SELF next to the real fund box, O0 holding the real NFT.
        let look = make_box(&lf, 10 * NANOCOIN, vec![token([0x42; 32], 1)], lp_regs(&wl, 0, 0, vec![]), H - 1);
        let real = make_idle_governance_box(&lf, 10 * NANOCOIN, wl.clone(), H - 1);
        let o = make_idle_governance_box(&lf, 20 * NANOCOIN, wl.clone(), H);
        assert_false(&lf, &build_context_multi(vec![look, real.clone()], 0, vec![o.clone()], H), "lookalike next to the real box");
        let two = make_box(&lf, 10 * NANOCOIN, vec![token(LP_NFT, 2)], lp_regs(&wl, 0, 0, vec![]), H - 1);
        assert_false(&lf, &build_context_multi(vec![two, real], 0, vec![o], H), "SELF with 2 units, O0 with 1");
    }

    /// F-008: the stream follows the NFT to its new holder.
    #[test]
    fn f008_forward_follows_nft_after_migration() {
        let la = load_accumulation_tree();
        let new_home = load_tree(DUMMY_TREE_HEX);
        let holder = make_box(&new_home, 80 * NANOCOIN, vec![make_nft_token()], HashMap::new(), 100);
        let succ = make_box(&new_home, 90 * NANOCOIN, vec![make_nft_token()], HashMap::new(), H);
        assert_true(&la, &build_context_multi(vec![holder, accum_box(10 * NANOCOIN, 100)], 1, vec![succ], H), "migrated holder");
    }

    /// F-025: re-merge cooldown (20,160 blocks since v2.1, was 720) and successor
    /// creation height. Merge tests run at MH, past the cooldown of boxes at 100.
    #[test]
    fn f025_consolidation_cooldown() {
        let tree = load_accumulation_tree();
        let out = |ch: u32| vec![make_output_box(&tree, 15 * NANOCOIN, ch)];
        let old = accum_box(10 * NANOCOIN, MH - COOLDOWN - 1);
        let young = accum_box(10 * NANOCOIN, MH - COOLDOWN);
        let other = accum_box(5 * NANOCOIN, 100);
        assert_true(&tree, &build_context_multi(vec![old.clone(), other.clone()], 0, out(MH), MH), "20,161 blocks old");
        assert_false(&tree, &build_context_multi(vec![young, other.clone()], 0, out(MH), MH), "20,160 blocks old");
        assert_false(&tree, &build_context_multi(vec![old.clone(), other.clone()], 0, out(MH - 11), MH), "creation h-11");
        assert_true(&tree, &build_context_multi(vec![old, other], 0, out(MH - 10), MH), "creation h-10");
    }

    /// Merged boxes carry no tokens and no registers.
    #[test]
    fn consolidation_output_is_plain() {
        let tree = load_accumulation_tree();
        let a = accum_box(10 * NANOCOIN, 100);
        let padded = make_box(&tree, 10 * NANOCOIN, vec![], HashMap::from([(R::R4, Constant::from(vec![0u8; 100]))]), MH);
        assert_false(&tree, &build_context(a.clone(), vec![padded], MH), "registers");
        let tok = make_box(&tree, 10 * NANOCOIN, vec![token([0x55; 32], 9)], HashMap::new(), MH);
        assert_false(&tree, &build_context(a, vec![tok], MH), "token");
    }
}

#[cfg(test)]
mod phase8_audit_fixes {
    use super::*;

    /// Audit finding: R6 must be preserved during consolidation to prevent timelock bypass.
    #[test]
    fn reject_consolidation_r6_tamper_during_migration() {
        let tree = load_governance_tree();
        let wl = sample_whitelist();
        let value = 50 * NANOCOIN;
        let approval_h = 100i64;

        // Box is in migration-approved state (R5=2, R6=100)
        let self_box = make_migration_box(&tree, value, &wl, &target_tree(), approval_h, H - 1);
        // Consolidation attempt that resets R6 to 0 while keeping R5 = 2
        let out0 = make_lp_box(&tree, value + 10 * NANOCOIN, &wl, 2, 0, proposition_hash(&target_tree()), H);
        let ctx = build_context(self_box, vec![out0], H);
        assert_false(&tree, &ctx, "R6 tamper during migration should reject (v1.1 fix)");
    }

    /// Verify consolidation still works during migration-approved state when R6 is preserved.
    #[test]
    fn accept_consolidation_during_migration_r6_preserved() {
        let tree = load_governance_tree();
        let wl = sample_whitelist();
        let value = 50 * NANOCOIN;
        let approval_h = 100i64;

        let self_box = make_migration_box(&tree, value, &wl, &target_tree(), approval_h, H - 1);
        let out0 = make_migration_box(&tree, value + 10 * NANOCOIN, &wl, &target_tree(), approval_h, H);
        let ctx = build_context(self_box, vec![out0], H);
        assert_signed(&tree, &ctx, "consolidation during migration with R6 preserved should accept");
    }

    /// F-024: every self-successor path accepts a one-output transaction.
    #[test]
    fn f024_one_output_on_every_path() {
        let tree = load_governance_tree();
        let wl = sample_whitelist();
        let t = proposition_hash(&target_tree());
        let mut wl_new = wl.clone();
        wl_new.push((vec![0x33; 32], H as i64));
        let cases: Vec<(&str, ErgoBox, ErgoBox)> = vec![
            ("spend (change only)", make_lp_box(&tree, 10 * NANOCOIN, &wl, 0, 0, vec![], H - 1), make_lp_box(&tree, 10 * NANOCOIN, &wl, 0, 0, vec![], H)),
            ("whitelist update", make_lp_box(&tree, 10 * NANOCOIN, &wl, 0, 0, vec![], H - 1), make_lp_box(&tree, 10 * NANOCOIN, &wl_new, 0, 0, vec![], H)),
            ("freeze", make_lp_box(&tree, 10 * NANOCOIN, &wl, 0, 0, vec![], H - 1), make_lp_box(&tree, 10 * NANOCOIN, &wl, 1, 0, vec![], H)),
            ("unfreeze", make_lp_box(&tree, 10 * NANOCOIN, &wl, 1, 0, vec![], H - 1), make_lp_box(&tree, 10 * NANOCOIN, &wl, 0, 0, vec![], H)),
            ("migration approval", make_lp_box(&tree, 10 * NANOCOIN, &wl, 0, 0, vec![], H - 1), make_lp_box(&tree, 10 * NANOCOIN, &wl, 2, H as i64, t.clone(), H)),
            ("migration cancel", make_lp_box(&tree, 10 * NANOCOIN, &wl, 2, 100, t.clone(), H - 1), make_lp_box(&tree, 10 * NANOCOIN, &wl, 0, 0, t.clone(), H)),
        ];
        for (what, s, o) in cases {
            assert_signed(&tree, &build_context(s, vec![o], H), what);
        }
    }
}

// ============================================================
// v2.1 (2026-11 retest): FUND FLOOR, FRESH SUCCESSOR, NEUTRAL ARRIVAL,
// WHITELIST CAP, ACCUMULATION HOLDER PIN
// ============================================================
//
// Every reject below is asserted FALSE (assert_false checks Outcome::False,
// so an evaluation error would fail the test) and has an accepting twin.

#[cfg(test)]
mod phase9_v21 {
    use super::*;

    fn funding_input(value: u64, ch: u32) -> ErgoBox { make_output_box(&load_tree(POOL2_TREE_HEX), value, ch) }

    /// `n` distinct whitelist entries (32-byte hashes) added at height `a`.
    fn entries(n: usize, a: i64) -> Whitelist {
        (0..n).map(|i| (vec![(i as u8).wrapping_add(1); 32], a)).collect()
    }

    /// R1: a spend may not leave the fund below fundFloor.
    #[test]
    fn v21_spend_floor() {
        let tree = load_governance_tree();
        let wl = sample_whitelist();
        let s = make_idle_governance_box(&tree, 150 * NANOCOIN, wl.clone(), H - 1);
        let at = vec![make_governance_box(&tree, FUND_FLOOR, wl.clone(), 0, 0, H), make_output_box(&whitelisted_tree(), 50 * NANOCOIN, H)];
        assert_signed(&tree, &build_context(s.clone(), at, H), "change exactly at the floor");
        let below = vec![make_governance_box(&tree, FUND_FLOOR - 1, wl.clone(), 0, 0, H), make_output_box(&whitelisted_tree(), 50 * NANOCOIN + 1, H)];
        assert_false(&tree, &build_context(s, below, H), "change one nano below the floor");
    }

    /// R1: a box already below the floor may keep its value but not drop.
    #[test]
    fn v21_below_floor_may_keep_not_drop() {
        let tree = load_governance_tree();
        let wl = sample_whitelist();
        let s = make_idle_governance_box(&tree, 50 * NANOCOIN, wl.clone(), H - 1);
        let ins = vec![s, funding_input(10 * NANOCOIN, H - 1)];
        let keep = vec![make_governance_box(&tree, 50 * NANOCOIN, wl.clone(), 0, 0, H), make_output_box(&whitelisted_tree(), 10 * NANOCOIN, H)];
        assert_signed(&tree, &build_context_multi(ins.clone(), 0, keep, H), "keeps its value");
        let drop = vec![make_governance_box(&tree, 50 * NANOCOIN - 1, wl.clone(), 0, 0, H), make_output_box(&whitelisted_tree(), 10 * NANOCOIN + 1, H)];
        assert_false(&tree, &build_context_multi(ins, 0, drop, H), "drops one nano");
    }

    /// R1: the successor at this script must be created at HEIGHT - 720 or later
    /// (v2.2, N-1; was HEIGHT - 10), so a signed transaction stays valid for 720
    /// blocks. Rewritten for v2.2: the h-10 / h-11 cases are now h-720 / h-721.
    #[test]
    fn v21_successor_creation_height() {
        let tree = load_governance_tree();
        let wl = sample_whitelist();
        let s = make_idle_governance_box(&tree, 200 * NANOCOIN, wl.clone(), 100);
        for (what, out0_value) in [("spend", 190 * NANOCOIN), ("consolidation", 200 * NANOCOIN)] {
            let outs = |ch: u32| {
                let mut o = vec![make_governance_box(&tree, out0_value, wl.clone(), 0, 0, ch)];
                if out0_value < 200 * NANOCOIN {
                    o.push(make_output_box(&whitelisted_tree(), 10 * NANOCOIN, H));
                }
                o
            };
            assert_signed(&tree, &build_context(s.clone(), outs(H - SIGN_WINDOW), H), &format!("{}, creation h-720", what));
            assert_false(&tree, &build_context(s.clone(), outs(H - SIGN_WINDOW - 1), H), &format!("{}, creation h-721", what));
        }
        // Freeze (the emergency control): same window.
        let frozen = |ch: u32| make_governance_box(&tree, 200 * NANOCOIN, wl.clone(), 1, 0, ch);
        assert_signed(&tree, &build_context(s.clone(), vec![frozen(H - SIGN_WINDOW)], H), "freeze, creation h-720");
        assert_false(&tree, &build_context(s, vec![frozen(H - SIGN_WINDOW - 1)], H), "freeze, creation h-721");
    }

    /// R2: a migration to another script must arrive with nothing approved.
    /// The target has the fund register layout (a same-design contract).
    #[test]
    fn v21_migration_to_other_script_arrives_neutral() {
        let tree = load_governance_tree();
        let wl = sample_whitelist();
        let a = 100i64;
        let h = (a + TIMELOCK_BLOCKS + 1) as u32;
        let s = make_migration_box(&tree, 100 * NANOCOIN, &wl, &target_tree(), a, h - 1);
        let t = target_tree();
        let x = proposition_hash(&load_tree(DUMMY_TREE_HEX));
        let arrive = |r5: i64, r6: i64, r7: Vec<u8>| {
            make_box(&t, 100 * NANOCOIN, vec![make_nft_token()], lp_regs(&wl, r5, r6, r7), h)
        };
        assert_false(&tree, &build_context(s.clone(), vec![arrive(2, h as i64 - 8_642, x.clone())], h), "arrives armed for a second migration");
        assert_false(&tree, &build_context(s.clone(), vec![arrive(2, 0, x.clone())], h), "arrives with flag 2");
        assert_false(&tree, &build_context(s.clone(), vec![arrive(0, 5, x.clone())], h), "arrives with a migration height");
        assert_false(&tree, &build_context(s.clone(), vec![arrive(1, 0, genesis_r7())], h), "arrives with flag 1");
        assert_signed(&tree, &build_context(s.clone(), vec![arrive(0, 0, genesis_r7())], h), "R5 = 0 and R6 = 0");
        let bare = make_box(&t, 100 * NANOCOIN, vec![make_nft_token()], HashMap::new(), h);
        assert_signed(&tree, &build_context(s, vec![bare], h), "register-free target");
    }

    /// R7: the whitelist holds at most 64 entries.
    #[test]
    fn v21_whitelist_cap() {
        let tree = load_governance_tree();
        let upd = |old: Whitelist, add: usize| {
            let mut new = old.clone();
            for i in 0..add {
                new.push((vec![0xE0u8.wrapping_add(i as u8); 32], H as i64));
            }
            let s = make_idle_governance_box(&tree, 200 * NANOCOIN, old, H - 1);
            build_context(s, vec![make_governance_box(&tree, 200 * NANOCOIN, new, 0, 0, H)], H)
        };
        assert_signed(&tree, &upd(entries(MAX_WHITELIST - 1, 0), 1), "append to 64 entries");
        assert_false(&tree, &upd(entries(MAX_WHITELIST, 0), 1), "append to 65 entries");
        assert_false(&tree, &upd(entries(MAX_WHITELIST - 1, 0), 2), "append two, to 65 entries");
    }

    /// R7: a migration approval from a 64-entry box gives a successor under
    /// 4,096 bytes. Large values and heights maximise the VLQ widths.
    #[test]
    fn v21_migration_approval_from_64_entries_fits() {
        let tree = load_governance_tree();
        let h = 2_000_000u32;
        let wl = entries(MAX_WHITELIST, h as i64 - 5);
        let value = 10_000_000_000_000_000u64; // 1e16 nano
        let s = make_idle_governance_box(&tree, value, wl.clone(), h - 1);
        let out0 = make_migration_box(&tree, value, &wl, &target_tree(), h as i64, h);
        let in_size = s.sigma_serialize_bytes().unwrap().len();
        let out_size = out0.sigma_serialize_bytes().unwrap().len();
        println!("64-entry fund box: {} bytes; migration-approval successor: {} bytes", in_size, out_size);
        assert!(out_size < 4096, "successor is {} bytes", out_size);
        // Genesis R7 (32 zero bytes) already holds the target's room; only R6 grows.
        assert!(out_size <= in_size + 4, "approval grew the box by {} bytes", out_size - in_size);
        assert_signed(&tree, &build_context(s, vec![out0], h), "approval from a 64-entry box");
    }

    /// R7: an empty R7 is never a live target: migration needs flag 2.
    #[test]
    fn v21_zero_r7_is_not_a_live_target() {
        let tree = load_governance_tree();
        let wl = sample_whitelist();
        let h = 30_000u32;
        let s = make_lp_box(&tree, 100 * NANOCOIN, &wl, 0, 100, genesis_r7(), h - 1);
        let out0 = make_box(&target_tree(), 100 * NANOCOIN, vec![make_nft_token()], HashMap::new(), h);
        assert_false(&tree, &build_context(s, vec![out0], h), "R5 = 0 with a zero R7");
    }

    /// R4: a fund migrated to the accumulation script by mistake cannot be taken.
    #[test]
    fn v21_nft_at_accumulation_script_is_not_a_holder() {
        let la = load_accumulation_tree();
        let v = 100 * NANOCOIN;
        let stuck = make_box(&la, v, vec![make_nft_token()], HashMap::new(), 15_000);
        let own = make_output_box(&load_tree(DUMMY_TREE_HEX), v, 100);
        let taken = make_box(&load_tree(DUMMY_TREE_HEX), 2 * v, vec![make_nft_token()], HashMap::new(), H);
        assert_false(&la, &build_context_multi(vec![stuck, own], 0, vec![taken], H), "NFT box at the accumulation script as INPUTS(0)");
        let holder = make_box(&load_tree(DUMMY_TREE_HEX), v, vec![make_nft_token()], HashMap::new(), 100);
        let succ = make_box(&load_tree(DUMMY_TREE_HEX), 2 * v, vec![make_nft_token()], HashMap::new(), H);
        assert_true(&la, &build_context_multi(vec![holder, accum_box(v, 100)], 1, vec![succ], H), "holder at another script");
    }

    /// R5: the holder must carry the NFT at tokens(0).
    #[test]
    fn v21_holder_nft_must_be_at_tokens0() {
        let la = load_accumulation_tree();
        let home = load_tree(DUMMY_TREE_HEX);
        let other = token([0x42; 32], 5);
        let acc = accum_box(10 * NANOCOIN, 100);
        let succ = make_box(&home, 90 * NANOCOIN, vec![make_nft_token()], HashMap::new(), H);
        let at1 = make_box(&home, 80 * NANOCOIN, vec![other.clone(), make_nft_token()], HashMap::new(), 100);
        assert_false(&la, &build_context_multi(vec![at1, acc.clone()], 1, vec![succ.clone()], H), "NFT at tokens(1) of INPUTS(0)");
        let at0 = make_box(&home, 80 * NANOCOIN, vec![make_nft_token(), other.clone()], HashMap::new(), 100);
        assert_true(&la, &build_context_multi(vec![at0.clone(), acc.clone()], 1, vec![succ], H), "NFT at tokens(0) of INPUTS(0)");
        let succ1 = make_box(&home, 90 * NANOCOIN, vec![other, make_nft_token()], HashMap::new(), H);
        assert_false(&la, &build_context_multi(vec![at0, acc], 1, vec![succ1], H), "NFT at tokens(1) of O0");
    }

    /// R9 (F-023 variant): a lookalike box (another token at tokens(0)) spent next
    /// to the real fund box, spending into an O0 that holds the real NFT. Only the
    /// constant-NFT check on SELF stops it.
    #[test]
    fn v21_f023_lookalike_spend_beside_real_box() {
        let lf = load_governance_tree();
        let wl = sample_whitelist();
        let look = make_box(&lf, 300 * NANOCOIN, vec![token([42u8; 32], 1)], lp_regs(&wl, 0, 0, genesis_r7()), H - 1);
        let real = make_idle_governance_box(&lf, 300 * NANOCOIN, wl.clone(), H - 1);
        let outs = vec![make_governance_box(&lf, 200 * NANOCOIN, wl.clone(), 0, 0, H), make_output_box(&whitelisted_tree(), 100 * NANOCOIN, H)];
        assert_false(&lf, &build_context_multi(vec![look, real.clone()], 0, outs.clone(), H), "lookalike spends");
        let genuine = make_idle_governance_box(&lf, 300 * NANOCOIN, wl, H - 1);
        assert_signed(&lf, &build_context_multi(vec![genuine, real], 0, outs, H), "genuine SELF");
    }
}

// ============================================================
// v2.2 (2026-11 retest round 2): EA-N1, TRANSFER COUNTS THE HOLDER
// SCRIPT'S OWN INPUTS
// ============================================================
//
// EA-N1: [holder T with the NFT, tokenless box d at T's script, accumulation
// box a] -> [T' = T + max(d, a), attacker min(d, a)] passed the v2.1 transfer
// binding (O0 >= INPUTS(0).value + accSum) whenever T's script sums its own
// script's inputs. v2.2 binds O0 to every input at this script or at
// INPUTS(0)'s script.

#[cfg(test)]
mod phase10_v22 {
    use super::*;

    /// EA-N1 skim with a second tokenless input at the holder's script: FALSE.
    /// The honest twin (successor = sum of all three) is TRUE, one nano short FALSE.
    #[test]
    fn v22_ea_n1_skim_with_stray_at_holder_script_rejected() {
        let la = load_accumulation_tree();
        let lf = load_governance_tree();
        let wl = sample_whitelist();
        let t = make_idle_governance_box(&lf, 100 * NANOCOIN, wl.clone(), 100);
        let d = make_output_box(&lf, 50 * NANOCOIN, 100); // tokenless, at the holder's script
        let a = accum_box(50 * NANOCOIN, 100);
        let ins = vec![t, d, a];
        let attacker = make_output_box(&load_tree(DUMMY_TREE_HEX), 50 * NANOCOIN, H);
        let skim = vec![make_idle_governance_box(&lf, 150 * NANOCOIN, wl.clone(), H), attacker];
        assert_false(&la, &build_context_multi(ins.clone(), 2, skim, H), "T' = T + max(d, a), attacker takes min(d, a)");
        let honest = vec![make_idle_governance_box(&lf, 200 * NANOCOIN, wl.clone(), H)];
        assert_true(&la, &build_context_multi(ins.clone(), 2, honest, H), "holder' = holder + stray + acc");
        let short = vec![make_idle_governance_box(&lf, 200 * NANOCOIN - 1, wl, H)];
        assert_false(&la, &build_context_multi(ins, 2, short, H), "one nano short of the sum of all three");
    }

    /// The ordinary forward into the multisig is unchanged: [F, acc 10 + 5] ->
    /// [F' = F + 15] is TRUE for both accumulation boxes and SIGNED for F;
    /// one nano short is FALSE.
    #[test]
    fn v22_ordinary_forward_into_multisig_unchanged() {
        let la = load_accumulation_tree();
        let lf = load_governance_tree();
        let wl = sample_whitelist();
        let f = make_idle_governance_box(&lf, 100 * NANOCOIN, wl.clone(), 100);
        let ins = vec![f, accum_box(10 * NANOCOIN, 100), accum_box(5 * NANOCOIN, 200)];
        let succ = vec![make_idle_governance_box(&lf, 115 * NANOCOIN, wl.clone(), H)];
        for i in 1..3 {
            assert_true(&la, &build_context_multi(ins.clone(), i, succ.clone(), H), "accumulation side of the forward");
        }
        assert_signed(&lf, &build_context_multi(ins.clone(), 0, succ, H), "multisig consolidation side");
        let short = vec![make_idle_governance_box(&lf, 115 * NANOCOIN - 1, wl, H)];
        assert_false(&la, &build_context_multi(ins, 1, short, H), "one nano short");
    }
}

// ============================================================
// 30 s RETUNE
// ============================================================

#[cfg(test)]
mod retune_30s {
    use super::*;

    #[test]
    fn timelock_is_72_hours_at_30s_blocks() {
        assert_eq!(TIMELOCK_BLOCKS, 8_640);
        assert_eq!(TIMELOCK_BLOCKS * BLOCK_TIME_SECONDS, 72 * 3600);
        assert_eq!(WINDOW_BLOCKS * BLOCK_TIME_SECONDS, 7 * 24 * 3600);
        // The constant alone proves nothing: check the compiled tree carries Long 8640
        // and Long 20160, not 12960.
        assert!(GOVERNANCE_TREE_HEX.contains("05808701"), "tree lacks Long 8640");
        assert!(GOVERNANCE_TREE_HEX.contains("0580bb02"), "tree lacks Long 20160");
        assert!(!GOVERNANCE_TREE_HEX.contains("05c0ca01"), "tree contains Long 12960");
    }

    /// v2.0: lp_accumulation.es names its destination by the LP NFT id, not by the
    /// fund's script hash, so it no longer changes when lp_fund.es does.
    #[test]
    fn accumulation_embeds_current_governance_hash() {
        let gov_hash = hex_of(&blake(&hex_to_bytes(GOVERNANCE_TREE_HEX)));
        assert!(!ACCUMULATION_TREE_HEX.contains(&gov_hash), "accumulation must not embed the fund hash");
        let nft = format!("0e20{}", hex_of(&LP_NFT));
        assert!(ACCUMULATION_TREE_HEX.contains(&nft), "accumulation must embed the LP NFT id");
        assert!(GOVERNANCE_TREE_HEX.contains(&nft), "fund must embed the LP NFT id");
        assert!(GOVERNANCE_TREE_HEX.contains(MINER_FEE_HEX), "fund must embed the fee script");
    }
}
