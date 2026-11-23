//! treasury_test.rs: ErgoTree evaluation tests for the treasury contracts v2.2
//! (governance v2.2, accumulation v2.2).
//!
//! Trees: `trees/treasury_governance.hex` and `trees/treasury_accumulation.hex`,
//! written by `05-integration-tests/compile/gen_trees.py` from the .es sources in
//! this crate with the test NFT id aa..aa (TREASURY_NFT_ID_PLACEHOLDER). Run
//! `python3 gen_trees.py --check` there to confirm they match a fresh compile.
//!
//! Every assertion names the exact outcome of script reduction:
//!   TRUE   the script reduced to true (accumulation script, no signature)
//!   FALSE  the script reduced to false
//!   ERROR  evaluation failed
//!   SIGNED the boolean part held and only the 2-of-3 signature remains; an
//!          accept is SIGNED plus a 2-of-3 prove/verify that succeeds.
//! Tests use deterministic keypairs (public test keys, F-013).
//!
//! Targets: ergo-lib 0.28 with "arbitrary" feature, rustc 1.85+

use std::collections::HashMap;
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
    mir::constant::Constant,
    serialization::SigmaSerializable,
    sigma_protocol::sigma_boolean::SigmaBoolean,
};
use sigma_test_util::force_any_val;

type Blake2b256 = Blake2b<U32>;

// ============================================================
// TREES (fresh compile, see gen_trees.py)
// ============================================================

const GOVERNANCE_TREE_HEX: &str = include_str!("../trees/treasury_governance.hex");
const ACCUMULATION_TREE_HEX: &str = include_str!("../trees/treasury_accumulation.hex");

/// blake2b256 of the two trees above (re-pinned 2026-11-12, v2.2, test NFT id).
const GOVERNANCE_TREE_HASH: &str = "44494e8b322d1b48494d854c27292acb7e342da9977dfebd9c4787840ed0e07b";
const ACCUMULATION_TREE_HASH: &str = "2c923241dde1aba4f3d78d2b3719885a116894025832c9638ac4139be1ed611d";

/// ErgoTreePredef.feeProposition(720): the standard miner fee script.
const MINER_FEE_HEX: &str = "1005040004000e36100204a00b08cd0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798ea02d192a39a8cc7a701730073011001020402d19683030193a38cc7b2a57300000193c2b2a57301007473027303830108cdeeac93b1a57304";
/// Payment recipient: P2PK of k*G, k = 1.
const RECIPIENT_HEX: &str = "0008cd0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";
/// sigmaProp(HEIGHT > 0): an unrelated script.
const DUMMY_TREE_HEX: &str = "10010400d191a37300";
/// sigmaProp(true): a register-free migration target.
const TARGET_TREE_HEX: &str = "10010101d17300";

// ============================================================
// DETERMINISTIC SIGNER KEYS (test-only, never on mainnet)
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
const TX_FEE: u64 = 1_000_000; // migration value slack; also the usual fee
const EXEC_FEE_CAP: u64 = 10_000_000; // execution fee output cap (v2.1, was 1,000,000)
const COOLDOWN: u32 = 20_160; // accumulation re-merge cooldown (v2.1, was 720)
const FUND_FLOOR: u64 = 100 * NANOCOIN; // fundFloor (v2.1)
const SIGN_WINDOW: u32 = 720; // successor freshness, signWindow (v2.2, was 10)
/// Height for accumulation merge tests: boxes created at 100 or 200 are past the cooldown.
const MH: u32 = 50_000;
/// Test NFT id compiled into both trees (TREASURY_NFT_ID_PLACEHOLDER).
const TREASURY_NFT: [u8; 32] = [0xAA; 32];

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
fn recipient_tree() -> ErgoTree { load_tree(RECIPIENT_HEX) }
fn recipient_bytes() -> Vec<u8> { recipient_tree().sigma_serialize_bytes().unwrap() }
fn target_tree() -> ErgoTree { load_tree(TARGET_TREE_HEX) }

fn blake(bytes: &[u8]) -> Vec<u8> {
    let mut h = Blake2b256::new();
    h.update(bytes);
    h.finalize().to_vec()
}

fn tree_hash(tree: &ErgoTree) -> Vec<u8> { blake(&tree.sigma_serialize_bytes().unwrap()) }

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

/// Context with SELF as the only input.
fn build_context(self_box: ErgoBox, outputs: Vec<ErgoBox>, height: u32) -> Context<'static> {
    build_context_multi(vec![self_box], 0, outputs, height)
}

/// Prove with `prover` and verify. Used on top of `outcome` for accepts.
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

/// Governance accept: only the signature remains, 2-of-3 proves, 1-of-3 does not.
fn assert_signed(tree: &ErgoTree, ctx: &Context, msg: &str) {
    assert_eq!(outcome(tree, ctx), Outcome::Signed, "{}", msg);
    assert!(evaluate(tree, ctx, &prover_2_of_3()), "{} (2-of-3 prove/verify)", msg);
    assert!(!evaluate(tree, ctx, &prover_1_of_3()), "{} (1-of-3 must not prove)", msg);
}

/// Accumulation accept: reduces to true and proves with no secret.
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

/// Compute proposal hash: blake2b256(recipient_bytes ++ longToByteArray(amount))
fn compute_proposal_hash(recipient_bytes: &[u8], amount: i64) -> Vec<u8> {
    let mut hasher = Blake2b256::new();
    hasher.update(recipient_bytes);
    hasher.update(&amount.to_be_bytes());
    hasher.finalize().to_vec()
}

// ============================================================
// BOX CONSTRUCTION
// ============================================================

fn token(id: [u8; 32], amount: u64) -> Token {
    Token { token_id: TokenId::from(Digest32::from(id)), amount: TokenAmount::try_from(amount).unwrap() }
}

fn make_nft_token() -> Token { token(TREASURY_NFT, 1) }

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

fn gov_regs(r4: Vec<u8>, r5: i64, r6: Vec<u8>, r7: i64, r8: i64) -> HashMap<R, Constant> {
    HashMap::from([
        (R::R4, Constant::from(r4)), (R::R5, Constant::from(r5)), (R::R6, Constant::from(r6)),
        (R::R7, Constant::from(r7)), (R::R8, Constant::from(r8)),
    ])
}

/// Build a governance box with NFT and all registers.
fn make_governance_box(
    tree: &ErgoTree,
    value: u64,
    proposal_hash: Vec<u8>,
    approval_height: i64,
    recipient: Vec<u8>,
    amount: i64,
    state_flag: i64,
    creation_height: u32,
) -> ErgoBox {
    make_box(tree, value, vec![make_nft_token()],
        gov_regs(proposal_hash, approval_height, recipient, amount, state_flag), creation_height)
}

/// Governance box with NFT but using a different script (for migration output)
fn make_governance_box_other_script(
    tree: &ErgoTree,
    value: u64,
    proposal_hash: Vec<u8>,
    approval_height: i64,
    recipient: Vec<u8>,
    amount: i64,
    state_flag: i64,
    creation_height: u32,
) -> ErgoBox {
    make_governance_box(tree, value, proposal_hash, approval_height, recipient, amount, state_flag, creation_height)
}

/// Shorthand: idle governance box (genesis layout: R4 32 zero bytes, R5 0, R6 empty, R7 0, R8 0)
fn make_idle_governance_box(tree: &ErgoTree, value: u64, creation_height: u32) -> ErgoBox {
    make_governance_box(tree, value, vec![0u8; 32], 0i64, vec![], 0i64, 0i64, creation_height)
}

/// Shorthand: governance box with a payment proposal (R4 consistent with R6, R7)
fn make_proposal_governance_box(
    tree: &ErgoTree, value: u64, recipient_bytes: Vec<u8>,
    amount: i64, approval_height: i64, state_flag: i64, creation_height: u32,
) -> ErgoBox {
    let proposal_hash = compute_proposal_hash(&recipient_bytes, amount);
    make_governance_box(tree, value, proposal_hash, approval_height, recipient_bytes, amount, state_flag, creation_height)
}

/// Box in the migration-approved state for target `target`.
fn make_migration_box(tree: &ErgoTree, value: u64, target: &ErgoTree, approval_height: i64, creation_height: u32) -> ErgoBox {
    make_governance_box(tree, value, tree_hash(target), approval_height, vec![], 0, 2, creation_height)
}

/// Copy of `b` with new value and new R5 / R8 (R4, R6, R7 kept).
fn successor(b: &ErgoBox, value: u64, r5: i64, r8: i64, h: u32) -> ErgoBox {
    let r = |id: R| b.additional_registers.get_constant(id).unwrap().unwrap();
    let mut regs = HashMap::new();
    regs.insert(R::R4, r(R::R4));
    regs.insert(R::R5, Constant::from(r5));
    regs.insert(R::R6, r(R::R6));
    regs.insert(R::R7, r(R::R7));
    regs.insert(R::R8, Constant::from(r8));
    make_box(&b.ergo_tree, value, b.tokens.clone().map(|t| t.to_vec()).unwrap_or_default(), regs, h)
}

fn reg_i64(b: &ErgoBox, id: R) -> i64 {
    use ergo_lib::ergotree_ir::mir::constant::TryExtractInto;
    b.additional_registers.get_constant(id).unwrap().unwrap().v.try_extract_into::<i64>().unwrap()
}

fn val(b: &ErgoBox) -> u64 { *b.value.as_u64() }

/// Honest execution at `exec_h` of a proposal approved at `approval_h`:
/// [SELF] -> [change, payment, fee]. SELF holds 200 YOLO so the change stays
/// above the 100 YOLO fund floor (v2.1; was 100 YOLO).
fn execution_parts(approval_h: i64, exec_h: u32) -> (ErgoTree, ErgoBox, Vec<ErgoBox>) {
    let tree = load_governance_tree();
    let value = 200 * NANOCOIN;
    let amount = 10i64 * NANOCOIN as i64;
    let self_box = make_proposal_governance_box(&tree, value, recipient_bytes(), amount, approval_h, 0, exec_h - 1);
    let out0 = successor(&self_box, value - amount as u64 - TX_FEE, 0, 0, exec_h);
    let out1 = make_output_box(&recipient_tree(), amount as u64, exec_h);
    (tree, self_box, vec![out0, out1, fee_box(TX_FEE, exec_h)])
}

/// Accumulation box created at `ch`.
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
        println!("governance contract v2.2 ErgoTree size: {} bytes", size);
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
}

#[cfg(test)]
mod phase1_threshold {
    use super::*;

    #[test]
    fn accept_2_of_3_threshold() {
        let tree = load_governance_tree();
        let h = 100u32;
        let value = 100 * NANOCOIN;
        let rb = recipient_bytes();
        let amount = 10i64 * NANOCOIN as i64;
        let proposal_hash = compute_proposal_hash(&rb, amount);

        let self_box = make_idle_governance_box(&tree, value, h - 1);
        let out0 = make_governance_box(&tree, value, proposal_hash, h as i64, rb, amount, 0i64, h);
        let ctx = build_context(self_box, vec![out0], h);
        assert_signed(&tree, &ctx, "2-of-3 threshold should accept");
    }

