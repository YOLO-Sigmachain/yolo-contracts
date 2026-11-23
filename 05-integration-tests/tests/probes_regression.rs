//! probes_regression.rs: the eight probe tests of the 2026-11 contract review,
//! ported against the v2.0 trees and updated for
//! v2.1: a governance successor (OUTPUTS(0) at the fund script) must now be created
//! at HEIGHT - 10 or later (HEIGHT - 720 since v2.2) and keep min(SELF.value,
//! 100 YOLO), and the merge
//! cooldown is 20,160 blocks, so those outputs are built at the context height and
//! the payouts leave the 100 YOLO floor.
//!
//! Each "genuine" case was an attack that the review reproduced; here it must be
//! rejected. Each "control" case keeps its role. Where the honest shape had to
//! change for v2.0, the comment says how. Results print as in the original probes
//! (run with `-- --nocapture --test-threads=1` to see them):
//!   TRUE / FALSE   the script's result
//!   SIGMA          the boolean part held; only the 2-of-3 signature remains
//!   ERROR          evaluation error (also a reject)
//! Every assertion names the exact outcome.
use std::collections::HashMap;
use std::sync::Arc;
use ergo_lib::ergo_chain_types::{Digest32, Header, PreHeader};
use ergo_lib::ergotree_interpreter::{eval::{context::Context, reduce_to_crypto}, sigma_protocol::prover::ContextExtension};
use ergo_lib::ergotree_ir::{
    chain::{ergo_box::{box_value::BoxValue, ErgoBox, ErgoBoxCandidate, NonMandatoryRegisterId as R, NonMandatoryRegisters},
        token::{Token, TokenAmount, TokenId}, tx_id::TxId},
    ergo_tree::ErgoTree,
    mir::{constant::{Constant, Literal}, value::CollKind},
    serialization::SigmaSerializable,
    sigma_protocol::sigma_boolean::SigmaBoolean,
    types::{stuple::STuple, stype::SType},
};
use sigma_test_util::force_any_val;

