//! integration_test.rs: Coinbase integration tests proving emission, treasury,
//! and LP fund contracts work together in a single coinbase transaction.
//!
//! Uses pre-compiled ErgoTree bytes from the individual test suites:
//!   - emission.es v1.3         (from 01-emission-tests)
//!   - treasury_accumulation.es (from 02-treasury-tests)
//!   - lp_accumulation.es       (from 03-lp-fund-tests)
//!   - treasury_governance.es   (from 02-treasury-tests, for test 3b)
//!   - lp_fund.es               (from 03-lp-fund-tests, for test 4b)
//!
//! No node needed. The trees are the files compile/gen_trees.py writes from a
//! fresh offline compile of the sources; `gen_trees.py --check` confirms them.
//! Targets: ergo-lib 0.28, rustc 1.85+

use std::convert::TryInto;

use ergo_lib::ergo_chain_types::{Digest32, EcPoint, Header, PreHeader};
use ergo_lib::ergotree_interpreter::{
    eval::{context::Context, reduce_to_crypto},
    sigma_protocol::{
        private_input::{DlogProverInput, PrivateInput},
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
    mir::constant::Constant,
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
const MAX_HALVINGS: u32 = 20; // chain years in the schedule total
/// Total supply: the 20-year schedule total.
const TOTAL_SUPPLY: u64 = 177_291_450_000_000_000;
/// Genesis treasury and LP fund boxes, 100 YOLO each (2026-11-12), taken
/// out of the emission box.
const GENESIS_FUND_BOX: u64 = 100 * NANOCOIN;
/// Genesis emission box (2026-11-12): TOTAL_SUPPLY - 2 x GENESIS_FUND_BOX.
const GENESIS_EMISSION_BOX: u64 = 177_291_250_000_000_000;
/// Real chain: last normal spend (box 2.5 YOLO) and terminal spend (box
/// 1.0 YOLO, below the 1.5 reward).
const REAL_LAST_NORMAL: u32 = 21_023_915;
const REAL_TERMINAL: u32 = 21_023_916;
const TREASURY_PCT: u64 = 4;
const LP_PCT: u64 = 3;

// ============================================================
// PRE-COMPILED ERGOTREE HEX CONSTANTS
// ============================================================

// Trees of the five contracts, written by compile/gen_trees.py from the .es
// sources in 01-, 02- and 03- (test NFT ids: emission cc..cc, treasury aa..aa,
// LP bb..bb). `python3 compile/gen_trees.py --check` recompiles all five and compares.
//
// Emission v1.4 (702 bytes): split 93 / 4 / 3 (four constants changed from
// v1.3, blake2b256 72393fc5...0b25; same length and template).
// v1.3: constant NFT id with SELF.tokens(0) == (id, 1), no
// tokens on the miner output, exactly one token and no R6 on the successor,
// added to v1.2 (638 bytes). The terminal LE guard (0x90) is at offset 616;
// blake2b256 below.
const EMISSION_TREE_HEX: &str = include_str!("../../01-emission-tests/trees/emission.hex");
const EMISSION_TREE_HASH: &str = "91eb745f2b44f255327a1e8720196944a34921e9e65e39bc275a406557fc0fe5";

// Accumulation scripts v2.2: destination = holder of a constant NFT at tokens(0),
// not itself at the accumulation script; merge cooldown 20,160 blocks; the transfer
// successor carries every input at the accumulation script or at the holder's own
// script (v2.2, EA-N1). They embed no script hash, so they do not change when a
// governance script changes.
const TREASURY_ACCUM_TREE_HEX: &str = include_str!("../../02-treasury-tests/trees/treasury_accumulation.hex");
const LP_ACCUM_TREE_HEX: &str = include_str!("../../03-lp-fund-tests/trees/lp_accumulation.hex");

/// Genesis emission box R4 and R5 for the test NFT ids: blake2b256 of the
/// treasury and LP accumulation trees (F-015). Real ids give other values.
const GENESIS_R4: &str = "2c923241dde1aba4f3d78d2b3719885a116894025832c9638ac4139be1ed611d";
const GENESIS_R5: &str = "f1816cc630c3dec827c5ef0c1c1263603bb0729b7752275a2e45762a55394a03";

// Governance scripts v2.2 (used in the forward tests 3b and 4b).
const TREASURY_GOV_TREE_HEX: &str = include_str!("../../02-treasury-tests/trees/treasury_governance.hex");
const LP_GOV_TREE_HEX: &str = include_str!("../../03-lp-fund-tests/trees/lp_fund.hex");

/// Test NFT ids compiled into the trees above.
const TREASURY_NFT: [u8; 32] = [0xAA; 32];
const LP_NFT: [u8; 32] = [0xBB; 32];

/// The three governance signer test keys (public test keys, F-013).
const SIGNER_SECRET_BYTES: [[u8; 32]; 3] = [
    [0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, 0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF,
     0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, 0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF],
    [0xFE, 0xDC, 0xBA, 0x98, 0x76, 0x54, 0x32, 0x10, 0xFE, 0xDC, 0xBA, 0x98, 0x76, 0x54, 0x32, 0x10,
     0xFE, 0xDC, 0xBA, 0x98, 0x76, 0x54, 0x32, 0x10, 0xFE, 0xDC, 0xBA, 0x98, 0x76, 0x54, 0x32, 0x10],
    [0xAA, 0xBB, 0xCC, 0xDD, 0x11, 0x22, 0x33, 0x44, 0xAA, 0xBB, 0xCC, 0xDD, 0x11, 0x22, 0x33, 0x44,
     0xAA, 0xBB, 0xCC, 0xDD, 0x11, 0x22, 0x33, 0x44, 0xAA, 0xBB, 0xCC, 0xDD, 0x11, 0x22, 0x33, 0x44],
];

// Dummy script: sigmaProp(HEIGHT > 0) — used for wrong-script rejection tests
const DUMMY_TREE_HEX: &str = "10010400d191a37300";

// Miner reward script, ErgoTreePredef.rewardOutputScript(2880, pk) as the
// stock Scala node serializes it: prefix ++ 33-byte pk ++ suffix (54 bytes).
// Same constants as 01-emission-tests (derived on the JVM, sigma-state 6.0.7).
const MINER_REWARD_DELAY: u32 = 2_880;
const MINER_SCRIPT_PREFIX_HEX: &str = "100204802d08cd";
const MINER_SCRIPT_SUFFIX_HEX: &str = "ea02d192a39a8cc7a70173007301";
// Header miner key: k*G with k = 1 (secret known, so tests can sign).
const MINER_SECRET: [u8; 32] = {
    let mut k = [0u8; 32];
    k[31] = 1;
    k
};
const MINER_PK: &str = "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";
const OTHER_PK: &str = "02c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5";

fn miner_reward_tree(pk_hex: &str) -> ErgoTree {
    load_tree(&format!("{}{}{}", MINER_SCRIPT_PREFIX_HEX, pk_hex, MINER_SCRIPT_SUFFIX_HEX))
}

fn make_miner_box(value: u64, creation_height: u32) -> ErgoBox {
    make_output_box(&miner_reward_tree(MINER_PK), value, creation_height)
}

fn miner_share(reward: u64) -> u64 {
    reward - treasury_reward(reward) - lp_reward(reward)
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

fn treasury_reward(reward: u64) -> u64 {
    reward * TREASURY_PCT / 100
}

fn lp_reward(reward: u64) -> u64 {
    reward * LP_PCT / 100
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

// ============================================================
// HELPERS
// ============================================================

fn proposition_hash(tree: &ErgoTree) -> Vec<u8> {
    let bytes = tree.sigma_serialize_bytes().expect("serialize tree");
    let mut h = Blake2b256::new();
    h.update(&bytes);
    h.finalize().to_vec()
}

/// Test emission NFT id compiled into the emission tree (v1.3, EMISSION_NFT_ID_PLACEHOLDER).
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

fn build_context(self_box: ErgoBox, outputs: Vec<ErgoBox>, height: u32) -> Context<'static> {
    build_context_pk(self_box, outputs, height, MINER_PK)
}

/// Context whose PreHeader miner key (CONTEXT.minerPubKey) is `miner_pk_hex`.
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
    pre_header.miner_pk = Box::new(
        EcPoint::from_base16_str(miner_pk_hex.to_string()).expect("valid compressed point"),
    );
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

/// Reduce the tree: Ok(b) for a trivial result `b`, Err on evaluation error
/// or a non-trivial (signature-requiring) result.
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

/// Prove with `secrets` and verify (for scripts that need a signature).
fn prove_and_verify(tree: &ErgoTree, ctx: &Context, secrets: Vec<PrivateInput>) -> bool {
    let prover = TestProver { secrets };
    let message = vec![7u8; 32];
    let proof = match prover.prove(tree, ctx, message.as_slice(), &HintsBag::empty()) {
        Ok(p) => p.proof,
        Err(_) => return false,
    };
    matches!(TestVerifier.verify(tree, ctx, proof, message.as_slice()), Ok(v) if v.result)
}

fn hex_of(b: &[u8]) -> String { b.iter().map(|x| format!("{:02x}", x)).collect() }

fn nft(id: [u8; 32]) -> Token {
    Token { token_id: TokenId::from(Digest32::from(id)), amount: TokenAmount::try_from(1u64).unwrap() }
}

fn make_box(tree: &ErgoTree, value: u64, tokens: Vec<Token>, regs: std::collections::HashMap<NonMandatoryRegisterId, Constant>, ch: u32) -> ErgoBox {
    let candidate = ErgoBoxCandidate {
        value: BoxValue::try_from(value).unwrap(),
        ergo_tree: tree.clone(),
        tokens: if tokens.is_empty() { None } else { Some(tokens.try_into().unwrap()) },
        additional_registers: NonMandatoryRegisters::new(regs).unwrap(),
        creation_height: ch,
    };
    ErgoBox::from_box_candidate(&candidate, TxId::zero(), 0).unwrap()
}

/// Idle treasury governance box: NFT, R4 32 zero bytes, R5 0, R6 empty, R7 0, R8 0.
fn treasury_gov_box(value: u64, ch: u32) -> ErgoBox {
    let regs = std::collections::HashMap::from([
        (NonMandatoryRegisterId::R4, Constant::from(vec![0u8; 32])),
        (NonMandatoryRegisterId::R5, Constant::from(0i64)),
        (NonMandatoryRegisterId::R6, Constant::from(Vec::<u8>::new())),
        (NonMandatoryRegisterId::R7, Constant::from(0i64)),
        (NonMandatoryRegisterId::R8, Constant::from(0i64)),
    ]);
    make_box(&load_tree(TREASURY_GOV_TREE_HEX), value, vec![nft(TREASURY_NFT)], regs, ch)
}

/// Idle LP fund box: NFT, empty whitelist, R5 0, R6 0, R7 32 zero bytes (the
/// v2.1 genesis value; was empty).
fn lp_fund_box(value: u64, ch: u32) -> ErgoBox {
    use ergo_lib::ergotree_ir::mir::{constant::Literal, value::CollKind};
    use ergo_lib::ergotree_ir::types::{stuple::STuple, stype::SType};
    use std::sync::Arc;
    let pair = SType::STuple(STuple::pair(SType::SColl(Arc::new(SType::SByte)), SType::SLong));
    let empty: Vec<Literal> = vec![];
    let wl = Constant {
        tpe: SType::SColl(Arc::new(pair.clone())),
        v: Literal::Coll(CollKind::WrappedColl { elem_tpe: pair, items: empty.into() }),
    };
    let regs = std::collections::HashMap::from([
        (NonMandatoryRegisterId::R4, wl),
        (NonMandatoryRegisterId::R5, Constant::from(0i64)),
        (NonMandatoryRegisterId::R6, Constant::from(0i64)),
        (NonMandatoryRegisterId::R7, Constant::from(vec![0u8; 32])),
    ]);
    make_box(&load_tree(LP_GOV_TREE_HEX), value, vec![nft(LP_NFT)], regs, ch)
}

/// Context with several inputs; SELF = inputs[self_idx].
fn build_context_multi(inputs: Vec<ErgoBox>, self_idx: usize, outputs: Vec<ErgoBox>, height: u32) -> Context<'static> {
    let ins: Vec<&'static ErgoBox> = inputs.into_iter().map(|b| &*Box::leak(Box::new(b))).collect();
    let mut pre_header = force_any_val::<PreHeader>();
    pre_header.miner_pk = Box::new(EcPoint::from_base16_str(MINER_PK.to_string()).unwrap());
    pre_header.height = height;
    Context {
        height,
        self_box: ins[self_idx],
        outputs: Vec::leak(outputs),
        data_inputs: None,
        inputs: ins.try_into().unwrap(),
        pre_header,
        headers: force_any_val::<[Header; 10]>(),
        extension: ContextExtension::empty(),
    }
}

fn signers_2_of_3() -> Vec<PrivateInput> {
    SIGNER_SECRET_BYTES[..2]
        .iter()
        .map(|b| PrivateInput::DlogProverInput(DlogProverInput::from_bytes(b).unwrap()))
        .collect()
}

/// Governance accept: reduction leaves only the threshold signature (`reduce`
/// reports that as a non-trivial result) and 2 of the 3 test keys prove it.
fn assert_signed(tree: &ErgoTree, ctx: &Context, msg: &str) {
    let r = reduce(tree, ctx);
    assert!(matches!(&r, Err(e) if e.starts_with("non-trivial")), "{}: {:?}", msg, r);
    assert!(prove_and_verify(tree, ctx, signers_2_of_3()), "{} (2-of-3)", msg);
    assert!(!prove_and_verify(tree, ctx, signers_2_of_3()[..1].to_vec()), "{} (1-of-3)", msg);
}

/// Build a valid normal-path coinbase scenario at any height using real
/// accumulation contract scripts. Returns (emission_tree, self_box, outputs, height).
fn coinbase_scenario(
    h: u32,
    self_val: u64,
    creation_height: u32,
) -> (ErgoTree, ErgoBox, Vec<ErgoBox>, u32) {
    let emission_tree = load_tree(EMISSION_TREE_HEX);
    let treasury_accum_tree = load_tree(TREASURY_ACCUM_TREE_HEX);
    let lp_accum_tree = load_tree(LP_ACCUM_TREE_HEX);

    let t_hash = proposition_hash(&treasury_accum_tree);
    let l_hash = proposition_hash(&lp_accum_tree);

    let self_box = make_emission_box(&emission_tree, self_val, &t_hash, &l_hash, creation_height);
    let reward = block_reward(h);
    let t_amt = treasury_reward(reward);
    let l_amt = lp_reward(reward);

    let outputs = vec![
        make_next_emission_box(&self_box, self_val - reward, h),
        make_output_box(&treasury_accum_tree, t_amt, h),
        make_output_box(&lp_accum_tree, l_amt, h),
        make_miner_box(miner_share(reward), h),
    ];

    (emission_tree, self_box, outputs, h)
}

// ============================================================
// PHASE 0: SETUP VERIFICATION
// ============================================================

#[cfg(test)]
mod phase0_setup {
    use super::*;

    #[test]
    fn all_trees_round_trip() {
        let trees = [
            ("emission", EMISSION_TREE_HEX),
            ("treasury_accum", TREASURY_ACCUM_TREE_HEX),
            ("lp_accum", LP_ACCUM_TREE_HEX),
            ("treasury_gov", TREASURY_GOV_TREE_HEX),
            ("lp_gov", LP_GOV_TREE_HEX),
        ];

        for (name, hex) in &trees {
            let tree = load_tree(hex);
            let bytes = tree.sigma_serialize_bytes().unwrap();
            let original = hex_to_bytes(hex);
            assert_eq!(bytes, original, "{} round-trip mismatch", name);
        }
    }

    #[test]
    fn all_propositions_parse() {
        let trees = [
            ("emission", EMISSION_TREE_HEX),
            ("treasury_accum", TREASURY_ACCUM_TREE_HEX),
            ("lp_accum", LP_ACCUM_TREE_HEX),
            ("treasury_gov", TREASURY_GOV_TREE_HEX),
            ("lp_gov", LP_GOV_TREE_HEX),
        ];

        for (name, hex) in &trees {
            let tree = load_tree(hex);
            tree.proposition().unwrap_or_else(|e| panic!("{} proposition failed: {:?}", name, e));
        }
    }

    #[test]
    fn accumulation_hashes_are_distinct() {
        let t_hash = proposition_hash(&load_tree(TREASURY_ACCUM_TREE_HEX));
        let l_hash = proposition_hash(&load_tree(LP_ACCUM_TREE_HEX));
        assert_ne!(t_hash, l_hash, "treasury and LP accumulation hashes must differ");
    }

    #[test]
    fn print_accumulation_hashes() {
        let t_tree = load_tree(TREASURY_ACCUM_TREE_HEX);
        let l_tree = load_tree(LP_ACCUM_TREE_HEX);

        let t_hash = proposition_hash(&t_tree);
        let l_hash = proposition_hash(&l_tree);

        let t_hex: String = t_hash.iter().map(|b| format!("{:02x}", b)).collect();
        let l_hex: String = l_hash.iter().map(|b| format!("{:02x}", b)).collect();

        println!("treasury accumulation hash (R4): {}", t_hex);
        println!("treasury accumulation tree size: {} bytes", t_tree.sigma_serialize_bytes().unwrap().len());
        println!("LP accumulation hash (R5):       {}", l_hex);
        println!("LP accumulation tree size:       {} bytes", l_tree.sigma_serialize_bytes().unwrap().len());

        let e_tree = load_tree(EMISSION_TREE_HEX);
        println!("emission tree size:              {} bytes", e_tree.sigma_serialize_bytes().unwrap().len());
    }

    /// v2.0 (F-001, F-008): the accumulation scripts embed the NFT id of their fund,
    /// not a governance script hash, and the genesis R4 / R5 are the hashes of the
    /// two accumulation trees (F-015).
    #[test]
    fn accumulation_governance_hashes_are_consistent() {
        let t_hex = TREASURY_ACCUM_TREE_HEX.trim();
        let l_hex = LP_ACCUM_TREE_HEX.trim();
        assert!(t_hex.contains(&format!("0e20{}", hex_of(&TREASURY_NFT))), "treasury accumulation embeds the treasury NFT id");
        assert!(l_hex.contains(&format!("0e20{}", hex_of(&LP_NFT))), "LP accumulation embeds the LP NFT id");
        assert!(!t_hex.contains(&hex_of(&LP_NFT)) && !l_hex.contains(&hex_of(&TREASURY_NFT)), "NFT ids not crossed");
        for gov in [TREASURY_GOV_TREE_HEX, LP_GOV_TREE_HEX] {
            let gov_hash = hex_of(&proposition_hash(&load_tree(gov)));
            assert!(!t_hex.contains(&gov_hash) && !l_hex.contains(&gov_hash), "no governance hash embedded");
        }
        assert_eq!(hex_of(&proposition_hash(&load_tree(TREASURY_ACCUM_TREE_HEX))), GENESIS_R4);
        assert_eq!(hex_of(&proposition_hash(&load_tree(LP_ACCUM_TREE_HEX))), GENESIS_R5);
        assert_eq!(hex_of(&proposition_hash(&load_tree(EMISSION_TREE_HEX))), EMISSION_TREE_HASH);
    }
}

// ============================================================
// TEST 1: HAPPY PATH AT HEIGHT 100
// ============================================================

#[cfg(test)]
mod test1_happy_path_h100 {
    use super::*;

    #[test]
    fn accept_coinbase_at_height_100() {
        let h = 100u32;
        let self_val = 200 * NANOCOIN;
        let (tree, self_box, outputs, h) = coinbase_scenario(h, self_val, h - 1);
        let ctx = build_context(self_box, outputs, h);
        assert_accepts(&tree, &ctx, "full coinbase TX at h=100 should accept with real accumulation scripts");
    }

    #[test]
    fn verify_amounts_at_height_100() {
        let reward = block_reward(100);
        assert_eq!(reward, 75 * NANOCOIN, "epoch 0 reward should be 75 coins");
        assert_eq!(treasury_reward(reward), 3_000_000_000, "treasury 4% = 3.0 coins");
        assert_eq!(lp_reward(reward), 2_250_000_000, "LP 3% = 2.25 coins");
    }
}

// ============================================================
// TEST 2: HAPPY PATH AT HALVING BOUNDARY (h=1,051,200)
// ============================================================

#[cfg(test)]
mod test2_halving_boundary {
    use super::*;

    #[test]
    fn accept_coinbase_at_first_halving() {
        let h = BLOCKS_PER_HALVING; // 1,051,200
        let self_val = 200 * NANOCOIN;
        let (tree, self_box, outputs, h) = coinbase_scenario(h, self_val, h - 1);
        let ctx = build_context(self_box, outputs, h);
        assert_accepts(&tree, &ctx, "coinbase at halving boundary should accept with halved reward");
    }

    #[test]
    fn verify_amounts_at_halving() {
        let reward = block_reward(BLOCKS_PER_HALVING);
        assert_eq!(reward, 37_500_000_000, "epoch 1 reward should be 37.5 coins");
        assert_eq!(treasury_reward(reward), 1_500_000_000, "treasury 4% of 37.5 = 1.5 coins");
        assert_eq!(lp_reward(reward), 1_125_000_000, "LP 3% of 37.5 = 1.125 coins");
    }
}

// ============================================================
// TEST 3: TREASURY ACCUMULATION IS INDEPENDENTLY SPENDABLE
// ============================================================

#[cfg(test)]
mod test3_treasury_accum_spendable {
    use super::*;

    /// 3a: Treasury accumulation box can consolidate with itself (sum of all
    /// inputs, box older than the 20,160-block cooldown, F-002 / F-025; v2.1
    /// moves the height from 1,000 to 30,000).
    #[test]
    fn accept_treasury_accum_consolidation() {
        let accum_tree = load_tree(TREASURY_ACCUM_TREE_HEX);
        let h = 30_000u32;
        let value = 3_000_000_000; // matches treasury reward at 75-coin block

        let a = make_output_box(&accum_tree, value, 100);
        let b = make_output_box(&accum_tree, value, 101);
        let out0 = make_output_box(&accum_tree, 2 * value, h);
        for i in 0..2 {
            let ctx = build_context_multi(vec![a.clone(), b.clone()], i, vec![out0.clone()], h);
            assert_eq!(reduce(&accum_tree, &ctx), Ok(true), "treasury accumulation consolidation should accept");
            assert!(evaluate(&accum_tree, &ctx));
        }
    }

    /// 3b (rewritten, F-001 / F-003): the old version paid into a tokenless box at
    /// the governance script, which could never be spent again. The pipeline is
    /// [treasury NFT holder, accumulation box] -> [holder successor with the sum].
    #[test]
    fn accept_treasury_accum_transfer_to_governance() {
        let accum_tree = load_tree(TREASURY_ACCUM_TREE_HEX);
        let gov_tree = load_tree(TREASURY_GOV_TREE_HEX);
        let h = 100u32;
        let value = 5 * NANOCOIN;

        let gov = treasury_gov_box(50 * NANOCOIN, h - 1);
        let acc = make_output_box(&accum_tree, value, h - 1);
        let succ = treasury_gov_box(50 * NANOCOIN + value, h);
        let ins = vec![gov, acc];
        let ctx = build_context_multi(ins.clone(), 1, vec![succ.clone()], h);
        assert_eq!(reduce(&accum_tree, &ctx), Ok(true), "accumulation side of the forward");
        assert!(evaluate(&accum_tree, &ctx));
        let ctx = build_context_multi(ins, 0, vec![succ], h);
        assert_signed(&gov_tree, &ctx, "governance consolidation side of the forward");
        // The old layout is rejected.
        let old = build_context(make_output_box(&accum_tree, value, h - 1), vec![make_output_box(&gov_tree, value, h)], h);
        assert_eq!(reduce(&accum_tree, &old), Ok(false), "tokenless box at the governance script");
    }

    /// 3c: Treasury accumulation rejects transfer to wrong destination.
    #[test]
    fn reject_treasury_accum_wrong_destination() {
        let accum_tree = load_tree(TREASURY_ACCUM_TREE_HEX);
        let wrong_tree = load_tree(DUMMY_TREE_HEX);
        let h = 100u32;
        let value = 5 * NANOCOIN;

        let self_box = make_output_box(&accum_tree, value, h - 1);
        let out0 = make_output_box(&wrong_tree, value, h);
        let ctx = build_context(self_box, vec![out0], h);
        assert_script_false(&accum_tree, &ctx, "treasury accumulation transfer to wrong script should reject");
    }
}

// ============================================================
// TEST 4: LP ACCUMULATION IS INDEPENDENTLY SPENDABLE
// ============================================================

#[cfg(test)]
mod test4_lp_accum_spendable {
    use super::*;

    /// 4a: LP accumulation box can consolidate with itself (F-002 / F-025; v2.1
    /// cooldown 20,160, height moved from 1,000 to 30,000).
    #[test]
    fn accept_lp_accum_consolidation() {
        let accum_tree = load_tree(LP_ACCUM_TREE_HEX);
        let h = 30_000u32;
        let value = 2_250_000_000; // matches LP reward at 75-coin block

        let a = make_output_box(&accum_tree, value, 100);
        let b = make_output_box(&accum_tree, value, 101);
        let out0 = make_output_box(&accum_tree, 2 * value, h);
        for i in 0..2 {
            let ctx = build_context_multi(vec![a.clone(), b.clone()], i, vec![out0.clone()], h);
            assert_eq!(reduce(&accum_tree, &ctx), Ok(true), "LP accumulation consolidation should accept");
            assert!(evaluate(&accum_tree, &ctx));
        }
    }

    /// 4b (rewritten, F-001 / F-003): forward into the LP fund NFT holder.
    #[test]
    fn accept_lp_accum_transfer_to_governance() {
        let accum_tree = load_tree(LP_ACCUM_TREE_HEX);
        let gov_tree = load_tree(LP_GOV_TREE_HEX);
        let h = 100u32;
        let value = 2_500_000_000;

        let gov = lp_fund_box(50 * NANOCOIN, h - 1);
        let acc = make_output_box(&accum_tree, value, h - 1);
        let succ = lp_fund_box(50 * NANOCOIN + value, h);
        let ins = vec![gov, acc];
        let ctx = build_context_multi(ins.clone(), 1, vec![succ.clone()], h);
        assert_eq!(reduce(&accum_tree, &ctx), Ok(true), "accumulation side of the forward");
        assert!(evaluate(&accum_tree, &ctx));
        let ctx = build_context_multi(ins, 0, vec![succ], h);
        assert_signed(&gov_tree, &ctx, "LP fund consolidation side of the forward");
        let old = build_context(make_output_box(&accum_tree, value, h - 1), vec![make_output_box(&gov_tree, value, h)], h);
        assert_eq!(reduce(&accum_tree, &old), Ok(false), "tokenless box at the LP fund script");
    }

    /// 4c: LP accumulation rejects transfer to wrong destination.
    #[test]
    fn reject_lp_accum_wrong_destination() {
        let accum_tree = load_tree(LP_ACCUM_TREE_HEX);
        let wrong_tree = load_tree(DUMMY_TREE_HEX);
        let h = 100u32;
        let value = 2_500_000_000;

        let self_box = make_output_box(&accum_tree, value, h - 1);
        let out0 = make_output_box(&wrong_tree, value, h);
        let ctx = build_context(self_box, vec![out0], h);
        assert_script_false(&accum_tree, &ctx, "LP accumulation transfer to wrong script should reject");
    }
}

// ============================================================
// TEST 5: WRONG TREASURY SCRIPT → REJECT
// ============================================================

#[cfg(test)]
mod test5_wrong_treasury_script {
    use super::*;

    #[test]
    fn reject_wrong_treasury_script() {
        let h = 100u32;
        let self_val = 200 * NANOCOIN;

        let emission_tree = load_tree(EMISSION_TREE_HEX);
        let treasury_accum_tree = load_tree(TREASURY_ACCUM_TREE_HEX);
        let lp_accum_tree = load_tree(LP_ACCUM_TREE_HEX);
        let wrong_tree = load_tree(DUMMY_TREE_HEX);

        let t_hash = proposition_hash(&treasury_accum_tree);
        let l_hash = proposition_hash(&lp_accum_tree);

        let self_box = make_emission_box(&emission_tree, self_val, &t_hash, &l_hash, h - 1);
        let reward = block_reward(h);
        let t_amt = treasury_reward(reward);
        let l_amt = lp_reward(reward);

        let outputs = vec![
            make_next_emission_box(&self_box, self_val - reward, h),
            make_output_box(&wrong_tree, t_amt, h),       // WRONG: not treasury accum
            make_output_box(&lp_accum_tree, l_amt, h),
            make_miner_box(miner_share(reward), h),
        ];
        let ctx = build_context(self_box, outputs, h);
        assert_script_false(&emission_tree, &ctx, "wrong treasury script should reject — blake2b256 hash mismatch with R4");
    }
}

// ============================================================
// TEST 6: WRONG LP SCRIPT → REJECT
// ============================================================

#[cfg(test)]
mod test6_wrong_lp_script {
    use super::*;

    #[test]
    fn reject_wrong_lp_script() {
        let h = 100u32;
        let self_val = 200 * NANOCOIN;

        let emission_tree = load_tree(EMISSION_TREE_HEX);
        let treasury_accum_tree = load_tree(TREASURY_ACCUM_TREE_HEX);
        let lp_accum_tree = load_tree(LP_ACCUM_TREE_HEX);
        let wrong_tree = load_tree(DUMMY_TREE_HEX);

        let t_hash = proposition_hash(&treasury_accum_tree);
        let l_hash = proposition_hash(&lp_accum_tree);

        let self_box = make_emission_box(&emission_tree, self_val, &t_hash, &l_hash, h - 1);
        let reward = block_reward(h);
        let t_amt = treasury_reward(reward);
        let l_amt = lp_reward(reward);

        let outputs = vec![
            make_next_emission_box(&self_box, self_val - reward, h),
            make_output_box(&treasury_accum_tree, t_amt, h),
            make_output_box(&wrong_tree, l_amt, h),        // WRONG: not LP accum
            make_miner_box(miner_share(reward), h),
        ];
        let ctx = build_context(self_box, outputs, h);
        assert_script_false(&emission_tree, &ctx, "wrong LP script should reject — blake2b256 hash mismatch with R5");
    }
}

// ============================================================
// TEST 7: SWAPPED OUTPUT POSITIONS → REJECT
// ============================================================

#[cfg(test)]
mod test7_swapped_positions {
    use super::*;

    #[test]
    fn reject_swapped_treasury_lp_positions() {
        let h = 100u32;
        let self_val = 200 * NANOCOIN;

        let emission_tree = load_tree(EMISSION_TREE_HEX);
        let treasury_accum_tree = load_tree(TREASURY_ACCUM_TREE_HEX);
        let lp_accum_tree = load_tree(LP_ACCUM_TREE_HEX);

        let t_hash = proposition_hash(&treasury_accum_tree);
        let l_hash = proposition_hash(&lp_accum_tree);

        let self_box = make_emission_box(&emission_tree, self_val, &t_hash, &l_hash, h - 1);
        let reward = block_reward(h);
        let t_amt = treasury_reward(reward);
        let l_amt = lp_reward(reward);

        // SWAPPED: LP accum at slot 1 (should be treasury), treasury accum at slot 2 (should be LP)
        let outputs = vec![
            make_next_emission_box(&self_box, self_val - reward, h),
            make_output_box(&lp_accum_tree, t_amt, h),       // slot 1: LP script, treasury value
            make_output_box(&treasury_accum_tree, l_amt, h),  // slot 2: treasury script, LP value
            make_miner_box(miner_share(reward), h),
        ];
        let ctx = build_context(self_box, outputs, h);
        assert_script_false(&emission_tree, &ctx, "swapped output positions should reject — hashes don't match respective registers");
    }
}

// ============================================================
// TEST 8: MULTI-BLOCK SEQUENCE ACROSS HALVING
// ============================================================

#[cfg(test)]
mod test8_multi_block_halving {
    use super::*;

    #[test]
    fn accept_three_block_chain_across_halving() {
        let emission_tree = load_tree(EMISSION_TREE_HEX);
        let treasury_accum_tree = load_tree(TREASURY_ACCUM_TREE_HEX);
        let lp_accum_tree = load_tree(LP_ACCUM_TREE_HEX);

        let t_hash = proposition_hash(&treasury_accum_tree);
        let l_hash = proposition_hash(&lp_accum_tree);

        let starting_value = 200 * NANOCOIN;

        // ---- Block 1,051,199 (epoch 0, reward = 75 coins) ----
        let h0 = BLOCKS_PER_HALVING - 1; // 1,051,199
        let reward0 = block_reward(h0);
        assert_eq!(reward0, 75 * NANOCOIN, "block before halving should have 75-coin reward");

        let emission_box_0 = make_emission_box(
            &emission_tree, starting_value, &t_hash, &l_hash, h0 - 1,
        );
        let val_after_0 = starting_value - reward0;
        let outputs_0 = vec![
            make_next_emission_box(&emission_box_0, val_after_0, h0),
            make_output_box(&treasury_accum_tree, treasury_reward(reward0), h0),
            make_output_box(&lp_accum_tree, lp_reward(reward0), h0),
            make_miner_box(miner_share(reward0), h0),
        ];
        let ctx_0 = build_context(emission_box_0, outputs_0.clone(), h0);
        assert_accepts(&emission_tree, &ctx_0, "block 1,051,199 (epoch 0) should accept");

        // ---- Block 1,051,200 (epoch 1, reward = 37.5 coins) ----
        let h1 = BLOCKS_PER_HALVING; // 1,051,200
        let reward1 = block_reward(h1);
        assert_eq!(reward1, 37_500_000_000, "first halving block should have 37.5-coin reward");

        // Chain: output emission box from block 0 becomes input for block 1.
        // Reconstruct with correct creation_height from previous output.
        let emission_box_1 = make_emission_box(
            &emission_tree, val_after_0, &t_hash, &l_hash, h0,
        );
        let val_after_1 = val_after_0 - reward1;
        let outputs_1 = vec![
            make_next_emission_box(&emission_box_1, val_after_1, h1),
            make_output_box(&treasury_accum_tree, treasury_reward(reward1), h1),
            make_output_box(&lp_accum_tree, lp_reward(reward1), h1),
            make_miner_box(miner_share(reward1), h1),
        ];
        let ctx_1 = build_context(emission_box_1, outputs_1.clone(), h1);
        assert_accepts(&emission_tree, &ctx_1, "block 1,051,200 (epoch 1, halving boundary) should accept");

        // ---- Block 1,051,201 (epoch 1, reward = 37.5 coins) ----
        let h2 = BLOCKS_PER_HALVING + 1; // 1,051,201
        let reward2 = block_reward(h2);
        assert_eq!(reward2, 37_500_000_000, "block after halving should still be 37.5-coin reward");

        let emission_box_2 = make_emission_box(
            &emission_tree, val_after_1, &t_hash, &l_hash, h1,
        );
        let val_after_2 = val_after_1 - reward2;
        let outputs_2 = vec![
            make_next_emission_box(&emission_box_2, val_after_2, h2),
            make_output_box(&treasury_accum_tree, treasury_reward(reward2), h2),
            make_output_box(&lp_accum_tree, lp_reward(reward2), h2),
            make_miner_box(miner_share(reward2), h2),
        ];
        let ctx_2 = build_context(emission_box_2, outputs_2, h2);
        assert_accepts(&emission_tree, &ctx_2, "block 1,051,201 (epoch 1, post-halving) should accept");

        // ---- Verify value chain ----
        assert_eq!(val_after_2, starting_value - 75 * NANOCOIN - 2 * 37_500_000_000,
            "remaining emission value should equal start minus 150 coins total");
        println!("3-block chain across halving: OK");
        println!("  block 1,051,199: reward=75, remaining={}", val_after_0);
        println!("  block 1,051,200: reward=37.5, remaining={}", val_after_1);
        println!("  block 1,051,201: reward=37.5, remaining={}", val_after_2);
    }
}

// ============================================================
// TEST 9: 30 s RETUNE — SIXTH ARM, TAIL BOUNDARY, YEAR-20 END
// ============================================================

/// Coinbase at `h` paying `reward` into the real accumulation scripts.
fn coinbase_paying(h: u32, self_val: u64, reward: u64) -> (ErgoTree, Context<'static>) {
    let emission_tree = load_tree(EMISSION_TREE_HEX);
    let treasury_accum_tree = load_tree(TREASURY_ACCUM_TREE_HEX);
    let lp_accum_tree = load_tree(LP_ACCUM_TREE_HEX);
    let t_hash = proposition_hash(&treasury_accum_tree);
    let l_hash = proposition_hash(&lp_accum_tree);
    let self_box = make_emission_box(&emission_tree, self_val, &t_hash, &l_hash, h - 1);
    let outputs = vec![
        make_next_emission_box(&self_box, self_val - reward, h),
        make_output_box(&treasury_accum_tree, treasury_reward(reward), h),
        make_output_box(&lp_accum_tree, lp_reward(reward), h),
        make_miner_box(miner_share(reward), h),
    ];
    let ctx = build_context(self_box, outputs, h);
    (emission_tree, ctx)
}

#[cfg(test)]
mod test9_retune_30s {
    use super::*;

    #[test]
    fn genesis_box_derives_from_schedule() {
        // The schedule total is the total supply; the emission box holds it
        // minus the two 100 YOLO fund boxes (2026-11-12).
        let total: u64 = (0..MAX_HALVINGS)
            .map(|e| block_reward(e * BLOCKS_PER_HALVING) * BLOCKS_PER_HALVING as u64)
            .sum();
        assert_eq!(total, TOTAL_SUPPLY);
        assert_eq!(GENESIS_EMISSION_BOX + 2 * GENESIS_FUND_BOX, total);
    }

    /// Real-chain box value before the spend at `h` (heights 1 .. h-1 paid;
    /// height 0 is never mined).
    fn real_box_value(h: u32) -> u64 {
        let mut paid: u64 = 0;
        for e in 0..=(h / BLOCKS_PER_HALVING) {
            let lo = std::cmp::max(1, e * BLOCKS_PER_HALVING);
            let hi = std::cmp::min(h - 1, (e + 1) * BLOCKS_PER_HALVING - 1);
            if hi >= lo {
                paid += block_reward(lo) * (hi - lo + 1) as u64;
            }
        }
        GENESIS_EMISSION_BOX.checked_sub(paid).expect("real box already spent out")
    }

    #[test]
    fn accept_coinbase_across_sixth_arm_boundary() {
        let b = 5 * BLOCKS_PER_HALVING; // 5_256_000
        assert_eq!(block_reward(b - 1), 4_687_500_000);
        assert_eq!(block_reward(b), 2_343_750_000);
        for h in [b - 1, b, b + 1] {
            let (tree, self_box, outputs, h) = coinbase_scenario(h, 200 * NANOCOIN, h - 1);
            let ctx = build_context(self_box, outputs, h);
            assert_accepts(&tree, &ctx, &format!("coinbase at h={} should accept", h));
        }
        let (tree, ctx) = coinbase_paying(b, 200 * NANOCOIN, MIN_REWARD);
        assert_script_false(&tree, &ctx, "tail payout at the sixth arm must reject");
    }

    #[test]
    fn accept_coinbase_across_tail_boundary() {
        let b = 6 * BLOCKS_PER_HALVING; // 6_307_200
        assert_eq!(block_reward(b - 1), 2_343_750_000);
        assert_eq!(block_reward(b), MIN_REWARD);
        for h in [b - 1, b, b + 1] {
            let (tree, self_box, outputs, h) = coinbase_scenario(h, 200 * NANOCOIN, h - 1);
            let ctx = build_context(self_box, outputs, h);
            assert_accepts(&tree, &ctx, &format!("coinbase at h={} should accept", h));
        }
        let (tree, ctx) = coinbase_paying(b, 200 * NANOCOIN, 2_343_750_000);
        assert_script_false(&tree, &ctx, "sixth-arm payout in the tail must reject");
    }

    #[test]
    fn year_20_end_box_drains_through_accumulation_scripts() {
        let end = MAX_HALVINGS * BLOCKS_PER_HALVING; // 21_024_000
        // Contract cases: two tail rewards left at end-2 (schedule view),
        // and a 75-coin box at `end` (the script has no end height). The
        // real box is spent out earlier, at 21_023_916 (2026-11-12 genesis).
        let (tree, ctx) = coinbase_paying(end - 2, 2 * MIN_REWARD, MIN_REWARD);
        assert_accepts(&tree, &ctx, "coinbase at end-2 should accept");
        let (tree, ctx) = coinbase_paying(end, 75 * NANOCOIN, MIN_REWARD);
        assert_accepts(&tree, &ctx, "first post-year-20 tail coinbase should accept");
        // Real chain: last normal coinbase at 21_023_915 from 2.5 coins.
        let v = real_box_value(REAL_LAST_NORMAL);
        assert_eq!(v, 2_500_000_000);
        let (tree, ctx) = coinbase_paying(REAL_LAST_NORMAL, v, MIN_REWARD);
        assert_accepts(&tree, &ctx, "real-chain last normal coinbase should accept");
    }

    /// Terminal coinbase into the real accumulation scripts: treasury accum,
    /// LP accum, then the bound miner output with the remainder (OUTPUTS(2)).
    fn terminal_coinbase(h: u32, remaining: u64) -> (ErgoTree, Context<'static>, [u64; 3]) {
        let emission_tree = load_tree(EMISSION_TREE_HEX);
        let treasury_accum_tree = load_tree(TREASURY_ACCUM_TREE_HEX);
        let lp_accum_tree = load_tree(LP_ACCUM_TREE_HEX);
        let miner_tree = miner_reward_tree(MINER_PK);
        let t_hash = proposition_hash(&treasury_accum_tree);
        let l_hash = proposition_hash(&lp_accum_tree);
        let self_box = make_emission_box(&emission_tree, remaining, &t_hash, &l_hash, h - 1);
        let t = treasury_reward(remaining);
        let l = lp_reward(remaining);
        let m = remaining - t - l;
        let outputs = vec![
            make_output_box(&treasury_accum_tree, t, h),
            make_output_box(&lp_accum_tree, l, h),
            make_output_box(&miner_tree, m, h),
        ];
        let ctx = build_context(self_box, outputs, h);
        (emission_tree, ctx, [t, l, m])
    }

    #[test]
    fn real_chain_last_block_terminal_coinbase_accepts() {
        // 2026-11-10 fix (`<=`), contract case at a tail height: a box
        // holding exactly one 1.5-coin reward. The terminal coinbase must
        // accept and pay the normal split. Before the 2026-11-12 genesis
        // split this was the real-chain last block; the real terminal spend
        // is now at 21_023_916 from a 1.0-coin box
        // (real_chain_terminal_coinbase_at_21_023_916).
        let h = MAX_HALVINGS * BLOCKS_PER_HALVING + 49;
        assert_eq!(block_reward(h), MIN_REWARD);
        let (tree, ctx, [t, l, m]) = terminal_coinbase(h, MIN_REWARD);
        assert_accepts(&tree, &ctx, "value == reward terminal coinbase should accept");
        assert_eq!((t, l, m), (60_000_000, 45_000_000, 1_395_000_000));
        assert_eq!(t, treasury_reward(MIN_REWARD));
        assert_eq!(l, lp_reward(MIN_REWARD));
        // Cannot fire one block early (2 rewards left) or 1 nano above.
        let (tree, ctx, _) = terminal_coinbase(h - 1, 2 * MIN_REWARD);
        assert_script_false(&tree, &ctx, "terminal at 2 x reward must reject");
        let (tree, ctx, _) = terminal_coinbase(h, MIN_REWARD + 1);
        assert_script_false(&tree, &ctx, "terminal at reward + 1 must reject");
    }

    #[test]
    fn real_chain_terminal_coinbase_at_21_023_916() {
        // 2026-11-12 genesis: the box spent at 21_023_916 holds 1.0 coin,
        // below the 1.5 reward. Terminal coinbase into the real accumulation
        // scripts: 0.04 / 0.03 / 0.93. One block earlier (2.5 coins) the
        // terminal path rejects.
        assert_eq!(REAL_LAST_NORMAL + 1, REAL_TERMINAL);
        let v = real_box_value(REAL_TERMINAL);
        assert_eq!(v, 1_000_000_000);
        assert!(v < block_reward(REAL_TERMINAL));
        let (tree, ctx, [t, l, m]) = terminal_coinbase(REAL_TERMINAL, v);
        assert_accepts(&tree, &ctx, "real-chain terminal coinbase should accept");
        assert_eq!((t, l, m), (40_000_000, 30_000_000, 930_000_000));
        let (tree, ctx, _) = terminal_coinbase(REAL_LAST_NORMAL, real_box_value(REAL_LAST_NORMAL));
        assert_script_false(&tree, &ctx, "terminal at 2.5 coins must reject");
    }
}

// ============================================================
// TEST 10: MINER OUTPUT BINDING (Gap C, 2026-11-10)
// ============================================================

#[cfg(test)]
mod test10_miner_binding {
    use super::*;

    fn miner_secret() -> PrivateInput {
        PrivateInput::DlogProverInput(DlogProverInput::from_bytes(&MINER_SECRET).unwrap())
    }

    #[test]
    fn coinbase_miner_reward_matures_after_2880_blocks() {
        // Coinbase at h = 100 into the real accumulation scripts, then spend
        // the miner reward box it created: locked until 100 + 2880, then
        // spendable with the header key's secret only.
        let h = 100u32;
        let (tree, self_box, outputs, h) = coinbase_scenario(h, 200 * NANOCOIN, h - 1);
        let reward_box = outputs[3].clone();
        let ctx = build_context(self_box, outputs, h);
        assert_eq!(reduce(&tree, &ctx), Ok(true), "coinbase must accept");
        assert_eq!(reward_box.value.as_u64(), &miner_share(block_reward(h)));
        assert_eq!(reward_box.creation_height, h);

        let rt = miner_reward_tree(MINER_PK);
        let spend_to = make_output_box(&load_tree(&format!("0008cd{}", MINER_PK)), NANOCOIN, h);
        let at = |hh: u32| build_context(reward_box.clone(), vec![spend_to.clone()], hh);
        // One block early: the delay half is false, so the script is false.
        assert_eq!(reduce(&rt, &at(h + MINER_REWARD_DELAY - 1)), Ok(false));
        assert!(!prove_and_verify(&rt, &at(h + MINER_REWARD_DELAY - 1), vec![miner_secret()]));
        // At maturity: needs the signature, and the miner's secret gives it.
        assert!(reduce(&rt, &at(h + MINER_REWARD_DELAY)).is_err(), "should need a signature");
        assert!(prove_and_verify(&rt, &at(h + MINER_REWARD_DELAY), vec![miner_secret()]));
        // Without a secret, or with another key's secret, it stays locked.
        assert!(!prove_and_verify(&rt, &at(h + MINER_REWARD_DELAY), vec![]));
        let mut k2 = [0u8; 32];
        k2[31] = 2;
        let other = PrivateInput::DlogProverInput(DlogProverInput::from_bytes(&k2).unwrap());
        assert!(!prove_and_verify(&rt, &at(h + MINER_REWARD_DELAY + 10_000), vec![other]));
    }

    #[test]
    fn coinbase_rejects_unbound_miner_output() {
        let h = 100u32;
        let (tree, self_box, outputs, h) = coinbase_scenario(h, 200 * NANOCOIN, h - 1);
        let m = miner_share(block_reward(h));
        let ctx = build_context(self_box.clone(), outputs.clone(), h);
        assert_eq!(reduce(&tree, &ctx), Ok(true), "twin must accept");
        let p2pk = load_tree(&format!("0008cd{}", MINER_PK));
        for (what, bad_box) in [
            ("plain P2PK", make_output_box(&p2pk, m, h)),
            ("other key", make_output_box(&miner_reward_tree(OTHER_PK), m, h)),
            ("treasury accumulation", make_output_box(&load_tree(TREASURY_ACCUM_TREE_HEX), m, h)),
            ("creation height 0", make_output_box(&miner_reward_tree(MINER_PK), m, 0)),
            ("value + 1", make_output_box(&miner_reward_tree(MINER_PK), m + 1, h)),
        ] {
            let mut o = outputs.clone();
            o[3] = bad_box;
            let ctx = build_context(self_box.clone(), o, h);
            assert_eq!(reduce(&tree, &ctx), Ok(false), "{} must be script-false", what);
        }
    }

    #[test]
    fn coinbase_header_key_decides_recipient() {
        // The same outputs are valid only in a block whose header key is the
        // one in the miner script; a tx pre-built for one key fails in a
        // block mined by another.
        let h = 100u32;
        let (tree, self_box, outputs, h) = coinbase_scenario(h, 200 * NANOCOIN, h - 1);
        let ok = build_context_pk(self_box.clone(), outputs.clone(), h, MINER_PK);
        assert_eq!(reduce(&tree, &ok), Ok(true));
        let other = build_context_pk(self_box, outputs, h, OTHER_PK);
        assert_eq!(reduce(&tree, &other), Ok(false));
    }

    #[test]
    fn terminal_coinbase_binds_miner_output() {
        // Terminal at value == reward (tail height, contract case): treasury
        // accum, LP accum, bound miner output.
        let h = MAX_HALVINGS * BLOCKS_PER_HALVING + 49;
        let emission_tree = load_tree(EMISSION_TREE_HEX);
        let t_tree = load_tree(TREASURY_ACCUM_TREE_HEX);
        let l_tree = load_tree(LP_ACCUM_TREE_HEX);
        let self_box = make_emission_box(
            &emission_tree, MIN_REWARD, &proposition_hash(&t_tree), &proposition_hash(&l_tree), h - 1,
        );
        let (t, l) = (treasury_reward(MIN_REWARD), lp_reward(MIN_REWARD));
        let m = MIN_REWARD - t - l;
        let good = vec![
            make_output_box(&t_tree, t, h),
            make_output_box(&l_tree, l, h),
            make_miner_box(m, h),
        ];
        let ctx = build_context(self_box.clone(), good.clone(), h);
        assert_eq!(reduce(&emission_tree, &ctx), Ok(true));
        let mut bad = good.clone();
        bad[2] = make_output_box(&load_tree(DUMMY_TREE_HEX), m, h);
        let ctx = build_context(self_box.clone(), bad, h);
        assert_eq!(reduce(&emission_tree, &ctx), Ok(false), "unbound terminal miner output");
        let ctx = build_context_pk(self_box, good, h, OTHER_PK);
        assert_eq!(reduce(&emission_tree, &ctx), Ok(false), "terminal under another header key");
    }

    #[test]
    fn emission_tree_pins_miner_binding() {
        let bytes = hex_to_bytes(EMISSION_TREE_HEX);
        assert_eq!(bytes.len(), 702);
        assert_eq!(bytes[616], 0x90, "terminal comparison must still be LE");
        assert_eq!(&bytes[617..621], &[0x72, 0x04, 0x72, 0x07]);
        let both = format!("0e07{}0e0e{}", MINER_SCRIPT_PREFIX_HEX, MINER_SCRIPT_SUFFIX_HEX);
        assert!(EMISSION_TREE_HEX.contains(&both));
        let mut hh = Blake2b256::new();
        hh.update(&bytes);
        let digest: String = hh.finalize().iter().map(|b| format!("{:02x}", b)).collect();
        assert_eq!(digest, EMISSION_TREE_HASH);
    }
}

// ============================================================
// TEST 11: EMISSION -> ACCUMULATION -> FUND PIPELINE (v2.0)
// ============================================================

#[cfg(test)]
mod test11_pipeline {
    use super::*;

    /// Coinbase at h1 and h2 pays both accumulation scripts; after the 20,160-block
    /// cooldown (v2.1, was 720) an unsigned keeper merges each stream; the signers then forward the
    /// merged box into the NFT holder, which must carry its value plus the sum.
    #[test]
    fn coinbase_to_fund_end_to_end() {
        let (h1, h2) = (1_000u32, 1_001u32);
        let (tree, s1, o1, _) = coinbase_scenario(h1, 200 * NANOCOIN, h1 - 1);
        assert_accepts(&tree, &build_context(s1, o1.clone(), h1), "coinbase 1");
        let (tree, s2, o2, _) = coinbase_scenario(h2, 200 * NANOCOIN, h2 - 1);
        assert_accepts(&tree, &build_context(s2, o2.clone(), h2), "coinbase 2");

        for (accum_hex, idx, holder) in [
            (TREASURY_ACCUM_TREE_HEX, 1usize, treasury_gov_box as fn(u64, u32) -> ErgoBox),
            (LP_ACCUM_TREE_HEX, 2usize, lp_fund_box as fn(u64, u32) -> ErgoBox),
        ] {
            let at = load_tree(accum_hex);
            let (a, b) = (o1[idx].clone(), o2[idx].clone());
            let sum = *a.value.as_u64() + *b.value.as_u64();
            // Keeper merge before the cooldown: refused.
            let early = h2 + 100;
            let ctx = build_context_multi(vec![a.clone(), b.clone()], 0, vec![make_output_box(&at, sum, early)], early);
            assert_script_false(&at, &ctx, "merge inside the cooldown");
            // After the cooldown: accepted for every input.
            let hm = h2 + 20_161;
            let merged = make_output_box(&at, sum, hm);
            for i in 0..2 {
                let ctx = build_context_multi(vec![a.clone(), b.clone()], i, vec![merged.clone()], hm);
                assert_accepts(&at, &ctx, "keeper merge");
            }
            // Signed forward of the fresh merged box (no cooldown on the transfer path).
            let hf = hm + 1;
            let fund = holder(500 * NANOCOIN, 100);
            let gov_tree = fund.ergo_tree.clone();
            let succ = holder(500 * NANOCOIN + sum, hf);
            let ins = vec![fund, merged.clone()];
            assert_accepts(&at, &build_context_multi(ins.clone(), 1, vec![succ.clone()], hf), "forward, accumulation side");
            assert_signed(&gov_tree, &build_context_multi(ins.clone(), 0, vec![succ], hf), "forward, fund side");
            let short = holder(500 * NANOCOIN + sum - 1, hf);
            assert_script_false(&at, &build_context_multi(ins, 1, vec![short], hf), "forward one nano short");
        }
    }

    /// F-009 with the real accumulation scripts: a padded share output is refused.
    #[test]
    fn coinbase_rejects_padded_share_output() {
        let h = 100u32;
        let (tree, s, outs, _) = coinbase_scenario(h, 200 * NANOCOIN, h - 1);
        for i in [1usize, 2] {
            let mut o = outs.clone();
            let pad = std::collections::HashMap::from([(NonMandatoryRegisterId::R4, Constant::from(vec![0u8; 3_000]))]);
            o[i] = make_box(&outs[i].ergo_tree, *outs[i].value.as_u64(), vec![], pad, h);
            assert_script_false(&tree, &build_context(s.clone(), o, h), "padded share output");
        }
    }
}

// ============================================================
// TEST 12: EA-N1 WITH THE REAL DAO TREASURY TREE (v2.2)
// ============================================================

#[cfg(test)]
mod test12_ea_n1_dao_treasury {
    use super::*;

    /// The DAO treasury.es tree as the DAO tests compile it (treasury NFT dd..dd,
    /// counter token ee..ee).
    const DAO_TREASURY_TREE_HEX: &str = include_str!("../trees/dao_treasury_tree.hex");
    const DAO_TREASURY_NFT: [u8; 32] = [0xDD; 32];
    /// blake2b256 of treasury_accumulation.es compiled with TREASURY=dd..dd
    /// (`compile/gen_trees.py --ids TREASURY=dd..dd`, v2.2).
    const ACCUM_DD_TREE_HASH: &str = "a86c1bdfb01eafa05fa6ae5a0bfa9df625428a5e4fdcb458f6fe411d5a88ee26";

    /// The treasury accumulation tree with the DAO treasury NFT id. The id is a
    /// segregated constant, so swapping its bytes gives the compiler's output for
    /// that id; the hash pin checks that.
    fn accum_dd_tree() -> ErgoTree {
        let from = format!("0e20{}", hex_of(&TREASURY_NFT));
        let to = format!("0e20{}", hex_of(&DAO_TREASURY_NFT));
        let hex = TREASURY_ACCUM_TREE_HEX.trim();
        assert_eq!(hex.matches(&from).count(), 1, "NFT id appears once in the accumulation tree");
        let t = load_tree(&hex.replace(&from, &to));
        assert_eq!(hex_of(&proposition_hash(&t)), ACCUM_DD_TREE_HASH, "matches the --ids TREASURY=dd..dd compile");
        t
    }

    /// EA-N1: [T treasury.es NFT 100 YOLO, d tokenless at treasury.es 50 YOLO,
    /// a accumulation 50 YOLO] -> [T' 150 YOLO, attacker 50 YOLO]. T passes
    /// DEPOSIT and d passes MERGE (each needs only T' >= T + its own box); the
    /// v2.2 accumulation script needs T + d + a and refuses. The honest
    /// [T' = 200 YOLO] passes all three; one nano short fails the accumulation box.
    #[test]
    fn ea_n1_dao_merge_plus_forward_skim_rejected() {
        let dao = load_tree(DAO_TREASURY_TREE_HEX);
        let acc_tree = accum_dd_tree();
        let h = 30_000u32;
        let t = make_box(&dao, 100 * NANOCOIN, vec![nft(DAO_TREASURY_NFT)], Default::default(), 100);
        let d = make_output_box(&dao, 50 * NANOCOIN, 100);
        let a = make_output_box(&acc_tree, 50 * NANOCOIN, 100);
        let ins = vec![t, d, a];
        let succ = |v: u64| make_box(&dao, v, vec![nft(DAO_TREASURY_NFT)], Default::default(), h);

        let skim = vec![succ(150 * NANOCOIN), make_output_box(&load_tree(DUMMY_TREE_HEX), 50 * NANOCOIN, h)];
        assert_accepts(&dao, &build_context_multi(ins.clone(), 0, skim.clone(), h), "T: DEPOSIT holds on the skim");
        assert_accepts(&dao, &build_context_multi(ins.clone(), 1, skim.clone(), h), "d: MERGE holds on the skim");
        assert_script_false(&acc_tree, &build_context_multi(ins.clone(), 2, skim, h), "a: accumulation refuses the skim");

        let honest = vec![succ(200 * NANOCOIN)];
        for i in 0..3 {
            let tree = if i == 2 { &acc_tree } else { &dao };
            assert_accepts(tree, &build_context_multi(ins.clone(), i, honest.clone(), h), "honest T' = T + d + a");
        }
        let short = vec![succ(200 * NANOCOIN - 1)];
        assert_script_false(&acc_tree, &build_context_multi(ins, 2, short, h), "one nano short of T + d + a");
    }
}