    #[test]
    fn reject_1_of_3_threshold() {
        let tree = load_governance_tree();
        let h = 100u32;
        let value = 100 * NANOCOIN;
        let rb = recipient_bytes();
        let amount = 10i64 * NANOCOIN as i64;
        let proposal_hash = compute_proposal_hash(&rb, amount);

        let self_box = make_idle_governance_box(&tree, value, h - 1);
        let out0 = make_governance_box(&tree, value, proposal_hash, h as i64, rb, amount, 0i64, h);
        let ctx = build_context(self_box, vec![out0], h);
        assert!(!evaluate(&tree, &ctx, &prover_1_of_3()), "1-of-3 should reject");
    }
}

#[cfg(test)]
mod phase2_approval {
    use super::*;

    fn approval(h: u32, recorded: i64) -> (ErgoTree, Context<'static>) {
        let tree = load_governance_tree();
        let value = 100 * NANOCOIN;
        let rb = recipient_bytes();
        let amount = 10i64 * NANOCOIN as i64;
        let self_box = make_idle_governance_box(&tree, value, h - 1);
        let out0 = make_governance_box(&tree, value, compute_proposal_hash(&rb, amount), recorded, rb, amount, 0i64, h);
        let ctx = build_context(self_box, vec![out0], h);
        (tree, ctx)
    }

    #[test]
    fn accept_approval_valid_proposal() {
        let (tree, ctx) = approval(100, 100);
        assert_signed(&tree, &ctx, "valid approval should accept");
    }

    #[test]
    fn reject_approval_inconsistent_hash() {
        let tree = load_governance_tree();
        let h = 100u32;
        let value = 100 * NANOCOIN;
        let rb = recipient_bytes();
        let amount = 10i64 * NANOCOIN as i64;
        // Wrong hash: doesn't match R6/R7
        let bad_hash = vec![0xFFu8; 32];

        let self_box = make_idle_governance_box(&tree, value, h - 1);
        let out0 = make_governance_box(&tree, value, bad_hash, h as i64, rb, amount, 0i64, h);
        let ctx = build_context(self_box, vec![out0], h);
        assert_false(&tree, &ctx, "inconsistent proposal hash should reject");
    }

    #[test]
    fn reject_approval_when_proposal_active() {
        let tree = load_governance_tree();
        let h = 200u32;
        let value = 100 * NANOCOIN;
        let rb = recipient_bytes();
        let amount = 10i64 * NANOCOIN as i64;

        let self_box = make_proposal_governance_box(&tree, value, rb.clone(), amount, 50, 0, h - 1);
        let proposal_hash = compute_proposal_hash(&rb, amount);
        let out0 = make_governance_box(&tree, value, proposal_hash, h as i64, rb, amount, 0i64, h);
        let ctx = build_context(self_box, vec![out0], h);
        assert_false(&tree, &ctx, "approval with active proposal should reject");
    }

    #[test]
    fn reject_approval_when_frozen() {
        let tree = load_governance_tree();
        let h = 100u32;
        let value = 100 * NANOCOIN;
        let rb = recipient_bytes();
        let amount = 10i64 * NANOCOIN as i64;
        let proposal_hash = compute_proposal_hash(&rb, amount);

        let self_box = make_governance_box(&tree, value, vec![0u8; 32], 0i64, vec![], 0i64, 1i64, h - 1);
        let out0 = make_governance_box(&tree, value, proposal_hash, h as i64, rb, amount, 1i64, h);
        let ctx = build_context(self_box, vec![out0], h);
        assert_false(&tree, &ctx, "approval when frozen should reject");
    }

    #[test]
    fn reject_approval_value_not_preserved() {
        let tree = load_governance_tree();
        let h = 100u32;
        let value = 100 * NANOCOIN;
        let rb = recipient_bytes();
        let amount = 10i64 * NANOCOIN as i64;
        let proposal_hash = compute_proposal_hash(&rb, amount);

        let self_box = make_idle_governance_box(&tree, value, h - 1);
        let out0 = make_governance_box(&tree, value - NANOCOIN, proposal_hash, h as i64, rb, amount, 0i64, h);
        let ctx = build_context(self_box, vec![out0], h);
        assert_false(&tree, &ctx, "approval with reduced value should reject");
    }

    #[test]
    fn reject_approval_insufficient_signers() {
        let (tree, ctx) = approval(100, 100);
        assert_eq!(outcome(&tree, &ctx), Outcome::Signed);
        assert!(!evaluate(&tree, &ctx, &prover_1_of_3()), "1-of-3 should reject");
    }

    /// F-016: the recorded height may be up to 10 blocks old, never ahead, never <= 0.
    #[test]
    fn f016_approval_height_range() {
        let h = 20_000u32;
        let (tree, ctx) = approval(h, h as i64 - 3);
        assert_signed(&tree, &ctx, "signed for h-3, mined at h");
        let (tree, ctx) = approval(h, h as i64 - 10);
        assert_signed(&tree, &ctx, "lower edge h-10");
        let (tree, ctx) = approval(h, h as i64 - 11);
        assert_false(&tree, &ctx, "h-11 is outside the range");
        let (tree, ctx) = approval(h, h as i64 + 1);
        assert_false(&tree, &ctx, "a future height is rejected");
        // At a very low height the range reaches below 1: 0 and negatives are rejected.
        let (tree, ctx) = approval(5, 0);
        assert_false(&tree, &ctx, "recorded height 0 is rejected");
        let (tree, ctx) = approval(5, -3);
        assert_false(&tree, &ctx, "negative recorded height is rejected");
        let (tree, ctx) = approval(5, 1);
        assert_signed(&tree, &ctx, "recorded height 1 at HEIGHT 5");
    }
}

#[cfg(test)]
mod phase3_execution {
    use super::*;

    #[test]
    fn accept_execution_after_timelock() {
        let approval_h = 100i64;
        let (tree, s, outs) = execution_parts(approval_h, (approval_h + TIMELOCK_BLOCKS + 1) as u32);
        let ctx = build_context(s, outs, (approval_h + TIMELOCK_BLOCKS + 1) as u32);
        assert_signed(&tree, &ctx, "execution after timelock should accept");
    }

    #[test]
    fn reject_execution_before_timelock() {
        let approval_h = 100i64;
        let exec_h = (approval_h + TIMELOCK_BLOCKS) as u32; // exact boundary, not past
        let (tree, s, outs) = execution_parts(approval_h, exec_h);
        let ctx = build_context(s, outs, exec_h);
        assert_false(&tree, &ctx, "execution before timelock should reject");
    }

    #[test]
    fn reject_execution_when_frozen() {
        let tree = load_governance_tree();
        let approval_h = 100i64;
        let exec_h = (approval_h + TIMELOCK_BLOCKS + 1) as u32;
        let value = 200 * NANOCOIN; // above the floor after paying (v2.1)
        let rb = recipient_bytes();
        let amount = 10i64 * NANOCOIN as i64;

        let self_box = make_proposal_governance_box(&tree, value, rb.clone(), amount, approval_h, 1, exec_h - 1);
        let out0 = successor(&self_box, value - amount as u64 - TX_FEE, 0, 0, exec_h);
        let out1 = make_output_box(&recipient_tree(), amount as u64, exec_h);
        let ctx = build_context(self_box, vec![out0, out1, fee_box(TX_FEE, exec_h)], exec_h);
        assert_false(&tree, &ctx, "execution when frozen should reject");
    }

    #[test]
    fn reject_execution_wrong_recipient() {
        let approval_h = 100i64;
        let exec_h = (approval_h + TIMELOCK_BLOCKS + 1) as u32;
        let (tree, s, mut outs) = execution_parts(approval_h, exec_h);
        outs[1] = make_output_box(&tree, val(&outs[1]), exec_h); // wrong recipient
        let ctx = build_context(s, outs, exec_h);
        assert_false(&tree, &ctx, "wrong recipient should reject");
    }

    #[test]
    fn reject_execution_wrong_amount() {
        let approval_h = 100i64;
        let exec_h = (approval_h + TIMELOCK_BLOCKS + 1) as u32;
        let (tree, s, mut outs) = execution_parts(approval_h, exec_h);
        outs[1] = make_output_box(&recipient_tree(), val(&outs[1]) - 1, exec_h); // short by 1
        outs[0] = successor(&outs[0], val(&outs[0]) + 1, 0, 0, exec_h);
        let ctx = build_context(s, outs, exec_h);
        assert_false(&tree, &ctx, "wrong amount should reject");
    }

    #[test]
    fn reject_execution_no_active_proposal() {
        let tree = load_governance_tree();
        let exec_h = 20000u32;
        let value = 200 * NANOCOIN; // above the floor after paying (v2.1)
        let amount = 10i64 * NANOCOIN as i64;

        let self_box = make_idle_governance_box(&tree, value, exec_h - 1);
        let out0 = successor(&self_box, value - amount as u64 - TX_FEE, 0, 0, exec_h);
        let out1 = make_output_box(&recipient_tree(), amount as u64, exec_h);
        let ctx = build_context(self_box, vec![out0, out1, fee_box(TX_FEE, exec_h)], exec_h);
        assert_false(&tree, &ctx, "no proposal should reject");
    }

    #[test]
    fn reject_execution_change_not_to_self() {
        let approval_h = 100i64;
        let exec_h = (approval_h + TIMELOCK_BLOCKS + 1) as u32;
        let (tree, s, mut outs) = execution_parts(approval_h, exec_h);
        // Change at another script, still holding the NFT and typed registers.
        outs[0] = make_governance_box_other_script(&load_accumulation_tree(), val(&outs[0]),
            vec![0u8; 32], 0, vec![], 0, 0, exec_h);
        let ctx = build_context(s, outs, exec_h);
        assert_false(&tree, &ctx, "change to wrong script should reject");
    }

    #[test]
    fn reject_execution_proposal_not_cleared() {
        let approval_h = 100i64;
        let exec_h = (approval_h + TIMELOCK_BLOCKS + 1) as u32;
        let (tree, s, mut outs) = execution_parts(approval_h, exec_h);
        outs[0] = successor(&outs[0], val(&outs[0]), approval_h, 0, exec_h); // R5 not cleared
        let ctx = build_context(s, outs, exec_h);
        assert_false(&tree, &ctx, "proposal not cleared should reject");
    }

    /// F-007: payment execution has an upper bound: approval + D + W.
    #[test]
    fn f007_execution_window_closes() {
        let a = 100i64;
        let last = (a + TIMELOCK_BLOCKS + WINDOW_BLOCKS) as u32;
        let (tree, s, outs) = execution_parts(a, last);
        assert_signed(&tree, &build_context(s, outs, last), "last block of the window");
        let (tree, s, outs) = execution_parts(a, last + 1);
        assert_false(&tree, &build_context(s, outs, last + 1), "one block after the window");
        let (tree, s, outs) = execution_parts(a, last + 1_000_000);
        assert_false(&tree, &build_context(s, outs, last + 1_000_000), "long after the window");
    }