fn from_hex(h: &str) -> ErgoTree {
    let h = h.trim();
    let b: Vec<u8> = (0..h.len()).step_by(2).map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap()).collect();
    ErgoTree::sigma_parse_bytes(&b).unwrap()
}
fn tree(n: &str) -> ErgoTree {
    from_hex(match n {
        "ta" => include_str!("../../02-treasury-tests/trees/treasury_accumulation.hex"),
        "tg" => include_str!("../../02-treasury-tests/trees/treasury_governance.hex"),
        "la" => include_str!("../../03-lp-fund-tests/trees/lp_accumulation.hex"),
        "lf" => include_str!("../../03-lp-fund-tests/trees/lp_fund.hex"),
        _ => unreachable!(),
    })
}
fn t_true() -> ErgoTree { from_hex("10010101d17300") }
fn mk_at(t: ErgoTree, v: u64, toks: Vec<Token>, regs: HashMap<R, Constant>, ch: u32) -> ErgoBox {
    ErgoBox::from_box_candidate(&ErgoBoxCandidate { value: BoxValue::try_from(v).unwrap(), ergo_tree: t,
        tokens: if toks.is_empty() { None } else { Some(toks.try_into().unwrap()) },
        additional_registers: NonMandatoryRegisters::new(regs).unwrap(), creation_height: ch }, TxId::zero(), 0).unwrap()
}
/// Boxes at creation height 1, as in the original probes.
fn mk(t: ErgoTree, v: u64, toks: Vec<Token>, regs: HashMap<R, Constant>, _idx: u16) -> ErgoBox { mk_at(t, v, toks, regs, 1) }
fn ctx(self_idx: usize, ins: &[ErgoBox], outs: &[ErgoBox], h: i64) -> Context<'static> {
    let ins: Vec<&'static ErgoBox> = ins.iter().cloned().map(|b| &*Box::leak(Box::new(b))).collect();
    Context { height: h as u32, self_box: ins[self_idx], outputs: Vec::leak(outs.to_vec()),
        data_inputs: None, inputs: ins.try_into().unwrap(),
        pre_header: force_any_val::<PreHeader>(), headers: force_any_val::<[Header; 10]>(), extension: ContextExtension::empty() }
}
fn red(t: &ErgoTree, c: &Context) -> Result<SigmaBoolean, String> {
    reduce_to_crypto(&t.proposition().unwrap(), c).map(|r| r.sigma_prop).map_err(|e| format!("{e:?}"))
}
fn show(label: &str, r: &Result<SigmaBoolean, String>) -> &'static str {
    let k = match r { Ok(SigmaBoolean::TrivialProp(true)) => "TRUE", Ok(SigmaBoolean::TrivialProp(false)) => "FALSE",
        Ok(_) => "SIGMA", Err(_) => "ERROR" };
    println!("{label}: {k}{}", if let Err(e) = r { format!(" [{}]", &e[..e.len().min(90)]) } else { String::new() });
    k
}
const N: u64 = 1_000_000_000;
const MIN: u64 = 1_000_000;
const D: i64 = 8_640;
/// v2.0 trees are compiled with these NFT ids; the probes' nft(7) / nft(9) are now
/// just foreign tokens.
const TNFT: u8 = 0xAA;
const LNFT: u8 = 0xBB;
fn nft(b: u8) -> Token { Token { token_id: TokenId::from(Digest32::from([b; 32])), amount: TokenAmount::try_from(1u64).unwrap() } }
fn none() -> HashMap<R, Constant> { HashMap::new() }
fn b2b(bytes: &[u8]) -> Vec<u8> {
    use blake2::{digest::consts::U32, Blake2b, Digest};
    let mut h = Blake2b::<U32>::new(); h.update(bytes); h.finalize().to_vec()
}
fn th(t: &ErgoTree) -> Vec<u8> { b2b(&t.sigma_serialize_bytes().unwrap()) }
/// treasury governance registers: R4 commitment, R5 approval height, R6 recipient script, R7 amount, R8 flag
fn tg_regs4(r4: Vec<u8>, r5: i64, r6: Vec<u8>, r7: i64, r8: i64) -> HashMap<R, Constant> {
    HashMap::from([(R::R4, Constant::from(r4)), (R::R5, Constant::from(r5)), (R::R6, Constant::from(r6)),
        (R::R7, Constant::from(r7)), (R::R8, Constant::from(r8))])
}
/// The original helper (R4 = 32 zero bytes).
fn tg_regs(r5: i64, r6: Vec<u8>, r7: i64, r8: i64) -> HashMap<R, Constant> { tg_regs4(vec![0u8; 32], r5, r6, r7, r8) }
fn wl_const(wl: &[(Vec<u8>, i64)]) -> Constant {
    let pair = SType::STuple(STuple::pair(SType::SColl(Arc::new(SType::SByte)), SType::SLong));
    let items: Vec<Literal> = wl.iter().map(|(h, a)| {
        let c1: Constant = h.clone().into(); let c2: Constant = (*a).into();
        Literal::Tup(vec![c1.v, c2.v].try_into().unwrap())
    }).collect();
    Constant { tpe: SType::SColl(Arc::new(pair.clone())), v: Literal::Coll(CollKind::WrappedColl { elem_tpe: pair, items: items.into() }) }
}
/// LP fund registers v2.0: R4 whitelist of (hash, height added), R5 flag, R6 migration height, R7 target hash
fn lf_regs_wl(wl: &[(Vec<u8>, i64)], r5: i64, r6: i64, r7: Vec<u8>) -> HashMap<R, Constant> {
    HashMap::from([(R::R4, wl_const(wl)), (R::R5, Constant::from(r5)), (R::R6, Constant::from(r6)), (R::R7, Constant::from(r7))])
}
/// The original helper's whitelist [1..1] (added at 0), R7 empty.
fn lf_regs(r5: i64, r6: i64) -> HashMap<R, Constant> { lf_regs_wl(&[(vec![1u8; 32], 0)], r5, r6, vec![]) }
fn any_out(v: u64, i: u16) -> ErgoBox { mk(t_true(), v, vec![], none(), i) }

