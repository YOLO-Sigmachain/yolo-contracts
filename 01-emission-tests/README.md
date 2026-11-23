# Emission contract: tests (v1.1)

Test suite for the `emission.es` contract. Every other contract assumes this one works, so it is tested exhaustively at every halving boundary, on the terminal path, and for the rounding at each epoch.

And a follow-up correctness pass cross-referencing every finding against Ergo's canonical `emissionBoxProp` contract from `ErgoScriptPredef.scala` (commit `bd1906e`).

## TL;DR

- **97 tests passing** (62 at v1.1 close-out, 63 after the June 2026 .5 fix, 73 after the 30 s retune, 82 after the 2026-11-10 terminal fix, 97 after the 2026-11-10 miner output binding) against a line-by-line Python simulator of the ErgoScript contract. Rust `tests/emission_test.rs`: 63 tests against the compiled v1.2 tree, 638 bytes, blake2b256 `56e7731dccd7e69237cb6869a4e494ead714a8af30fc9d5820aaccf00ee74a11` (v1.1 was 577 bytes, `06b9279a...9a4e`). v1.2 (2026-11 review, F-009): the four treasury and LP share outputs must carry no tokens, no registers and creation height HEIGHT. The tree is written to `trees/emission.hex` by `05-integration-tests/compile/gen_trees.py`. The Python simulator does not model share-output tokens, registers or creation height.
- **2026-11-12 genesis split:** the genesis emission box holds 177,291,250 YOLO; the other 200 YOLO of the 177,291,450 YOLO total supply sit in the treasury fund box (100) and the LP fund box (100). The reward schedule is unchanged, so the box no longer lands on the reward: the last normal spend is at height 21,023,915 from a 2.5 YOLO box (successor 1.0 YOLO), and the terminal spend is at 21,023,916 from a 1.0 YOLO box, split 40,000,000 / 30,000,000 / 930,000,000 nano (treasury / LP / miner). `emission.es` changed in comments only; the tree is byte-identical. The value == reward terminal case stays tested as a contract case.
- **2026-11-10 terminal fix:** `insufficient` is now `SELF.value <= blockReward` (was `<`). With the genesis box of the time (177,291,450 YOLO) the box spent at height 21,024,049 held exactly one reward; with `<` it could never be spent. One-byte tree change (LT 0x8f to LE 0x90 at offset 389), new blake2b256 `630f1337dd39bdea14dfd83bff6c02e4312292e98680b174a1f33cc84ee0d095`. See `EDGE_CASES.md` L3.
- **2026-11-10 miner output binding (Gap C):** the last output must be `rewardOutputScript(2880, CONTEXT.minerPubKey)` (the stock Ergo miner reward script with a 24 h delay), created at `HEIGHT`, holding exactly the miner share; exactly 4 outputs on the normal path and 3 on the terminal path. Tree 455 to 577 bytes. See `EDGE_CASES.md` L1.
- **3 contract adjustments made after Ergo cross-reference:**
  1. `>=` → `==` on treasury & LP recipient values (matches Ergo's `EQ` on emission consumption)
  2. Added `heightIncreased` check (matches Ergo's `GT(Height, boxCreationHeight(Self))`)
  3. Added `heightCorrect` check (matches Ergo's `EQ(boxCreationHeight(rewardOut), Height)`)
- **59 halving-boundary heights** exercised (H−1, H, H+1 for epochs 0–19).
- **Terminal path** covered: 5 accept scenarios, 11 reject scenarios, plus 9 terminal-fix tests (`TerminalLeFixTests`, 2026-11-10).
- **34 total reject tests** across normal and terminal paths, plus the miner output binding suite (Python `MinerBindingTests` 15, Rust `phase13_miner_binding` 16).
- **sigma-rust template** complete and ready to run locally (needs rustc 1.85+).

## Files

| File | Purpose |
|---|---|
| `emission.es` | Contract v1.1 source (Ergo-parity pass applied). |
| `contract_sim.py` | Python simulator mirroring `emission.es` line-by-line. Diagnostic reasons for every reject. |
| `test_emission_exhaustive.py` | 97-test suite. Run: `python3 test_emission_exhaustive.py` |
| `emission_model.py` | Reference oracle for block reward math (carries the 30 s schedule). |
| `emission_test.rs` | sigma-rust test template with real `Context` construction (v1.1). |
| `Cargo.toml` | Pinned deps for local sigma-rust run. `rust-version = "1.85"`. |
| `ERGO_COMPARISON.md` | Line-by-line comparison vs. Ergo's `emissionBoxProp`. |
| `EDGE_CASES.md` | Coverage matrix, 14 handled edge cases, 5 design decisions, 3 optional recommendations. |

## Run

### Python (covers the checklist below)

```bash
python3 test_emission_exhaustive.py
```

Expected output ends with:
```
Ran 97 tests
OK
TOTAL: 97  FAILURES: 0  ERRORS: 0
```

### Rust (on-chain ErgoTree confirmation — run locally)

```bash
rustup update stable   # need rustc 1.85+
cargo test --release -- --nocapture
```

Place `emission_test.rs` at `tests/emission_test.rs` in your project (or adapt paths).

## What changed v1.0 → v1.1

User-level directive was: *"If a finding matches Ergo's behavior, adjust. If it deviates, investigate further."* After fetching Ergo's canonical emission contract:

| Finding | vs Ergo | Action |
|---|---|---|
| `>=` on treasury/LP values | **DEVIATES** — Ergo uses strict `EQ` | **ADJUSTED → `==`** |
| Add MIN_BOX_VALUE check | Would deviate (Ergo doesn't have it) | No change |
| Remove dead-code floor clamp | Different reward formula | No change (defensive) |
| Zero-value terminal accepts | Equivalent to Ergo's `lastCoins` | No change |

Plus 3 checks found in Ergo's contract that we didn't have:
| Gap | Ergo has | Action |
|---|---|---|
| A. `heightIncreased` | Yes | **ADDED** |
| B. `heightCorrect` | Yes | **ADDED** |
| C. In-contract miner output check | Yes | **ADDED** 2026-11-10 (delay 2,880 instead of 720) |

## Contract findings status (all resolved)

All non-blocking. Fully documented in `EDGE_CASES.md` and `ERGO_COMPARISON.md`.

## Coverage checklist

- [x] Every halving boundary (H−1, H, H+1, epochs 0–19) — 59 heights
- [x] Terminal path — entry, steady-state, NFT-burn (3 positions), same-block reject, reject paths
- [x] Rounding at each epoch — sum-to-reward invariant at every epoch, specific nanocoin values at 0–6
- [x] Reject tests — 34 total (23 normal-path, 11 terminal-path)
- [x] sigma-rust template — complete, needs rustc 1.85+ to run locally
- [x] **Ergo-parity review — completed, 3 adjustments applied**