    /// F-018: execution needs the normal state; a migration-approved box cannot pay.
    #[test]
    fn f018_execution_rejected_from_migration_state() {
        let tree = load_governance_tree();
        let a = 100i64;
        let exec_h = (a + TIMELOCK_BLOCKS + 1) as u32;
        let rb = recipient_bytes();
        let amount = 10i64 * NANOCOIN as i64;
        // R8 = 2 with a payee and amount whose hash is consistent.
        let s = make_proposal_governance_box(&tree, 200 * NANOCOIN, rb, amount, a, 2, exec_h - 1);
        let out0 = successor(&s, 200 * NANOCOIN - amount as u64 - TX_FEE, 0, 0, exec_h);
        let outs = vec![out0, make_output_box(&recipient_tree(), amount as u64, exec_h), fee_box(TX_FEE, exec_h)];
        assert_false(&tree, &build_context(s, outs, exec_h), "execution from R8 = 2");
    }

    /// F-018: execution checks the proposal hash against R6 and R7 of SELF.
    #[test]
    fn f018_execution_checks_proposal_hash() {
        let tree = load_governance_tree();
        let a = 100i64;
        let exec_h = (a + TIMELOCK_BLOCKS + 1) as u32;
        let amount = 10i64 * NANOCOIN as i64;
        // R4 does not match R6 ++ R7 (as after a migration approval or a hand-built box).
        let s = make_governance_box(&tree, 200 * NANOCOIN, vec![0x11; 32], a, recipient_bytes(), amount, 0, exec_h - 1);
        let out0 = successor(&s, 200 * NANOCOIN - amount as u64 - TX_FEE, 0, 0, exec_h);
        let outs = vec![out0, make_output_box(&recipient_tree(), amount as u64, exec_h), fee_box(TX_FEE, exec_h)];
        assert_false(&tree, &build_context(s, outs, exec_h), "inconsistent proposal hash");
    }

    /// F-021: the slack below SELF.value - amount must go to a miner fee output.
    /// v2.1: the cap is 10,000,000 (was 1,000,000).
    #[test]
    fn f021_execution_slack_tied_to_fee_output() {
        let a = 100i64;
        let h = (a + TIMELOCK_BLOCKS + 1) as u32;
        let (tree, s, outs) = execution_parts(a, h);
        // Slack to an arbitrary output instead of the fee script.
        let mut o = outs.clone();
        o[2] = make_output_box(&load_tree(DUMMY_TREE_HEX), TX_FEE, h);
        assert_false(&tree, &build_context(s.clone(), o, h), "slack to a non-fee output");
        // Slack taken with no fee output at all.
        assert_false(&tree, &build_context(s.clone(), outs[..2].to_vec(), h), "slack with no fee output");
        // Fee output at the cap (accepted) and one nano above it (FALSE).
        let mut o = outs.clone();
        o[0] = successor(&outs[0], val(&outs[0]) + TX_FEE - EXEC_FEE_CAP, 0, 0, h);
        o[2] = fee_box(EXEC_FEE_CAP, h);
        assert_signed(&tree, &build_context(s.clone(), o, h), "fee at 10,000,000");
        let mut o = outs.clone();
        o[0] = successor(&outs[0], val(&outs[0]) + TX_FEE - EXEC_FEE_CAP - 1, 0, 0, h);
        o[2] = fee_box(EXEC_FEE_CAP + 1, h);
        assert_false(&tree, &build_context(s.clone(), o, h), "fee above 10,000,000");
        // Two fee outputs.
        let mut o = outs.clone();
        o[0] = successor(&outs[0], val(&outs[0]) + TX_FEE / 2, 0, 0, h);
        o[2] = fee_box(TX_FEE / 2, h);
        o.push(fee_box(TX_FEE / 2, h));
        assert_false(&tree, &build_context(s.clone(), o, h), "two fee outputs");
        // No slack: [change = V - A, payment] needs no fee output (F-024: two outputs).
        let mut o = outs[..2].to_vec();
        o[0] = successor(&outs[0], val(&outs[0]) + TX_FEE, 0, 0, h);
        assert_signed(&tree, &build_context(s.clone(), o, h), "no slack, two outputs");
        // Honest fee output smaller than the cap, change correspondingly larger.
        let mut o = outs.clone();
        o[0] = successor(&outs[0], val(&outs[0]) + TX_FEE / 2, 0, 0, h);
        o[2] = fee_box(TX_FEE / 2, h);
        assert_signed(&tree, &build_context(s, o, h), "half fee");
    }

    /// F-024: execution needs exactly the payment output after O0; a one-output
    /// transaction is FALSE, not an evaluation error.
    #[test]
    fn f024_execution_with_one_output_is_false_not_error() {
        let a = 100i64;
        let h = (a + TIMELOCK_BLOCKS + 1) as u32;
        let (tree, s, outs) = execution_parts(a, h);
        assert_false(&tree, &build_context(s, outs[..1].to_vec(), h), "no payment output");
    }

    /// Execution successor keeps R4, R6, R7 (field-by-field successor).
    #[test]
    fn reject_execution_successor_changes_proposal_fields() {
        let a = 100i64;
        let h = (a + TIMELOCK_BLOCKS + 1) as u32;
        let (tree, s, outs) = execution_parts(a, h);
        let mut o = outs.clone();
        o[0] = make_governance_box(&tree, val(&outs[0]), vec![0u8; 32], 0, vec![], 0, 0, h);
        assert_false(&tree, &build_context(s, o, h), "R4/R6/R7 rewritten on execution");
    }
}

#[cfg(test)]
mod phase4_cancellation {
    use super::*;

    #[test]
    fn accept_cancellation_active_proposal() {
        let tree = load_governance_tree();
        let h = 200u32;
        let value = 100 * NANOCOIN;
        let rb = recipient_bytes();
        let amount = 10i64 * NANOCOIN as i64;

        let self_box = make_proposal_governance_box(&tree, value, rb.clone(), amount, 50, 0, h - 1);
        let out0 = successor(&self_box, value, 0, 0, h);
        let ctx = build_context(self_box, vec![out0], h);
        assert_signed(&tree, &ctx, "cancellation should accept (one output, F-024)");
    }

    #[test]
    fn reject_cancellation_value_stolen() {
        let tree = load_governance_tree();
        let h = 200u32;
        let value = 100 * NANOCOIN;
        let rb = recipient_bytes();
        let amount = 10i64 * NANOCOIN as i64;

        let self_box = make_proposal_governance_box(&tree, value, rb.clone(), amount, 50, 0, h - 1);
        let out0 = successor(&self_box, value - NANOCOIN, 0, 0, h);
        let dummy = make_output_box(&tree, NANOCOIN, h);
        let ctx = build_context(self_box, vec![out0, dummy], h);
        assert_false(&tree, &ctx, "cancel with stolen value should reject");
    }

    /// F-004: cancelling a migration approval must clear the migration flag.
    #[test]
    fn f004_cancel_of_migration_resets_flag() {
        let tree = load_governance_tree();
        let h = 10_000u32;
        let s = make_migration_box(&tree, 500 * NANOCOIN, &target_tree(), h as i64, h - 1);
        // Old behaviour: R5 -> 0 with R8 kept at 2. Now rejected.
        let keep2 = successor(&s, 500 * NANOCOIN, 0, 2, h);
        assert_false(&tree, &build_context(s.clone(), vec![keep2], h), "cancel keeping R8 = 2");
        let reset = successor(&s, 500 * NANOCOIN, 0, 0, h);
        assert_signed(&tree, &build_context(s, vec![reset], h), "cancel resets R8 to 0");
    }

    /// F-004 / F-007: a box with R8 = 2 and R5 = 0 (the old cancelled state) cannot migrate.
    #[test]
    fn f004_migration_execute_needs_positive_approval_height() {
        let tree = load_governance_tree();
        let h = 10_000u32;
        let s = make_governance_box(&tree, 500 * NANOCOIN, tree_hash(&target_tree()), 0, vec![], 0, 2, h - 1);
        let out0 = make_box(&target_tree(), 500 * NANOCOIN, vec![make_nft_token()], HashMap::new(), h);
        assert_false(&tree, &build_context(s, vec![out0], h), "R5 = 0 with R8 = 2");
    }

    /// F-004 full sequence: approve, cancel, migrate in the same block.
    #[test]
    fn f004_approve_cancel_execute_sequence_rejected() {
        let tree = load_governance_tree();
        let h = 10_000u32;
        let idle = make_idle_governance_box(&tree, 500 * NANOCOIN, h - 1);
        let approved = make_migration_box(&tree, 500 * NANOCOIN, &target_tree(), h as i64, h);
        assert_signed(&tree, &build_context(idle, vec![approved.clone()], h), "step 1 approval");
        let cancelled = successor(&approved, 500 * NANOCOIN, 0, 0, h);
        assert_signed(&tree, &build_context(approved, vec![cancelled.clone()], h), "step 2 cancel");
        let out0 = make_box(&target_tree(), 500 * NANOCOIN, vec![make_nft_token()], HashMap::new(), h);
        assert_false(&tree, &build_context(cancelled, vec![out0], h), "step 3 execute must fail");
    }
}

#[cfg(test)]
mod phase5_consolidation {
    use super::*;

    #[test]
    fn accept_consolidation_value_increases() {
        let tree = load_governance_tree();
        let h = 100u32;
        let value = 50 * NANOCOIN;

        let self_box = make_idle_governance_box(&tree, value, h - 1);
        let out0 = make_idle_governance_box(&tree, value + 10 * NANOCOIN, h);
        let ctx = build_context(self_box, vec![out0], h);
        assert_signed(&tree, &ctx, "consolidation should accept (one output, F-024)");
    }

    #[test]
    fn reject_consolidation_when_frozen() {
        let tree = load_governance_tree();
        let h = 100u32;
        let value = 50 * NANOCOIN;

        let self_box = make_governance_box(&tree, value, vec![0u8; 32], 0i64, vec![], 0i64, 1i64, h - 1);
        let out0 = make_governance_box(&tree, value + 10 * NANOCOIN, vec![0u8; 32], 0i64, vec![], 0i64, 1i64, h);
        let ctx = build_context(self_box, vec![out0], h);
        assert_false(&tree, &ctx, "consolidation when frozen should reject");
    }

    #[test]
    fn reject_consolidation_registers_changed() {
        let tree = load_governance_tree();
        let h = 100u32;
        let value = 50 * NANOCOIN;

        let self_box = make_idle_governance_box(&tree, value, h - 1);
        // Change R7: no path allows this during consolidation
        let out0 = make_governance_box(&tree, value + 10 * NANOCOIN, vec![0u8; 32], 0i64, vec![], 999i64, 0i64, h);
        let ctx = build_context(self_box, vec![out0], h);
        assert_false(&tree, &ctx, "changed registers should reject");
    }

    #[test]
    fn reject_consolidation_value_decreases() {
        let tree = load_governance_tree();
        let h = 100u32;
        let value = 50 * NANOCOIN;

        let self_box = make_idle_governance_box(&tree, value, h - 1);
        let out0 = make_idle_governance_box(&tree, value - NANOCOIN, h);
        let ctx = build_context(self_box, vec![out0], h);
        assert_false(&tree, &ctx, "decreased value should reject");
    }