// ---------- F-001: transfer branch has no value floor ----------
#[test]
fn f001_transfer_dust_to_governance_script_rest_to_builder() {
    for (acc_n, gov_n, id) in [("ta", "tg", TNFT), ("la", "lf", LNFT)] {
        let (acc, gov) = (tree(acc_n), tree(gov_n));
        let a = mk(acc.clone(), 100 * N, vec![], none(), 0);
        let b = mk(acc.clone(), 100 * N, vec![], none(), 1);
        // GENUINE: dust box at the governance script, everything else to an arbitrary output -> now FALSE
        let outs = vec![mk(gov.clone(), MIN, vec![], none(), 0), any_out(200 * N - MIN, 1)];
        for i in 0..2 {
            let r = red(&acc, &ctx(i, &[a.clone(), b.clone()], &outs, 100));
            assert_eq!(show(&format!("F-001 genuine {acc_n} input {i} (dust forward)"), &r), "FALSE");
        }
        // CONTROL (discriminates): first output at an unrelated script is rejected
        let r = red(&acc, &ctx(0, &[a.clone()], &[any_out(100 * N, 0)], 100));
        assert_eq!(show(&format!("F-001 control {acc_n} (output at an unrelated script)"), &r), "FALSE");
        // The original honest control (full value into a tokenless box at the governance
        // script) is the F-003 lock and is now FALSE.
        let r = red(&acc, &ctx(0, &[a.clone()], &[mk(gov.clone(), 100 * N, vec![], none(), 0)], 100));
        assert_eq!(show(&format!("F-001 old control {acc_n} (full value into a tokenless box)"), &r), "FALSE");
        // CONTROL (honest, new shape): [NFT holder, accumulation box] -> [holder successor + value]
        let holder = mk(t_true(), 50 * N, vec![nft(id)], none(), 0);
        let succ = mk(t_true(), 150 * N, vec![nft(id)], none(), 0);
        let r = red(&acc, &ctx(1, &[holder, a.clone()], &[succ], 100));
        assert_eq!(show(&format!("F-001 control {acc_n} (forward into the NFT holder)"), &r), "TRUE");
    }
}

// ---------- F-002: consolidation floor is per input ----------
#[test]
fn f002_consolidation_keeps_only_the_largest_input() {
    // v2.0 consolidation also needs every input older than the cooldown and the merged
    // box created within 10 blocks of HEIGHT (F-025): the probe height moves from 100
    // to 30,000 (v2.1 cooldown 20,160; was 1,000 with 720) and the output is created
    // at the probe height.
    let h = 30_000i64;
    for acc_n in ["ta", "la"] {
        let acc = tree(acc_n);
        let a = mk(acc.clone(), 100 * N, vec![], none(), 0);
        let b = mk(acc.clone(), 60 * N, vec![], none(), 1);
        let c = mk(acc.clone(), 40 * N, vec![], none(), 2);
        // GENUINE: three inputs (200), one accumulation output of 100, the other 100 elsewhere -> now FALSE
        let outs = vec![mk_at(acc.clone(), 100 * N, vec![], none(), h as u32), any_out(100 * N, 1)];
        for i in 0..3 {
            let r = red(&acc, &ctx(i, &[a.clone(), b.clone(), c.clone()], &outs, h));
            assert_eq!(show(&format!("F-002 genuine {acc_n} input {i} (output = largest input)"), &r), "FALSE");
        }
        // CONTROL (discriminates): output below the largest input is rejected for that input
        let outs = vec![mk_at(acc.clone(), 99 * N, vec![], none(), h as u32), any_out(101 * N, 1)];
        let r = red(&acc, &ctx(0, &[a.clone(), b.clone(), c.clone()], &outs, h));
        assert_eq!(show(&format!("F-002 control {acc_n} input 0 (output below it)"), &r), "FALSE");
        // CONTROL (honest): full sum merged passes
        let outs = vec![mk_at(acc.clone(), 200 * N, vec![], none(), h as u32)];
        for i in 0..3 {
            let r = red(&acc, &ctx(i, &[a.clone(), b.clone(), c.clone()], &outs, h));
            assert_eq!(show(&format!("F-002 control {acc_n} input {i} (honest merge of the sum)"), &r), "TRUE");
        }
    }
}

// ---------- F-003: a tokenless box at a governance script cannot be spent ----------
#[test]
fn f003_tokenless_governance_box_is_unspendable() {
    // treasury governance. The well-formed control now needs the compiled NFT id (aa).
    let tg = tree("tg");
    let payee = t_true().sigma_serialize_bytes().unwrap();
    let good = mk(tg.clone(), 500 * N, vec![nft(TNFT)], tg_regs(0, payee.clone(), 0, 0), 0);
    // v2.1: successors are created at the probe height (20,000).
    let succ = |v: u64, r5: i64, r8: i64| mk_at(tg.clone(), v, vec![nft(TNFT)], tg_regs(r5, payee.clone(), 0, r8), 20_000);
    // CONTROL: a well-formed box does spend on consolidation (2-of-3 remain), now with one output too
    for outs in [vec![succ(500 * N, 0, 0), any_out(MIN, 1)], vec![succ(500 * N, 0, 0)]] {
        let r = red(&tg, &ctx(0, &[good.clone()], &outs, 20_000));
        assert_eq!(show(&format!("F-003 control tg (well-formed box, consolidation, {} outputs)", outs.len()), &r), "SIGMA");
    }
    // GENUINE: tokenless, registerless box; try every output layout the paths accept
    let bare = mk(tg.clone(), 500 * N, vec![], none(), 0);
    let layouts: Vec<(&str, Vec<ErgoBox>, i64)> = vec![
        ("consolidation", vec![succ(500 * N, 0, 0), any_out(MIN, 1)], 20_000),
        ("approval", vec![succ(500 * N, 20_000, 0), any_out(MIN, 1)], 20_000),
        ("freeze", vec![succ(500 * N, 0, 1), any_out(MIN, 1)], 20_000),
        ("migration approval", vec![succ(500 * N, 20_000, 2), any_out(MIN, 1)], 20_000),
        ("migration execute", vec![mk(t_true(), 500 * N, vec![nft(TNFT)], tg_regs(0, payee.clone(), 0, 0), 0), any_out(MIN, 1)], 20_000),
        ("plain spend", vec![any_out(500 * N, 0), any_out(MIN, 1)], 20_000),
    ];
    for (name, outs, h) in &layouts {
        let r = red(&tg, &ctx(0, &[bare.clone()], outs, *h));
        assert_eq!(show(&format!("F-003 genuine tg bare box, {name} layout"), &r), "ERROR");
    }
    // typed registers but no token: still rejected, now as FALSE (SELF must hold the NFT)
    let regs_only = mk(tg.clone(), 500 * N, vec![], tg_regs(0, payee.clone(), 0, 0), 0);
    for (name, outs, h) in &layouts {
        let r = red(&tg, &ctx(0, &[regs_only.clone()], outs, *h));
        assert_eq!(show(&format!("F-003 genuine tg registers-but-no-token box, {name} layout"), &r), "FALSE");
    }
    // LP fund (v2.0 register layout)
    let lf = tree("lf");
    let lgood = mk(lf.clone(), 500 * N, vec![nft(LNFT)], lf_regs(0, 0), 0);
    let lsucc = |r5: i64, r6: i64| mk_at(lf.clone(), 500 * N, vec![nft(LNFT)], lf_regs(r5, r6), 20_000);
    let r = red(&lf, &ctx(0, &[lgood.clone()], &[lsucc(0, 0)], 20_000));
    assert_eq!(show("F-003 control lf (well-formed box, consolidation)", &r), "SIGMA");
    let lbare = mk(lf.clone(), 500 * N, vec![], none(), 0);
    let lregs = mk(lf.clone(), 500 * N, vec![], lf_regs(0, 0), 0);
    for (name, outs) in [("consolidation", vec![lsucc(0, 0)]), ("freeze", vec![lsucc(1, 0)]), ("migration approval", vec![lsucc(2, 20_000)]),
                         ("plain spend", vec![any_out(500 * N, 0)])] {
        let r = red(&lf, &ctx(0, &[lbare.clone()], &outs, 20_000));
        assert_eq!(show(&format!("F-003 genuine lf bare box, {name} layout"), &r), "ERROR");
        let r = red(&lf, &ctx(0, &[lregs.clone()], &outs, 20_000));
        assert_eq!(show(&format!("F-003 genuine lf registers-but-no-token box, {name} layout"), &r), "FALSE");
    }
}