    /// F-006: the migration commitment (R4) survives consolidation unchanged.
    #[test]
    fn f006_consolidation_preserves_migration_commitment() {
        let tree = load_governance_tree();
        let h = 200u32;
        let s = make_migration_box(&tree, 50 * NANOCOIN, &target_tree(), 150, h - 1);
        let ok = successor(&s, 60 * NANOCOIN, 150, 2, h);
        assert_signed(&tree, &build_context(s.clone(), vec![ok], h), "commitment kept");
        let other = make_governance_box(&tree, 60 * NANOCOIN, tree_hash(&load_tree(DUMMY_TREE_HEX)), 150, vec![], 0, 2, h);
        assert_false(&tree, &build_context(s, vec![other], h), "commitment rewritten by consolidation");
    }
}

#[cfg(test)]
mod phase6_freeze {
    use super::*;

    #[test]
    fn accept_freeze_threshold_sig() {
        let tree = load_governance_tree();
        let h = 100u32;
        let value = 100 * NANOCOIN;

        let self_box = make_idle_governance_box(&tree, value, h - 1);
        let out0 = successor(&self_box, value, 0, 1, h);
        let ctx = build_context(self_box, vec![out0], h);
        assert_signed(&tree, &ctx, "freeze should accept (one output, F-024)");
    }

    #[test]
    fn reject_freeze_already_frozen() {
        let tree = load_governance_tree();
        let h = 100u32;
        let value = 100 * NANOCOIN;

        let self_box = make_governance_box(&tree, value, vec![0u8; 32], 0i64, vec![], 0i64, 1i64, h - 1);
        let out0 = successor(&self_box, value, 0, 1, h);
        let ctx = build_context(self_box, vec![out0], h);
        assert_false(&tree, &ctx, "already frozen should reject");
    }

    /// F-012: unfreeze returns the flag to 0 and keeps everything else.
    #[test]
    fn f012_unfreeze_path() {
        let tree = load_governance_tree();
        let h = 500u32;
        let rb = recipient_bytes();
        let amount = 10i64 * NANOCOIN as i64;
        let frozen = make_proposal_governance_box(&tree, 100 * NANOCOIN, rb, amount, 400, 1, h - 1);
        let ok = successor(&frozen, 100 * NANOCOIN, 400, 0, h);
        assert_signed(&tree, &build_context(frozen.clone(), vec![ok], h), "unfreeze");
        let bad = successor(&frozen, 100 * NANOCOIN, 0, 0, h);
        assert_false(&tree, &build_context(frozen.clone(), vec![bad], h), "unfreeze must keep R5");
        let bad = successor(&frozen, 100 * NANOCOIN, 400, 2, h);
        assert_false(&tree, &build_context(frozen.clone(), vec![bad], h), "unfreeze to 2");
        let bad = successor(&frozen, 99 * NANOCOIN, 400, 0, h);
        assert_false(&tree, &build_context(frozen, vec![bad], h), "unfreeze losing value");
        let idle = make_idle_governance_box(&tree, 100 * NANOCOIN, h - 1);
        let out = successor(&idle, 100 * NANOCOIN, 0, 0, h);
        // From state 0 the same output is a consolidation, so it is accepted;
        // an unfreeze of a box that is not frozen is not a separate capability.
        assert_signed(&tree, &build_context(idle, vec![out], h), "state 0 -> 0 is consolidation");
    }

    /// F-012: a frozen migration approval comes back as state 0 with R5 > 0 and
    /// a target hash in R4; it cannot pay and cannot migrate, only be cancelled.
    #[test]
    fn f012_unfrozen_migration_cannot_pay_or_migrate() {
        let tree = load_governance_tree();
        let a = 100i64;
        // 200 YOLO (v2.1, was 100): the payment attempt below stays above the
        // floor, so only the commitment check rejects it.
        let s = make_migration_box(&tree, 200 * NANOCOIN, &target_tree(), a, 150);
        let frozen = successor(&s, 200 * NANOCOIN, a, 1, 200);
        assert_signed(&tree, &build_context(s, vec![frozen.clone()], 200), "freeze in state 2");
        let back = successor(&frozen, 200 * NANOCOIN, a, 0, 300);
        assert_signed(&tree, &build_context(frozen, vec![back.clone()], 300), "unfreeze");
        let h = (a + TIMELOCK_BLOCKS + 1) as u32;
        let out0 = make_box(&target_tree(), 200 * NANOCOIN, vec![make_nft_token()], HashMap::new(), h);
        assert_false(&tree, &build_context(back.clone(), vec![out0], h), "no migration from state 0");
        let pay = successor(&back, 200 * NANOCOIN - NANOCOIN - TX_FEE, 0, 0, h);
        let outs = vec![pay, make_output_box(&recipient_tree(), NANOCOIN, h), fee_box(TX_FEE, h)];
        assert_false(&tree, &build_context(back.clone(), outs, h), "no payment from a migration commitment");
        let cancel = successor(&back, 200 * NANOCOIN, 0, 0, h);
        assert_signed(&tree, &build_context(back, vec![cancel], h), "cancel");
    }
}

#[cfg(test)]
mod phase7_migration {
    use super::*;

    fn approval_out(tree: &ErgoTree, value: u64, r4: Vec<u8>, r5: i64, r6: Vec<u8>, r7: i64, h: u32) -> ErgoBox {
        make_governance_box(tree, value, r4, r5, r6, r7, 2, h)
    }

    #[test]
    fn accept_migration_approval() {
        let tree = load_governance_tree();
        let h = 100u32;
        let value = 100 * NANOCOIN;

        let self_box = make_idle_governance_box(&tree, value, h - 1);
        // Output: R8=2 (migration flag), R5=HEIGHT, R4 = target hash, R6 empty, R7 0
        let out0 = approval_out(&tree, value, tree_hash(&target_tree()), h as i64, vec![], 0, h);
        let ctx = build_context(self_box, vec![out0], h);
        assert_signed(&tree, &ctx, "migration approval should accept");
    }

    #[test]
    fn reject_migration_approval_when_frozen() {
        let tree = load_governance_tree();
        let h = 100u32;
        let value = 100 * NANOCOIN;

        let self_box = make_governance_box(&tree, value, vec![0u8; 32], 0i64, vec![], 0i64, 1i64, h - 1);
        let out0 = approval_out(&tree, value, tree_hash(&target_tree()), h as i64, vec![], 0, h);
        let ctx = build_context(self_box, vec![out0], h);
        assert_false(&tree, &ctx, "migration approval when frozen should reject");
    }

    #[test]
    fn reject_migration_approval_when_proposal_active() {
        let tree = load_governance_tree();
        let h = 200u32;
        let value = 100 * NANOCOIN;
        let rb = recipient_bytes();
        let amount = 10i64 * NANOCOIN as i64;

        let self_box = make_proposal_governance_box(&tree, value, rb, amount, 50, 0, h - 1);
        let out0 = approval_out(&tree, value, tree_hash(&target_tree()), h as i64, vec![], 0, h);
        let ctx = build_context(self_box, vec![out0], h);
        assert_false(&tree, &ctx, "migration approval with active proposal should reject");
    }

    #[test]
    fn accept_migration_execute_after_timelock() {
        let tree = load_governance_tree();
        // v2.1: the example target is no longer the accumulation script, which
        // would hand the NFT to anyone (R4); a separate test covers that case.
        let new_contract = target_tree();
        let approval_h = 100i64;
        let exec_h = (approval_h + TIMELOCK_BLOCKS + 1) as u32;
        let value = 100 * NANOCOIN;

        let self_box = make_migration_box(&tree, value, &new_contract, approval_h, exec_h - 1);
        // Target with typed registers of the old layout, R5 = 0 and R8 = 0: accepted
        // (v2.1 reads only R5 and R8 of a target at another script).
        let out0 = make_governance_box_other_script(&new_contract, value - TX_FEE, vec![0u8; 32], 0i64, vec![], 0i64, 0i64, exec_h);
        let ctx = build_context(self_box, vec![out0, fee_box(TX_FEE, exec_h)], exec_h);
        assert_signed(&tree, &ctx, "migration execute should accept");
    }

    #[test]
    fn reject_migration_execute_without_flag() {
        let tree = load_governance_tree();
        let approval_h = 100i64;
        let exec_h = (approval_h + TIMELOCK_BLOCKS + 1) as u32;
        let value = 100 * NANOCOIN;

        // R8=0, not migration approved (R4 holds the target hash anyway)
        let self_box = make_governance_box(&tree, value, tree_hash(&target_tree()), approval_h, vec![], 0i64, 0i64, exec_h - 1);
        let out0 = make_box(&target_tree(), value - TX_FEE, vec![make_nft_token()], HashMap::new(), exec_h);
        let ctx = build_context(self_box, vec![out0], exec_h);
        assert_false(&tree, &ctx, "migration without flag should reject");
    }

    #[test]
    fn reject_migration_execute_before_timelock() {
        let tree = load_governance_tree();
        let approval_h = 100i64;
        let exec_h = (approval_h + TIMELOCK_BLOCKS) as u32; // exact boundary
        let value = 100 * NANOCOIN;

        let self_box = make_migration_box(&tree, value, &target_tree(), approval_h, exec_h - 1);
        let out0 = make_box(&target_tree(), value - TX_FEE, vec![make_nft_token()], HashMap::new(), exec_h);
        let ctx = build_context(self_box, vec![out0], exec_h);
        assert_false(&tree, &ctx, "migration before timelock should reject");
    }

    /// F-006: execute checks the target script hash recorded at approval.
    #[test]
    fn f006_migration_execute_checks_target_hash() {
        let tree = load_governance_tree();
        let a = 100i64;
        let h = (a + TIMELOCK_BLOCKS + 1) as u32;
        let s = make_migration_box(&tree, 100 * NANOCOIN, &target_tree(), a, h - 1);
        let wrong = make_box(&load_tree(DUMMY_TREE_HEX), 100 * NANOCOIN, vec![make_nft_token()], HashMap::new(), h);
        assert_false(&tree, &build_context(s.clone(), vec![wrong], h), "unreviewed target");
        let right = make_box(&target_tree(), 100 * NANOCOIN, vec![make_nft_token()], HashMap::new(), h);
        assert_signed(&tree, &build_context(s, vec![right], h), "recorded target");
    }

    /// F-006: execute puts a floor of SELF.value - 1,000,000 on the target.
    #[test]
    fn f006_migration_execute_value_floor() {
        let tree = load_governance_tree();
        let a = 100i64;
        let h = (a + TIMELOCK_BLOCKS + 1) as u32;
        let s = make_migration_box(&tree, 100 * NANOCOIN, &target_tree(), a, h - 1);
        let at_floor = make_box(&target_tree(), 100 * NANOCOIN - TX_FEE, vec![make_nft_token()], HashMap::new(), h);
        assert_signed(&tree, &build_context(s.clone(), vec![at_floor, fee_box(TX_FEE, h)], h), "at the floor");
        let thin = make_box(&target_tree(), 100 * NANOCOIN - TX_FEE - 1, vec![make_nft_token()], HashMap::new(), h);
        let rest = make_output_box(&load_tree(DUMMY_TREE_HEX), TX_FEE + 1, h);
        assert_false(&tree, &build_context(s, vec![thin, rest], h), "one nano below the floor");
    }