// ---------- F-004: cancel keeps the migration flag, so the delay is skipped ----------
#[test]
fn f004_migration_approve_cancel_execute_without_waiting() {
    let tg = tree("tg");
    let target_tree = t_true();
    let commit = th(&target_tree);
    // v2.0 migration approval records the target hash in R4, R6 empty, R7 0.
    let gb = |r5: i64, r8: i64| mk(tg.clone(), 500 * N, vec![nft(TNFT)], tg_regs4(commit.clone(), r5, vec![], 0, r8), 0);
    let h = 10_000i64; // above 8,640, as the finding requires
    // v2.1: governance successors are created at h.
    let gbo = |r5: i64, r8: i64| mk_at(tg.clone(), 500 * N, vec![nft(TNFT)], tg_regs4(commit.clone(), r5, vec![], 0, r8), h as u32);
    // step 1: migration approval at h  (R8 0 -> 2, R5 = h)
    let r = red(&tg, &ctx(0, &[gb(0, 0)], &[gbo(h, 2), any_out(MIN, 1)], h));
    assert_eq!(show("F-004 step 1 migration approval at h=10000", &r), "SIGMA");
    let target = mk(target_tree.clone(), 500 * N, vec![nft(TNFT)], tg_regs(0, vec![], 0, 0), 0); // the recorded target, carries the NFT
    // CONTROL: migration execute straight after approval, same height: rejected (delay enforced)
    let r = red(&tg, &ctx(0, &[gb(h, 2)], &[target.clone(), any_out(MIN, 1)], h));
    assert_eq!(show("F-004 control: execute right after approval at h=10000", &r), "FALSE");
    let r = red(&tg, &ctx(0, &[gb(h, 2)], &[target.clone(), any_out(MIN, 1)], h + D));
    assert_eq!(show("F-004 control: execute at approval + 8640 (boundary)", &r), "FALSE");
    // CONTROL (honest): execute after the delay passes
    let r = red(&tg, &ctx(0, &[gb(h, 2)], &[target.clone(), any_out(MIN, 1)], h + D + 1));
    assert_eq!(show("F-004 control: execute at approval + 8641 (honest)", &r), "SIGMA");
    // step 2 GENUINE: cancellation at h keeping R8 = 2 -> now FALSE
    let r = red(&tg, &ctx(0, &[gb(h, 2)], &[gbo(0, 2), any_out(MIN, 1)], h));
    assert_eq!(show("F-004 step 2 cancellation at h=10000 keeping R8=2", &r), "FALSE");
    // honest cancellation now writes R8 = 0
    let r = red(&tg, &ctx(0, &[gb(h, 2)], &[gbo(0, 0), any_out(MIN, 1)], h));
    assert_eq!(show("F-004 step 2 (fixed) cancellation writing R8=0", &r), "SIGMA");
    // step 3 GENUINE: migration execute from R5 = 0, R8 = 2 at the same height -> now FALSE
    let r = red(&tg, &ctx(0, &[gb(0, 2)], &[target.clone(), any_out(MIN, 1)], h));
    assert_eq!(show("F-004 step 3 migration execute at h=10000 (no wait)", &r), "FALSE");
    // and the value need not follow the NFT -> now FALSE (and the value floor applies)
    let thin = mk(target_tree.clone(), MIN, vec![nft(TNFT)], tg_regs(0, vec![], 0, 0), 0);
    let r = red(&tg, &ctx(0, &[gb(0, 2)], &[thin.clone(), any_out(500 * N - MIN, 1)], h));
    assert_eq!(show("F-004 step 3 variant: NFT box at minimum value, funds elsewhere", &r), "FALSE");
    let r = red(&tg, &ctx(0, &[gb(h, 2)], &[thin, any_out(500 * N - MIN, 1)], h + D + 1));
    assert_eq!(show("F-004 step 3 variant after the delay: value floor", &r), "FALSE");
}