    /// F-006: approval must record a 32-byte target hash with R6 empty and R7 = 0.
    #[test]
    fn f006_migration_approval_records_target() {
        let tree = load_governance_tree();
        let h = 100u32;
        let s = make_idle_governance_box(&tree, 100 * NANOCOIN, h - 1);
        let short = approval_out(&tree, 100 * NANOCOIN, vec![1u8; 31], h as i64, vec![], 0, h);
        assert_false(&tree, &build_context(s.clone(), vec![short], h), "31-byte commitment");
        let with_payee = approval_out(&tree, 100 * NANOCOIN, tree_hash(&target_tree()), h as i64, recipient_bytes(), 0, h);
        assert_false(&tree, &build_context(s.clone(), vec![with_payee], h), "R6 must be empty");
        let with_amount = approval_out(&tree, 100 * NANOCOIN, tree_hash(&target_tree()), h as i64, vec![], 5, h);
        assert_false(&tree, &build_context(s.clone(), vec![with_amount], h), "R7 must be 0");
        // F-016 on the migration approval height.
        let old = approval_out(&tree, 100 * NANOCOIN, tree_hash(&target_tree()), h as i64 - 11, vec![], 0, h);
        assert_false(&tree, &build_context(s.clone(), vec![old], h), "height h-11");
        let late = approval_out(&tree, 100 * NANOCOIN, tree_hash(&target_tree()), h as i64 - 10, vec![], 0, h);
        assert_signed(&tree, &build_context(s, vec![late], h), "height h-10");
    }

    /// F-006: a migrate-to-self must arrive in the neutral state.
    #[test]
    fn f006_migrate_to_self_arrives_neutral() {
        let tree = load_governance_tree();
        let a = 100i64;
        let h = (a + TIMELOCK_BLOCKS + 1) as u32;
        let s = make_migration_box(&tree, 100 * NANOCOIN, &tree, a, h - 1);
        let neutral = successor(&s, 100 * NANOCOIN, 0, 0, h);
        assert_signed(&tree, &build_context(s.clone(), vec![neutral], h), "neutral arrival");
        // Same state with the same R5 and commitment is just a consolidation (accepted);
        // arriving with a new commitment and an old approval height is not.
        let same = successor(&s, 100 * NANOCOIN, a, 2, h);
        assert_signed(&tree, &build_context(s.clone(), vec![same], h), "unchanged state = consolidation");
        let armed = make_governance_box(&tree, 100 * NANOCOIN, tree_hash(&load_tree(DUMMY_TREE_HEX)), 1, vec![], 0, 2, h);
        assert_false(&tree, &build_context(s.clone(), vec![armed], h), "arrives armed for another target");
        let armed_pay = make_proposal_governance_box(&tree, 100 * NANOCOIN, recipient_bytes(), NANOCOIN as i64, 1, 0, h);
        assert_false(&tree, &build_context(s.clone(), vec![armed_pay], h), "arrives with a ready payment");
        // Same commitment, but re-armed with an old approval height: could migrate again at once.
        let rearmed = successor(&s, 100 * NANOCOIN, 1, 2, h);
        assert_false(&tree, &build_context(s.clone(), vec![rearmed], h), "arrives re-armed with R5 = 1");
        // Flag left at 2 with R5 = 0 (the state F-004 left behind).
        let half = successor(&s, 100 * NANOCOIN, 0, 2, h);
        assert_false(&tree, &build_context(s, vec![half], h), "arrives with R8 = 2, R5 = 0");
    }

    /// F-007: migration execute has a window of 20,160 blocks after the delay.
    #[test]
    fn f007_migration_window_closes() {
        let tree = load_governance_tree();
        let a = 100i64;
        let last = (a + TIMELOCK_BLOCKS + WINDOW_BLOCKS) as u32;
        let s = make_migration_box(&tree, 100 * NANOCOIN, &target_tree(), a, 200);
        let out = |h: u32| make_box(&target_tree(), 100 * NANOCOIN, vec![make_nft_token()], HashMap::new(), h);
        assert_signed(&tree, &build_context(s.clone(), vec![out(last)], last), "last block");
        assert_false(&tree, &build_context(s.clone(), vec![out(last + 1)], last + 1), "after the window");
        // After the window the approval can still be cancelled, then re-approved.
        let cancel = successor(&s, 100 * NANOCOIN, 0, 0, last + 5);
        assert_signed(&tree, &build_context(s, vec![cancel], last + 5), "cancel after expiry");
    }

    /// F-024: migration execute to another script reads only R5 and R8 of the
    /// target (v2.1, getOrElse(0)). Rewritten for v2.1: a target whose R5 is not
    /// a Long is now rejected (was accepted when no register was read).
    #[test]
    fn f024_migration_to_register_free_or_differently_typed_target() {
        let tree = load_governance_tree();
        let a = 100i64;
        let h = (a + TIMELOCK_BLOCKS + 1) as u32;
        let s = make_migration_box(&tree, 100 * NANOCOIN, &target_tree(), a, h - 1);
        let bare = make_box(&target_tree(), 100 * NANOCOIN, vec![make_nft_token()], HashMap::new(), h);
        assert_signed(&tree, &build_context(s.clone(), vec![bare], h), "register-free target, one output");
        // R4 of another type (Long) is not read: accepted.
        let odd_r4 = make_box(&target_tree(), 100 * NANOCOIN, vec![make_nft_token()],
            HashMap::from([(R::R4, Constant::from(7i64))]), h);
        assert_signed(&tree, &build_context(s.clone(), vec![odd_r4], h), "R4 of another type");
        // R5 of another type (Coll[Byte]) is read as Long. sigma-rust 0.28 does not
        // check the declared type and the comparison with 0L is FALSE here; the
        // Scala interpreter throws on the type mismatch. Both reject.
        let odd = make_box(&target_tree(), 100 * NANOCOIN, vec![make_nft_token()],
            HashMap::from([(R::R4, Constant::from(7i64)), (R::R5, Constant::from(vec![1u8, 2, 3]))]), h);
        assert_false(&tree, &build_context(s, vec![odd], h), "R5 of another type");
    }
}

#[cfg(test)]
mod phase8_accumulation {
    use super::*;

    /// Consolidation now sums every input at the script (F-002) and waits the cooldown (F-025).
    #[test]
    fn accept_accum_consolidation() {
        let tree = load_accumulation_tree();
        let h = MH; // past the 20,160-block cooldown (v2.1; was 10,000)
        let a = accum_box(10 * NANOCOIN, 100);
        let b = accum_box(5 * NANOCOIN, 200);
        let out0 = make_output_box(&tree, 15 * NANOCOIN, h);
        for i in 0..2 {
            let ctx = build_context_multi(vec![a.clone(), b.clone()], i, vec![out0.clone()], h);
            assert_true(&tree, &ctx, "accum consolidation should accept");
        }
    }

    /// Rewritten (F-001, F-003): the old layout (tokenless box at the governance
    /// script) is the vulnerable one. The honest forward is [NFT holder, accumulation
    /// boxes] -> [NFT holder successor carrying the sum].
    #[test]
    fn accept_accum_transfer_to_governance() {
        let accum_tree = load_accumulation_tree();
        let gov_tree = load_governance_tree();
        let h = 100u32;
        let gov = make_idle_governance_box(&gov_tree, 50 * NANOCOIN, h - 1);
        let acc = accum_box(10 * NANOCOIN, h - 1);
        let succ = make_idle_governance_box(&gov_tree, 60 * NANOCOIN, h);
        let ins = vec![gov, acc];
        let ctx = build_context_multi(ins.clone(), 1, vec![succ.clone()], h);
        assert_true(&accum_tree, &ctx, "forward into the NFT holder");
        let ctx = build_context_multi(ins, 0, vec![succ], h);
        assert_signed(&gov_tree, &ctx, "governance consolidation of the forward");
        // The old layout is now rejected.
        let old = build_context(accum_box(10 * NANOCOIN, h - 1), vec![make_output_box(&gov_tree, 10 * NANOCOIN, h)], h);
        assert_false(&accum_tree, &old, "tokenless box at the governance script");
    }

    #[test]
    fn reject_accum_wrong_destination() {
        let accum_tree = load_accumulation_tree();
        let wrong_tree = load_tree(DUMMY_TREE_HEX);
        let h = MH; // past the 20,160-block cooldown (v2.1; was 10,000)
        let self_box = accum_box(10 * NANOCOIN, 100);
        let out0 = make_output_box(&wrong_tree, 10 * NANOCOIN, h);
        let ctx = build_context(self_box, vec![out0], h);
        assert_false(&accum_tree, &ctx, "wrong destination should reject");
    }

    #[test]
    fn reject_accum_value_decrease() {
        let tree = load_accumulation_tree();
        let h = MH; // past the 20,160-block cooldown (v2.1; was 10,000)
        let self_box = accum_box(10 * NANOCOIN, 100);
        let out0 = make_output_box(&tree, 10 * NANOCOIN - 1, h);
        let ctx = build_context(self_box, vec![out0], h);
        assert_false(&tree, &ctx, "value decrease should reject");
    }

    /// F-001: a dust box at the governance script with the rest elsewhere.
    #[test]
    fn f001_reject_dust_forward() {
        let tree = load_accumulation_tree();
        let gov = load_governance_tree();
        let h = 10_000u32;
        let a = accum_box(100 * NANOCOIN, 100);
        let b = accum_box(100 * NANOCOIN, 100);
        let outs = vec![make_output_box(&gov, TX_FEE, h), make_output_box(&load_tree(DUMMY_TREE_HEX), 200 * NANOCOIN - TX_FEE, h)];
        for i in 0..2 {
            assert_false(&tree, &build_context_multi(vec![a.clone(), b.clone()], i, outs.clone(), h), "dust forward");
        }
        // Even with the real NFT holder at INPUTS(0), O0 must carry its value plus the sum.
        let g = make_idle_governance_box(&gov, 50 * NANOCOIN, 100);
        let short = make_idle_governance_box(&gov, 50 * NANOCOIN + 200 * NANOCOIN - 1, h);
        let ctx = build_context_multi(vec![g.clone(), a.clone(), b.clone()], 1, vec![short], h);
        assert_false(&tree, &ctx, "successor one nano short of old value + sum");
        let exact = make_idle_governance_box(&gov, 250 * NANOCOIN, h);
        let ctx = build_context_multi(vec![g, a, b], 2, vec![exact], h);
        assert_true(&tree, &ctx, "successor carrying old value + sum");
    }

    /// F-001: INPUTS(0) must hold the NFT, and so must O0.
    #[test]
    fn f001_reject_transfer_without_nft_holder() {
        let tree = load_accumulation_tree();
        let gov = load_governance_tree();
        let h = 10_000u32;
        let acc = accum_box(10 * NANOCOIN, 100);
        // NFT holder is not INPUTS(0).
        let g = make_idle_governance_box(&gov, 50 * NANOCOIN, 100);
        let succ = make_idle_governance_box(&gov, 60 * NANOCOIN, h);
        let ctx = build_context_multi(vec![acc.clone(), g.clone()], 0, vec![succ], h);
        assert_false(&tree, &ctx, "NFT holder at INPUTS(1)");
        // O0 lacks the NFT (the NFT went to OUTPUTS(1)).
        let o0 = make_box(&gov, 60 * NANOCOIN, vec![], gov_regs(vec![0; 32], 0, vec![], 0, 0), h);
        let o1 = make_box(&gov, NANOCOIN, vec![make_nft_token()], gov_regs(vec![0; 32], 0, vec![], 0, 0), h);
        let ctx = build_context_multi(vec![g, acc], 1, vec![o0, o1], h);
        assert_false(&tree, &ctx, "O0 without the NFT");
    }

    /// F-002: merge n boxes, return the largest, keep the rest.
    #[test]
    fn f002_reject_merge_returning_largest_input() {
        let tree = load_accumulation_tree();
        let h = MH; // past the 20,160-block cooldown (v2.1; was 10,000)
        let ins = vec![accum_box(100 * NANOCOIN, 100), accum_box(60 * NANOCOIN, 100), accum_box(40 * NANOCOIN, 100)];
        let outs = vec![make_output_box(&tree, 100 * NANOCOIN, h), make_output_box(&load_tree(DUMMY_TREE_HEX), 100 * NANOCOIN, h)];
        for i in 0..3 {
            assert_false(&tree, &build_context_multi(ins.clone(), i, outs.clone(), h), "output = largest input");
        }
        let one_short = vec![make_output_box(&tree, 200 * NANOCOIN - 1, h)];
        assert_false(&tree, &build_context_multi(ins.clone(), 0, one_short, h), "sum minus one");
        let honest = vec![make_output_box(&tree, 200 * NANOCOIN, h)];
        for i in 0..3 {
            assert_true(&tree, &build_context_multi(ins.clone(), i, honest.clone(), h), "honest merge of the sum");
        }
    }

    /// F-003: forwarding into a tokenless box at the governance script is rejected;
    /// a tokenless box at the governance script cannot be spent on any path either.
    #[test]
    fn f003_tokenless_governance_box() {
        let tree = load_accumulation_tree();
        let gov = load_governance_tree();
        let h = 20_000u32;
        let acc = accum_box(10 * NANOCOIN, 100);
        let bare_out = make_output_box(&gov, 10 * NANOCOIN, h);
        assert_false(&tree, &build_context(acc, vec![bare_out], h), "forward into a tokenless box");
        let regs_only = make_box(&gov, 10 * NANOCOIN, vec![], gov_regs(vec![0; 32], 0, vec![], 0, 0), h - 1);
        let succ = make_box(&gov, 10 * NANOCOIN, vec![], gov_regs(vec![0; 32], 0, vec![], 0, 0), h);
        assert_false(&gov, &build_context(regs_only, vec![succ.clone()], h), "tokenless box with typed registers");
        // Without registers the SELF register reads fail: an evaluation error, also a reject.
        let bare = make_output_box(&gov, 10 * NANOCOIN, h - 1);
        assert_error(&gov, &build_context(bare, vec![succ], h), "tokenless box without registers");
    }

    /// F-019: co-spent accumulation value must reach the governance successor.
    #[test]
    fn f019_reject_co_spend_skim() {
        let ta = load_accumulation_tree();
        let tg = load_governance_tree();
        let h = 20_000u32;
        let gb = make_idle_governance_box(&tg, 500 * NANOCOIN, 100);
        let acc = accum_box(100 * NANOCOIN, 100);
        let ins = vec![gb.clone(), acc];
        let outs = vec![make_idle_governance_box(&tg, 500 * NANOCOIN, h), make_output_box(&load_tree(DUMMY_TREE_HEX), 100 * NANOCOIN, h)];
        // The governance side alone still accepts its consolidation ...
        assert_signed(&tg, &build_context_multi(ins.clone(), 0, outs.clone(), h), "governance side");
        // ... but the accumulation box now refuses, so the transaction is invalid.
        assert_false(&ta, &build_context_multi(ins.clone(), 1, outs, h), "accumulation side");
        let honest = vec![make_idle_governance_box(&tg, 600 * NANOCOIN, h)];
        assert_true(&ta, &build_context_multi(ins, 1, honest, h), "successor carries the sum");
    }

    /// F-023: a hand-built box at the governance script with another token and an old
    /// approval height can neither receive a forward nor execute.
    #[test]
    fn f023_lookalike_governance_box() {
        let ta = load_accumulation_tree();
        let tg = load_governance_tree();
        let amount = 150 * NANOCOIN as i64;
        let fake_nft = token([42u8; 32], 1);
        let look = |r5: i64, v: u64, h: u32| make_box(&tg, v, vec![fake_nft.clone()],
            gov_regs(compute_proposal_hash(&recipient_bytes(), amount), r5, recipient_bytes(), amount, 0), h);
        let a = accum_box(100 * NANOCOIN, 100);
        let b = accum_box(100 * NANOCOIN, 100);
        for i in 0..2 {
            let ctx = build_context_multi(vec![a.clone(), b.clone()], i, vec![look(1, 200 * NANOCOIN, 10_000)], 10_000);
            assert_false(&ta, &ctx, "forward into the lookalike");
        }
        let succ = make_box(&tg, 50 * NANOCOIN - TX_FEE, vec![fake_nft.clone()],
            gov_regs(compute_proposal_hash(&recipient_bytes(), amount), 0, recipient_bytes(), amount, 0), 10_001);
        let outs = vec![succ, make_output_box(&recipient_tree(), 150 * NANOCOIN, 10_001), fee_box(TX_FEE, 10_001)];
        let ctx = build_context(look(1, 200 * NANOCOIN, 10_000), outs, 10_001);
        assert_false(&tg, &ctx, "execution on the lookalike");
    }

    /// F-014: SELF must hold the constant NFT id with amount 1.
    #[test]
    fn f014_self_must_hold_constant_nft() {
        let tg = load_governance_tree();
        let h = 300u32;
        for (what, t) in [("other id", token([0x42; 32], 1)), ("amount 2", token(TREASURY_NFT, 2))] {
            let s = make_box(&tg, 10 * NANOCOIN, vec![t.clone()], gov_regs(vec![0; 32], 0, vec![], 0, 0), h - 1);
            let o = make_box(&tg, 10 * NANOCOIN, vec![t], gov_regs(vec![0; 32], 0, vec![], 0, 0), h);
            assert_false(&tg, &build_context(s, vec![o], h), what);
        }
        // Lookalike SELF spent next to the real fund box, O0 holding the real NFT:
        // only the SELF check stops the lookalike's own script.
        let look = make_box(&tg, 10 * NANOCOIN, vec![token([0x42; 32], 1)], gov_regs(vec![0; 32], 0, vec![], 0, 0), h - 1);
        let real = make_idle_governance_box(&tg, 10 * NANOCOIN, h - 1);
        let o = make_idle_governance_box(&tg, 20 * NANOCOIN, h);
        assert_false(&tg, &build_context_multi(vec![look, real.clone()], 0, vec![o.clone()], h), "lookalike next to the real box");
        let two = make_box(&tg, 10 * NANOCOIN, vec![token(TREASURY_NFT, 2)], gov_regs(vec![0; 32], 0, vec![], 0, 0), h - 1);
        assert_false(&tg, &build_context_multi(vec![two, real], 0, vec![o], h), "SELF with 2 units, O0 with 1");
        // NFT present but not at tokens(0).
        let s = make_box(&tg, 10 * NANOCOIN, vec![token([0x42; 32], 5), make_nft_token()], gov_regs(vec![0; 32], 0, vec![], 0, 0), h - 1);
        let o = make_box(&tg, 10 * NANOCOIN, vec![make_nft_token()], gov_regs(vec![0; 32], 0, vec![], 0, 0), h);
        assert_false(&tg, &build_context(s, vec![o], h), "NFT not at tokens(0)");
    }

    /// F-008: the stream follows the NFT. After a migration the holder is at another
    /// script and still receives forwards; a fresh box at the old script does not.
    #[test]
    fn f008_forward_follows_nft_after_migration() {
        let ta = load_accumulation_tree();
        let h = 10_000u32;
        let new_home = load_tree(DUMMY_TREE_HEX);
        let holder = make_box(&new_home, 80 * NANOCOIN, vec![make_nft_token()], HashMap::new(), 100);
        let acc = accum_box(10 * NANOCOIN, 100);
        let succ = make_box(&new_home, 90 * NANOCOIN, vec![make_nft_token()], HashMap::new(), h);
        assert_true(&ta, &build_context_multi(vec![holder, acc.clone()], 1, vec![succ], h), "forward to migrated holder");
        let tg = load_governance_tree();
        let fresh = make_box(&tg, 10 * NANOCOIN, vec![token([0x77; 32], 1)], gov_regs(vec![0; 32], 0, vec![], 0, 0), h);
        assert_false(&ta, &build_context(acc, vec![fresh], h), "fresh token at the old script");
    }

    /// F-025: a box merged 20,160 blocks ago or less cannot be merged again (v2.1,
    /// was 720), and the merged box must be created within 10 blocks of HEIGHT.
    #[test]
    fn f025_consolidation_cooldown() {
        let tree = load_accumulation_tree();
        let h = MH; // v2.1: cooldown 20,160 (was 720 at h = 10,000)
        let out = |ch: u32| vec![make_output_box(&tree, 15 * NANOCOIN, ch)];
        let old = accum_box(10 * NANOCOIN, h - COOLDOWN - 1);
        let other = accum_box(5 * NANOCOIN, 100);
        assert_true(&tree, &build_context_multi(vec![old.clone(), other.clone()], 0, out(h), h), "20,161 blocks old");
        let young = accum_box(10 * NANOCOIN, h - COOLDOWN);
        // Each input checks its own age: the young input is FALSE, so the transaction
        // fails even though the older co-input on its own reduces to TRUE.
        assert_false(&tree, &build_context_multi(vec![young.clone(), other.clone()], 0, out(h), h), "20,160 blocks old");
        assert_true(&tree, &build_context_multi(vec![young.clone(), other.clone()], 1, out(h), h), "older co-input");
        assert_true(&tree, &build_context_multi(vec![old.clone(), other.clone()], 0, out(h - 10), h), "creation h-10");
        assert_false(&tree, &build_context_multi(vec![old.clone(), other.clone()], 0, out(h - 11), h), "creation h-11");
        assert_false(&tree, &build_context_multi(vec![old.clone(), other.clone()], 0, out(h + 1), h), "creation h+1");
        // The transfer branch has no cooldown: signers forward a fresh box at once.
        let gov = load_governance_tree();
        let g = make_idle_governance_box(&gov, 50 * NANOCOIN, h - 1);
        let fresh = accum_box(10 * NANOCOIN, h - 1);
        let succ = make_idle_governance_box(&gov, 60 * NANOCOIN, h);
        assert_true(&tree, &build_context_multi(vec![g, fresh], 1, vec![succ], h), "forward a fresh box");
    }