// ---------- F-023: hand-built governance box with an old approval height ----------
#[test]
fn f023_lookalike_governance_box_executes_at_once() {
    let (ta, tg) = (tree("ta"), tree("tg"));
    let payee = t_true().sigma_serialize_bytes().unwrap();
    let amount = 150 * N as i64;
    let mut pre = payee.clone(); pre.extend_from_slice(&amount.to_be_bytes());
    let phash = b2b(&pre); // consistent proposal hash, so only the NFT check can stop it
    // v2.1: the box holds 300 (was 200), so the change after paying 150 stays above
    // the 100 YOLO floor, and the change is created at the probe height.
    let look = |r5: i64| mk(tg.clone(), 300 * N, vec![nft(42)], tg_regs4(phash.clone(), r5, payee.clone(), amount, 0), 0);
    // step 1 GENUINE: anyone forwards two accumulation boxes into the hand-built box -> now FALSE
    let a = mk(ta.clone(), 100 * N, vec![], none(), 0);
    let b = mk(ta.clone(), 100 * N, vec![], none(), 1);
    for i in 0..2 {
        let r = red(&ta, &ctx(i, &[a.clone(), b.clone()], &[look(1)], 10_000));
        assert_eq!(show(&format!("F-023 step 1 accumulation input {i} forwards into the hand-built box"), &r), "FALSE");
    }
    // step 2 GENUINE: execution on that box in the next block, R5 = 1 -> now FALSE
    let fee = from_hex("1005040004000e36100204a00b08cd0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798ea02d192a39a8cc7a701730073011001020402d19683030193a38cc7b2a57300000193c2b2a57301007473027303830108cdeeac93b1a57304");
    let outs = vec![mk_at(tg.clone(), 150 * N - MIN, vec![nft(42)], tg_regs4(phash.clone(), 0, payee.clone(), amount, 0), 10_001),
                    any_out(150 * N, 1), mk(fee, MIN, vec![], none(), 2)];
    let r = red(&tg, &ctx(0, &[look(1)], &outs, 10_001));
    assert_eq!(show("F-023 step 2 execution at h=10001 on a box created with R5=1", &r), "FALSE");
    // CONTROL: the same box with a recent approval height is rejected until the delay passes
    let r = red(&tg, &ctx(0, &[look(10_000)], &outs, 10_001));
    assert_eq!(show("F-023 control: same box with R5=10000 at h=10001", &r), "FALSE");
    // CONTROL (discriminates): the same execution from a box holding the real NFT passes
    let real = |r5: i64| mk(tg.clone(), 300 * N, vec![nft(TNFT)], tg_regs4(phash.clone(), r5, payee.clone(), amount, 0), 0);
    let mut routs = outs.clone();
    routs[0] = mk_at(tg.clone(), 150 * N - MIN, vec![nft(TNFT)], tg_regs4(phash.clone(), 0, payee.clone(), amount, 0), 10_001);
    let r = red(&tg, &ctx(0, &[real(1)], &routs, 10_001));
    assert_eq!(show("F-023 control: real NFT, R5=1, at h=10001 (inside the window)", &r), "SIGMA");
}

// ---------- NV-1: what every governance spend needs on its outputs ----------
#[test]
fn nv1_output_register_typing() {
    let tg = tree("tg");
    let target_tree = t_true();
    let gb = |r5: i64, r8: i64| mk(tg.clone(), 500 * N, vec![nft(TNFT)], tg_regs4(th(&target_tree), r5, vec![], 0, r8), 0);
    let h = 30_000i64;
    // v2.1: a consolidation successor is created at h.
    let gbo = |r5: i64, r8: i64| mk_at(tg.clone(), 500 * N, vec![nft(TNFT)], tg_regs4(th(&target_tree), r5, vec![], 0, r8), h as u32);
    // migration execute to a target WITHOUT registers: was ERROR, now passes (F-024)
    let bare_target = mk(target_tree.clone(), 500 * N, vec![nft(TNFT)], none(), 0);
    let r = red(&tg, &ctx(0, &[gb(h - D - 1, 2)], &[bare_target.clone(), any_out(MIN, 1)], h));
    assert_eq!(show("NV-1 tg migration execute, target without registers", &r), "SIGMA");
    let r = red(&tg, &ctx(0, &[gb(h - D - 1, 2)], &[bare_target], h));
    assert_eq!(show("NV-1 tg migration execute, target without registers, ONE output", &r), "SIGMA");
    // treasury consolidation with a single output: was ERROR, now passes
    let r = red(&tg, &ctx(0, &[gb(0, 0)], &[gbo(0, 0)], h));
    assert_eq!(show("NV-1 tg consolidation with ONE output", &r), "SIGMA");
    let r2 = red(&tg, &ctx(0, &[gb(0, 0)], &[gbo(0, 0), any_out(MIN, 1)], h));
    assert_eq!(show("NV-1 tg consolidation with TWO outputs", &r2), "SIGMA");
    // LP fund migration execute, target with and without typed registers
    let lf = tree("lf");
    let lfb = mk(lf.clone(), 500 * N, vec![nft(LNFT)], lf_regs_wl(&[(vec![1u8; 32], 0)], 2, h - D - 1, th(&target_tree)), 0);
    let r = red(&lf, &ctx(0, &[lfb.clone()], &[mk(target_tree.clone(), 500 * N, vec![nft(LNFT)], lf_regs(0, 0), 0)], h));
    assert_eq!(show("NV-1 lf migration execute, target with typed R4-R7", &r), "SIGMA");
    let r = red(&lf, &ctx(0, &[lfb.clone()], &[mk(target_tree.clone(), 500 * N, vec![nft(LNFT)], none(), 0)], h));
    assert_eq!(show("NV-1 lf migration execute, target without registers", &r), "SIGMA");
}