    /// Merged boxes carry no tokens and no registers; a token-bearing accumulation
    /// box can be forwarded but not merged.
    #[test]
    fn consolidation_output_is_plain() {
        let tree = load_accumulation_tree();
        let h = MH; // past the 20,160-block cooldown (v2.1; was 10,000)
        let a = accum_box(10 * NANOCOIN, 100);
        let padded = make_box(&tree, 10 * NANOCOIN, vec![], HashMap::from([(R::R4, Constant::from(vec![0u8; 100]))]), h);
        assert_false(&tree, &build_context(a.clone(), vec![padded], h), "registers on the merged box");
        let tok = make_box(&tree, 10 * NANOCOIN, vec![token([0x55; 32], 9)], HashMap::new(), h);
        assert_false(&tree, &build_context(a, vec![tok], h), "token on the merged box");
        let with_tok = make_box(&tree, 10 * NANOCOIN, vec![token([0x55; 32], 9)], HashMap::new(), 100);
        assert_false(&tree, &build_context(with_tok, vec![make_output_box(&tree, 10 * NANOCOIN, h)], h), "token-bearing input");
    }
}

#[cfg(test)]
mod phase9_integration {
    use super::*;

    #[test]
    fn reject_double_execution() {
        let tree = load_governance_tree();
        let exec_h = 20000u32;
        let value = 200 * NANOCOIN; // above the floor after paying (v2.1; was 80)
        let amount = 10i64 * NANOCOIN as i64;

        let self_box = make_idle_governance_box(&tree, value, exec_h - 1);
        let out0 = successor(&self_box, value - amount as u64 - TX_FEE, 0, 0, exec_h);
        let out1 = make_output_box(&recipient_tree(), amount as u64, exec_h);
        let ctx = build_context(self_box, vec![out0, out1, fee_box(TX_FEE, exec_h)], exec_h);
        assert_false(&tree, &ctx, "double execution should reject");
    }

    #[test]
    fn accept_approval_after_cancellation() {
        let tree = load_governance_tree();
        let h = 300u32;
        let value = 100 * NANOCOIN;
        let rb = recipient_bytes();
        let amount = 5i64 * NANOCOIN as i64;
        // Box after a cancellation: R5 = 0, earlier R4/R6/R7 still present.
        let self_box = make_proposal_governance_box(&tree, value, rb.clone(), 3 * NANOCOIN as i64, 0, 0, h - 1);
        let out0 = make_governance_box(&tree, value, compute_proposal_hash(&rb, amount), h as i64, rb, amount, 0i64, h);
        let ctx = build_context(self_box, vec![out0], h);
        assert_signed(&tree, &ctx, "approval after cancel should accept");
    }

    #[test]
    fn reject_proposal_replacement_without_cancel() {
        let tree = load_governance_tree();
        let h = 200u32;
        let value = 100 * NANOCOIN;
        let rb = recipient_bytes();
        let amount = 10i64 * NANOCOIN as i64;
        let proposal_hash = compute_proposal_hash(&rb, amount);

        let self_box = make_proposal_governance_box(&tree, value, rb.clone(), amount, 50, 0, h - 1);
        let out0 = make_governance_box(&tree, value, proposal_hash, h as i64, rb, amount, 0i64, h);
        let ctx = build_context(self_box, vec![out0], h);
        assert_false(&tree, &ctx, "replacement without cancel should reject");
    }

    #[test]
    fn consolidation_preserves_active_proposal() {
        let tree = load_governance_tree();
        let h = 100u32;
        let value = 50 * NANOCOIN;
        let rb = recipient_bytes();
        let amount = 10i64 * NANOCOIN as i64;
        let approval_h = 50i64;

        let self_box = make_proposal_governance_box(&tree, value, rb.clone(), amount, approval_h, 0, h - 1);
        let proposal_hash = compute_proposal_hash(&rb, amount);
        let out0 = make_governance_box(&tree, value + 10 * NANOCOIN, proposal_hash, approval_h, rb, amount, 0i64, h);
        let ctx = build_context(self_box, vec![out0], h);
        assert_signed(&tree, &ctx, "consolidation with proposal should accept");
    }

    #[test]
    fn full_migration_flow() {
        // Step 1: Migration approval (R8: 0 -> 2, R5 = HEIGHT, R4 = target hash)
        let tree = load_governance_tree();
        let h1 = 100u32;
        let value = 100 * NANOCOIN;
        let target = target_tree();

        let self1 = make_idle_governance_box(&tree, value, h1 - 1);
        let out1 = make_migration_box(&tree, value, &target, h1 as i64, h1);
        let ctx1 = build_context(self1, vec![out1.clone()], h1);
        assert_signed(&tree, &ctx1, "migration approval step should accept");

        // Step 2: Migration execute after timelock, to a register-free target.
        let h2 = (h1 as i64 + TIMELOCK_BLOCKS + 1) as u32;
        let out2 = make_box(&target, value - TX_FEE, vec![make_nft_token()], HashMap::new(), h2);
        let ctx2 = build_context(out1, vec![out2, fee_box(TX_FEE, h2)], h2);
        assert_signed(&tree, &ctx2, "migration execute step should accept");
    }

    /// Full payment cycle with the F-016 range and F-021 fee output.
    #[test]
    fn full_payment_flow() {
        let tree = load_governance_tree();
        let rb = recipient_bytes();
        let amount = 7 * NANOCOIN as i64;
        let h1 = 1_000u32;
        // 200 YOLO (v2.1, was 100) so the change stays above the fund floor.
        let idle = make_idle_governance_box(&tree, 200 * NANOCOIN, h1 - 1);
        let approved = make_governance_box(&tree, 200 * NANOCOIN, compute_proposal_hash(&rb, amount), h1 as i64 - 4, rb, amount, 0, h1);
        assert_signed(&tree, &build_context(idle, vec![approved.clone()], h1), "approval");
        let h2 = (h1 as i64 - 4 + TIMELOCK_BLOCKS + 1) as u32;
        let change = successor(&approved, 200 * NANOCOIN - amount as u64 - TX_FEE, 0, 0, h2);
        assert_eq!(reg_i64(&change, R::R5), 0);
        let outs = vec![change, make_output_box(&recipient_tree(), amount as u64, h2), fee_box(TX_FEE, h2)];
        assert_signed(&tree, &build_context(approved, outs, h2), "execution");
    }
}

// ============================================================
// v2.1 (2026-11 retest): FUND FLOOR, FRESH SUCCESSOR, NEUTRAL ARRIVAL,
// ACCUMULATION HOLDER PIN
// ============================================================
//
// Every reject below is asserted FALSE (assert_false checks Outcome::False,
// so an evaluation error would fail the test) and has an accepting twin.

#[cfg(test)]
mod phase10_v21 {
    use super::*;

    /// A plain P2PK box that pays the fee or tops up an execution.
    fn funding_input(value: u64, ch: u32) -> ErgoBox { make_output_box(&recipient_tree(), value, ch) }

    /// R1: execution may not leave the fund below fundFloor. SELF 150 YOLO pays an
    /// approved 50 YOLO; the execution bound alone (150 - 50 - fee) would allow
    /// 99.999 YOLO, the floor needs 100 YOLO. The fee comes from a separate input.
    #[test]
    fn v21_execution_floor() {
        let tree = load_governance_tree();
        let a = 100i64;
        let h = (a + TIMELOCK_BLOCKS + 1) as u32;
        let amount = 50 * NANOCOIN;
        let s = make_proposal_governance_box(&tree, 150 * NANOCOIN, recipient_bytes(), amount as i64, a, 0, h - 1);
        let ins = vec![s.clone(), funding_input(TX_FEE, h - 1)];
        let at = vec![successor(&s, FUND_FLOOR, 0, 0, h), make_output_box(&recipient_tree(), amount, h), fee_box(TX_FEE, h)];
        assert_signed(&tree, &build_context_multi(ins.clone(), 0, at, h), "change exactly at the floor");
        let below = vec![successor(&s, FUND_FLOOR - 1, 0, 0, h), make_output_box(&recipient_tree(), amount + 1, h), fee_box(TX_FEE, h)];
        assert_false(&tree, &build_context_multi(ins, 0, below, h), "change one nano below the floor");
    }

    /// R1: a box already below the floor may keep its value but not drop.
    #[test]
    fn v21_below_floor_may_keep_not_drop() {
        let tree = load_governance_tree();
        let a = 100i64;
        let h = (a + TIMELOCK_BLOCKS + 1) as u32;
        let amount = 10 * NANOCOIN;
        let s = make_proposal_governance_box(&tree, 50 * NANOCOIN, recipient_bytes(), amount as i64, a, 0, h - 1);
        // The payment and fee come from another input.
        let ins = vec![s.clone(), funding_input(amount + TX_FEE, h - 1)];
        let keep = vec![successor(&s, 50 * NANOCOIN, 0, 0, h), make_output_box(&recipient_tree(), amount, h), fee_box(TX_FEE, h)];
        assert_signed(&tree, &build_context_multi(ins.clone(), 0, keep, h), "keeps its value");
        let drop = vec![successor(&s, 50 * NANOCOIN - 1, 0, 0, h), make_output_box(&recipient_tree(), amount + 1, h), fee_box(TX_FEE, h)];
        assert_false(&tree, &build_context_multi(ins, 0, drop, h), "drops one nano");
        // Consolidation of a box below the floor: unchanged value is accepted.
        let idle = make_idle_governance_box(&tree, 50 * NANOCOIN, h - 1);
        let same = make_idle_governance_box(&tree, 50 * NANOCOIN, h);
        assert_signed(&tree, &build_context(idle, vec![same], h), "consolidation keeping the value");
    }

    /// R1: the successor at this script must be created at HEIGHT - 720 or later
    /// (v2.2, N-1; was HEIGHT - 10), so each signed spend restarts the storage
    /// rent period and a signed transaction stays valid for 720 blocks.
    /// Rewritten for v2.2: the h-10 / h-11 cases are now h-720 / h-721.
    #[test]
    fn v21_successor_creation_height() {
        let tree = load_governance_tree();
        let h = 20_000u32;
        // Consolidation.
        let idle = make_idle_governance_box(&tree, 50 * NANOCOIN, 100);
        let ok = make_idle_governance_box(&tree, 60 * NANOCOIN, h - SIGN_WINDOW);
        assert_signed(&tree, &build_context(idle.clone(), vec![ok], h), "consolidation, creation h-720");
        let old = make_idle_governance_box(&tree, 60 * NANOCOIN, h - SIGN_WINDOW - 1);
        assert_false(&tree, &build_context(idle, vec![old], h), "consolidation, creation h-721");
        // Execution.
        let a = 100i64;
        let h = (a + TIMELOCK_BLOCKS + 1) as u32;
        let (tree, s, outs) = execution_parts(a, h);
        let mut o = outs.clone();
        o[0] = successor(&outs[0], val(&outs[0]), 0, 0, h - SIGN_WINDOW);
        assert_signed(&tree, &build_context(s.clone(), o, h), "execution, creation h-720");
        let mut o = outs.clone();
        o[0] = successor(&outs[0], val(&outs[0]), 0, 0, h - SIGN_WINDOW - 1);
        assert_false(&tree, &build_context(s, o, h), "execution, creation h-721");
        // Freeze (the emergency control): same window.
        let h = 20_000u32;
        let idle = make_idle_governance_box(&tree, 50 * NANOCOIN, 100);
        let frozen = |ch: u32| successor(&idle, 50 * NANOCOIN, 0, 1, ch);
        assert_signed(&tree, &build_context(idle.clone(), vec![frozen(h - SIGN_WINDOW)], h), "freeze, creation h-720");
        assert_false(&tree, &build_context(idle.clone(), vec![frozen(h - SIGN_WINDOW - 1)], h), "freeze, creation h-721");
    }