// ---------- F-005: whitelist entry added and paid at the same height ----------
fn lf_box(t: &ErgoTree, v: u64, wl: &[(Vec<u8>, i64)], r5: i64, r6: i64) -> ErgoBox {
    mk(t.clone(), v, vec![nft(LNFT)], lf_regs_wl(wl, r5, r6, vec![]), 0)
}
/// v2.1: the same box as a successor, created at `ch`.
fn lf_out(t: &ErgoTree, v: u64, wl: &[(Vec<u8>, i64)], ch: i64) -> ErgoBox {
    mk_at(t.clone(), v, vec![nft(LNFT)], lf_regs_wl(wl, 0, 0, vec![]), ch as u32)
}
#[test]
fn f005_whitelist_update_then_spend_same_height() {
    let lf = tree("lf");
    let dest = th(&t_true());
    let h = 20_000i64;
    let old = vec![(vec![1u8; 32], 0i64)];
    // v2.0: the new entry is stamped with the height it is added at.
    let new = vec![(vec![1u8; 32], 0i64), (dest.clone(), h)];
    // CONTROL: before the update, a spend to the unlisted script is rejected
    // v2.1: the spends leave the 100 YOLO floor (was 1,000,000 nano).
    let r = red(&lf, &ctx(0, &[lf_box(&lf, 500 * N, &old, 0, 0)], &[lf_out(&lf, 100 * N, &old, h), any_out(400 * N, 1)], h));
    assert_eq!(show("F-005 control: spend to an unlisted script", &r), "FALSE");
    // step 1: whitelist update adds the script, same height
    let r = red(&lf, &ctx(0, &[lf_box(&lf, 500 * N, &old, 0, 0)], &[lf_out(&lf, 500 * N, &new, h)], h));
    assert_eq!(show("F-005 step 1 whitelist update at h=20000", &r), "SIGMA");
    // step 2 GENUINE: spend everything but the floor to the new entry, same height -> now FALSE
    let spend = |hh: i64| red(&lf, &ctx(0, &[lf_box(&lf, 500 * N, &new, 0, 0)], &[lf_out(&lf, 100 * N, &new, hh), any_out(400 * N, 1)], hh));
    assert_eq!(show("F-005 step 2 spend of all but the floor to the new entry at h=20000", &spend(h)), "FALSE");
    assert_eq!(show("F-005 step 2 at h + 8640", &spend(h + D)), "FALSE");
    // CONTROL (honest): the same spend after the per-entry delay passes
    assert_eq!(show("F-005 control: same spend at h + 8641", &spend(h + D + 1)), "SIGMA");
    // GENUINE variant: add the entry with an old height instead of now -> FALSE at the update
    let backdated = vec![(vec![1u8; 32], 0i64), (dest, 1)];
    let r = red(&lf, &ctx(0, &[lf_box(&lf, 500 * N, &old, 0, 0)], &[lf_out(&lf, 500 * N, &backdated, h)], h));
    assert_eq!(show("F-005 variant: entry added with height 1", &r), "FALSE");
}

// ---------- F-019: co-spent accumulation value need not reach the governance successor ----------
#[test]
fn f019_consolidation_ignores_co_input_value() {
    let (ta, tg) = (tree("ta"), tree("tg"));
    let payee = t_true().sigma_serialize_bytes().unwrap();
    // v2.1: successors (and SELF, harmlessly) are created at the probe height.
    let gb = |v: u64| mk_at(tg.clone(), v, vec![nft(TNFT)], tg_regs(0, payee.clone(), 0, 0), 20_000);
    let tab = mk(ta.clone(), 100 * N, vec![], none(), 1);
    // GENUINE: [governance box 500, accumulation box 100] -> [successor 500, arbitrary output 100]
    let ins = [gb(500 * N), tab.clone()];
    let outs = [gb(500 * N), any_out(100 * N, 1)];
    let r = red(&tg, &ctx(0, &ins, &outs, 20_000));
    // The governance side on its own is still a valid consolidation ...
    assert_eq!(show("F-019 genuine: governance consolidation, successor at the old value", &r), "SIGMA");
    // ... but the co-spent accumulation box now refuses, so the transaction is invalid.
    let r = red(&ta, &ctx(1, &ins, &outs, 20_000));
    assert_eq!(show("F-019 genuine: the co-spent accumulation box", &r), "FALSE");
    // CONTROL: successor below the old governance value is rejected
    let outs = [gb(499 * N), any_out(101 * N, 1)];
    let r = red(&tg, &ctx(0, &ins, &outs, 20_000));
    assert_eq!(show("F-019 control: successor below the old value", &r), "FALSE");
    // CONTROL (honest): successor carries the sum
    let outs = [gb(600 * N), any_out(MIN, 1)];
    let r = red(&tg, &ctx(0, &ins, &outs, 20_000));
    assert_eq!(show("F-019 control: successor carries the sum (honest), governance side", &r), "SIGMA");
    let r = red(&ta, &ctx(1, &ins, &outs, 20_000));
    assert_eq!(show("F-019 control: successor carries the sum (honest), accumulation side", &r), "TRUE");
}