    /// R2: a migration to another script must arrive with nothing approved.
    /// The target has the governance register layout (a same-design contract).
    #[test]
    fn v21_migration_to_other_script_arrives_neutral() {
        let tree = load_governance_tree();
        let a = 100i64;
        let h = (a + TIMELOCK_BLOCKS + 1) as u32;
        let s = make_migration_box(&tree, 100 * NANOCOIN, &target_tree(), a, h - 1);
        let t = target_tree();
        let arrive = |r4: Vec<u8>, r5: i64, r6: Vec<u8>, r7: i64, r8: i64| {
            make_box(&t, 100 * NANOCOIN, vec![make_nft_token()], gov_regs(r4, r5, r6, r7, r8), h)
        };
        let pay = 99 * NANOCOIN as i64;
        let armed_pay = arrive(compute_proposal_hash(&recipient_bytes(), pay), h as i64 - 8_642, recipient_bytes(), pay, 0);
        assert_false(&tree, &build_context(s.clone(), vec![armed_pay], h), "arrives with an old approval height");
        let armed_mig = arrive(tree_hash(&load_tree(DUMMY_TREE_HEX)), 0, vec![], 0, 2);
        assert_false(&tree, &build_context(s.clone(), vec![armed_mig], h), "arrives with flag 2");
        let both = arrive(tree_hash(&load_tree(DUMMY_TREE_HEX)), h as i64 - 8_642, vec![], 0, 2);
        assert_false(&tree, &build_context(s.clone(), vec![both], h), "arrives armed for a second migration");
        let frozen = arrive(vec![0u8; 32], 0, vec![], 0, 1);
        assert_false(&tree, &build_context(s.clone(), vec![frozen], h), "arrives with flag 1");
        let neutral = arrive(vec![0u8; 32], 0, vec![], 0, 0);
        assert_signed(&tree, &build_context(s.clone(), vec![neutral], h), "R5 = 0 and R8 = 0");
        let bare = make_box(&t, 100 * NANOCOIN, vec![make_nft_token()], HashMap::new(), h);
        assert_signed(&tree, &build_context(s, vec![bare], h), "register-free target");
    }

    /// R4: a fund migrated to the accumulation script by mistake cannot be taken:
    /// a box at the accumulation script holding the NFT is not a holder.
    #[test]
    fn v21_nft_at_accumulation_script_is_not_a_holder() {
        let ta = load_accumulation_tree();
        let h = 20_000u32;
        let v = 100 * NANOCOIN;
        let stuck = make_box(&ta, v, vec![make_nft_token()], HashMap::new(), 15_000);
        let own = make_output_box(&load_tree(DUMMY_TREE_HEX), v, 100);
        let taken = make_box(&load_tree(DUMMY_TREE_HEX), 2 * v, vec![make_nft_token()], HashMap::new(), h);
        let ctx = build_context_multi(vec![stuck, own], 0, vec![taken], h);
        assert_false(&ta, &ctx, "NFT box at the accumulation script as INPUTS(0)");
        // Twin: the same shape with the holder at another script is a forward.
        let holder = make_box(&load_tree(DUMMY_TREE_HEX), v, vec![make_nft_token()], HashMap::new(), 100);
        let succ = make_box(&load_tree(DUMMY_TREE_HEX), 2 * v, vec![make_nft_token()], HashMap::new(), h);
        let ctx = build_context_multi(vec![holder, accum_box(v, 100)], 1, vec![succ], h);
        assert_true(&ta, &ctx, "holder at another script");
    }

    /// R5: the holder must carry the NFT at tokens(0).
    #[test]
    fn v21_holder_nft_must_be_at_tokens0() {
        let ta = load_accumulation_tree();
        let h = 20_000u32;
        let home = load_tree(DUMMY_TREE_HEX);
        let other = token([0x42; 32], 5);
        let acc = accum_box(10 * NANOCOIN, 100);
        let succ = make_box(&home, 90 * NANOCOIN, vec![make_nft_token()], HashMap::new(), h);
        let at1 = make_box(&home, 80 * NANOCOIN, vec![other.clone(), make_nft_token()], HashMap::new(), 100);
        assert_false(&ta, &build_context_multi(vec![at1, acc.clone()], 1, vec![succ.clone()], h), "NFT at tokens(1) of INPUTS(0)");
        let at0 = make_box(&home, 80 * NANOCOIN, vec![make_nft_token(), other.clone()], HashMap::new(), 100);
        assert_true(&ta, &build_context_multi(vec![at0.clone(), acc.clone()], 1, vec![succ], h), "NFT at tokens(0) of INPUTS(0)");
        let succ1 = make_box(&home, 90 * NANOCOIN, vec![other, make_nft_token()], HashMap::new(), h);
        assert_false(&ta, &build_context_multi(vec![at0, acc], 1, vec![succ1], h), "NFT at tokens(1) of O0");
    }

    /// R9 (F-023 variant): a lookalike box (another token at tokens(0), an old R5
    /// and a valid commitment) spent next to the real fund box, executing into an
    /// O0 that holds the real NFT. Only the constant-NFT check on SELF stops it.
    #[test]
    fn v21_f023_lookalike_execution_beside_real_box() {
        let tg = load_governance_tree();
        let a = 100i64;
        let h = (a + TIMELOCK_BLOCKS + 1) as u32;
        let amount = 50 * NANOCOIN;
        let fake = token([42u8; 32], 1);
        let regs = |r5: i64| gov_regs(compute_proposal_hash(&recipient_bytes(), amount as i64), r5, recipient_bytes(), amount as i64, 0);
        let look = make_box(&tg, 200 * NANOCOIN, vec![fake], regs(a), h - 1);
        let real = make_idle_governance_box(&tg, 200 * NANOCOIN, h - 1);
        let o0 = make_box(&tg, 150 * NANOCOIN - TX_FEE, vec![make_nft_token()], regs(0), h);
        let outs = vec![o0, make_output_box(&recipient_tree(), amount, h), fee_box(TX_FEE, h)];
        assert_false(&tg, &build_context_multi(vec![look.clone(), real.clone()], 0, outs.clone(), h), "lookalike executes");
        // Twin: the same transaction from a SELF that holds the real NFT is SIGNED.
        let genuine = make_box(&tg, 200 * NANOCOIN, vec![make_nft_token()], regs(a), h - 1);
        assert_signed(&tg, &build_context_multi(vec![genuine, real], 0, outs, h), "genuine SELF");
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
mod phase11_v22 {
    use super::*;

    /// EA-N1 skim with a second tokenless input at the holder's script: FALSE.
    /// The honest twin (successor = sum of all three) is TRUE, one nano short FALSE.
    #[test]
    fn v22_ea_n1_skim_with_stray_at_holder_script_rejected() {
        let ta = load_accumulation_tree();
        let tg = load_governance_tree();
        let h = 20_000u32;
        let t = make_idle_governance_box(&tg, 100 * NANOCOIN, 100);
        let d = make_output_box(&tg, 50 * NANOCOIN, 100); // tokenless, at the holder's script
        let a = accum_box(50 * NANOCOIN, 100);
        let ins = vec![t, d, a];
        let attacker = make_output_box(&load_tree(DUMMY_TREE_HEX), 50 * NANOCOIN, h);
        let skim = vec![make_idle_governance_box(&tg, 150 * NANOCOIN, h), attacker];
        assert_false(&ta, &build_context_multi(ins.clone(), 2, skim, h), "T' = T + max(d, a), attacker takes min(d, a)");
        let honest = vec![make_idle_governance_box(&tg, 200 * NANOCOIN, h)];
        assert_true(&ta, &build_context_multi(ins.clone(), 2, honest, h), "holder' = holder + stray + acc");
        let short = vec![make_idle_governance_box(&tg, 200 * NANOCOIN - 1, h)];
        assert_false(&ta, &build_context_multi(ins, 2, short, h), "one nano short of the sum of all three");
    }

    /// The ordinary forward into the multisig is unchanged: [G, acc 10 + 5] ->
    /// [G' = G + 15] is TRUE for both accumulation boxes and SIGNED for G;
    /// one nano short is FALSE.
    #[test]
    fn v22_ordinary_forward_into_multisig_unchanged() {
        let ta = load_accumulation_tree();
        let tg = load_governance_tree();
        let h = 20_000u32;
        let g = make_idle_governance_box(&tg, 100 * NANOCOIN, 100);
        let ins = vec![g, accum_box(10 * NANOCOIN, 100), accum_box(5 * NANOCOIN, 200)];
        let succ = vec![make_idle_governance_box(&tg, 115 * NANOCOIN, h)];
        for i in 1..3 {
            assert_true(&ta, &build_context_multi(ins.clone(), i, succ.clone(), h), "accumulation side of the forward");
        }
        assert_signed(&tg, &build_context_multi(ins.clone(), 0, succ, h), "multisig consolidation side");
        let short = vec![make_idle_governance_box(&tg, 115 * NANOCOIN - 1, h)];
        assert_false(&ta, &build_context_multi(ins, 1, short, h), "one nano short");
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
        // The constant alone proves nothing: check the compiled tree carries Long 8640 and
        // Long 20160, not 12960.
        assert!(GOVERNANCE_TREE_HEX.contains("05808701"), "tree lacks Long 8640");
        assert!(GOVERNANCE_TREE_HEX.contains("0580bb02"), "tree lacks Long 20160");
        assert!(!GOVERNANCE_TREE_HEX.contains("05c0ca01"), "tree contains Long 12960");
    }

    /// v2.0: the accumulation script names its destination by the treasury NFT id,
    /// not by a governance script hash, so it no longer changes when the governance
    /// script does (report, hash re-pin note).
    #[test]
    fn accumulation_embeds_current_governance_hash() {
        let gov_hash = hex_of(&blake(&hex_to_bytes(GOVERNANCE_TREE_HEX)));
        assert!(!ACCUMULATION_TREE_HEX.contains(&gov_hash), "accumulation must not embed the governance hash");
        let nft = format!("0e20{}", hex_of(&TREASURY_NFT));
        assert!(ACCUMULATION_TREE_HEX.contains(&nft), "accumulation must embed the treasury NFT id");
        assert!(GOVERNANCE_TREE_HEX.contains(&nft), "governance must embed the treasury NFT id");
        assert!(GOVERNANCE_TREE_HEX.contains(MINER_FEE_HEX), "governance must embed the fee script");
    }
}
