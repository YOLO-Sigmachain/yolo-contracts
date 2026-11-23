# EDGE_CASES.md — Emission Contract v1.1

**Contract:** `emission.es` v1.1 (Ergo-parity pass applied)
**Validated against:** `emission_model.py` (reference oracle) and `contract_sim.py` (faithful Python simulator of the ErgoScript contract)
**Test suite:** `test_emission_exhaustive.py`: **97 tests, all passing**; Rust `tests/emission_test.rs`: **63 tests** against the compiled v1.2 tree, 638 bytes, blake2b256 `56e7731dccd7e69237cb6869a4e494ead714a8af30fc9d5820aaccf00ee74a11` (v1.1 plus the 2026-11 review F-009 share-output pins; v1.1 was the 577-byte tree `06b9279ac71ac96d023032613e1fda77469c054424a3615db8bb0877c6ef9a4e`, which includes the 2026-11-10 terminal `<=` fix and the 2026-11-10 miner output binding; in v1.2 the LE guard is at offset 552). The previous 455-byte tree `630f1337dd39bdea14dfd83bff6c02e4312292e98680b174a1f33cc84ee0d095` is the same source without the miner binding.
**Ergo cross-reference:** `ERGO_COMPARISON.md` — line-by-line comparison against `emissionBoxProp` in `ErgoScriptPredef.scala` (commit `bd1906e`)
**Schedule (30 s retune, locked 2026-11-09):** 1,051,200 blocks per halving (365-day year at 30 s); six arms 75, 37.5, 18.75, 9.375, 4.6875, 2.34375 YOLO, then a 1.5 YOLO tail; total supply 177,291,450 YOLO = 6 arm years + 14 tail years (20 chain years). Since 2026-11-12 the genesis emission box holds 177,291,250 YOLO and the treasury and LP fund boxes hold 100 YOLO each; the real box is spent out at height 21,023,916 (last normal spend at 21,023,915 from 2.5 YOLO, terminal spend from 1.0 YOLO, below the 1.5 reward). The sixth arm removed from the 20 s schedule by the June 2026 ".5 fix" is restored (1,051,200 is divisible by 64, so 2.34375 × 1,051,200 = 2,463,750 YOLO is integral).

---

## v1.1 changes from v1.0

Applied after cross-referencing against Ergo's canonical emission contract:

1. **[Finding 1 — matches Ergo]** Treasury and LP value checks tightened from `>=` to strict `==`. Ergo uses `EQ(coinsToIssue, Minus(ExtractAmount(Self), ExtractAmount(rewardOut)))` — strict equality on the emission-box delta. Our recipient-value checks now mirror that convention in both `normalPath` and `terminalPath`.

2. **[Gap A — added for Ergo parity]** `heightIncreased = HEIGHT > SELF.creationInfo._1` added as a contract-wide precondition. Mirrors Ergo's `GT(Height, boxCreationHeight(Self))`. Redundant with our singleton-NFT design, kept for defense-in-depth.

3. **[Gap B — added for Ergo parity]** `heightCorrect = nextBox.creationInfo._1 == HEIGHT` added to the normal path's `validEmissionBox` check. Mirrors Ergo's `EQ(boxCreationHeight(rewardOut), Height)`. Locks the successor emission box to the current block.

3a. **[Gap C, added 2026-11-10]** Miner output bound in the contract, as Ergo's `correctMinerOutput` does, with a 2,880-block (24 h) reward delay instead of Ergo's 720. The last output must have the script `rewardOutputScript(2880, CONTEXT.minerPubKey)`, creation height `HEIGHT` and exactly the miner share; the output count is 4 on the normal path and 3 on the terminal path. See L1.

### Findings that did NOT trigger changes

4. **[Finding 2]** Proposed `MIN_BOX_VALUE` check on successor: **NOT ADDED.** Ergo doesn't include this either — they handle exhaustion via the `lastCoins` bypass, which is structurally equivalent to our terminal path.

5. **[Finding 3]** Dead-code floor clamp in reward lookup: **KEPT.** Ergo uses a different reward formula (linear subtraction with natural zero-floor), so the practice doesn't translate. Our lookup-table structure benefits from the defensive clamp.

6. **[Finding 4]** Zero-value terminal path acceptance: **NO CHANGE.** Ergo's `lastCoins` bypass allows equivalent "anything goes at the end" behavior. Consensus `MIN_BOX_VALUE` makes zero-value boxes unreachable in practice.

See `ERGO_COMPARISON.md` for the full analysis.

---

## Coverage summary (v1.1)

| Category | Coverage |
|---|---|
| Halving boundaries (H−1, H, H+1) | Epochs 0 through 19 — **59 distinct heights** exercised |
| Terminal path entry | 5 scenarios: `remaining = 0`, `remaining = reward−1`, `remaining = MIN_REWARD/2`, realistic walk, same-block-spend reject |
| Rounding at each epoch | Sum-to-reward invariant at every epoch 0–24; specific nanocoin values validated at epochs 0–6 |
| Normal-path rejects | 23 reject scenarios (includes 6 new for v1.1: overpay × 2, heightIncreased × 2, heightCorrect × 2) |
| Terminal-path rejects | 11 reject scenarios (includes 3 new for v1.1: overpay × 2, same-block-spend × 1) |
| Miner output binding (L1) | Accepts at all 7 reward levels on both paths, at every halving boundary (H−1, H, H+1), at value == reward on a tail height and at the real-chain terminal spend 21,023,916, with 4 header keys; 15 reject kinds (table below; Rust `phase13_miner_binding`, Python `MinerBindingTests`), each script-false with an accepting twin except the two-output terminal case |
| Cross-oracle | Python simulator's `block_reward` matches `emission_model.block_reward` at every boundary and at 326 sampled heights spanning 25 epochs |

---

## Handled edge cases

### 1. Halving boundary transitions — handled, tested
The reward changes only at `H = epoch × BLOCKS_PER_HALVING`, never mid-epoch. It is not a clean `2r` to `r` at every boundary: at boundary 6 it drops from 2.34375 to 1.5 YOLO, and from boundary 7 on it does not change (already at the floor). Tested epochs 0–19 at H−1, H, H+1.

### 2. Floor / min-reward clamp — handled, tested
Epoch 5: `INITIAL_REWARD / 32 = 2,343,750,000` (above `MIN_REWARD = 1,500,000,000`). Epoch 6+: `else minReward` branch returns exactly `MIN_REWARD`. Outer clamp is dead code for enumerated branches (defensive).

### 3. Rounding preservation — handled, tested
Treasury: `reward × 10 / 100`, LP: `reward × 5 / 100`, miner absorbs residual. At every reward level (75, 37.5, 18.75, 9.375, 4.6875, 2.34375, 1.5 coins), split is exact.

### 4. Strict equality on recipient values — **new in v1.1**
`OUTPUTS(1).value == treasuryReward` and `OUTPUTS(2).value == lpReward` (not `>=`). Same for terminal path. Matches Ergo's `EQ` convention. Miner cannot voluntarily overpay treasury/LP — any deviation rejects.

### 5. Terminal path — handled, tested
Triggers when `SELF.value <= blockReward` (`<` until 2026-11-10, see L3). Strict `==` on `termTreasury` and `termLP`. Emission NFT must be burned (no output carries it): `.forall` across all outputs. Same-block spend rejected via `heightIncreased`.

### 6. Terminal with zero remaining — handled, design decision documented
`termTreasury = 0`, `termLP = 0`, so `== 0` trivially passes for zero-value outputs. Consensus `MIN_BOX_VALUE` prevents reaching this state. Matches Ergo's equivalent behavior under `lastCoins`.

### 7. Normal / terminal mutual exclusion — handled, tested
Since the `<=` fix, `sufficientFunds` (`>=`) and `insufficient` (`<=`) are both true at `SELF.value == blockReward`, so the value guards alone no longer separate the paths at that one value. The paths are still exclusive for any one transaction because they need opposite NFT placement: the normal path needs the emission NFT in `OUTPUTS(0)`, the terminal path forbids it in every output. At that value the normal shape also needs a 0-value successor, which consensus rejects, so the terminal spend is the only usable one.

### 8. NFT preservation (normal) and burn (terminal) — handled, tested
Normal: `OUTPUTS(0)` preserves NFT. Terminal: no output may carry NFT anywhere (asymmetric `.forall` enforcement). Tested with NFT smuggled to 3 different output positions.

### 9. Register preservation — handled, tested
`OUTPUTS(0).R4` and `OUTPUTS(0).R5` must equal `SELF.R4` and `SELF.R5`.

### 10. Script preservation — handled, tested
`OUTPUTS(0).propositionBytes == SELF.propositionBytes`.

### 11. Output ordering — handled, tested
Normal: emission / treasury / LP / miner (exactly 4 outputs). Terminal: treasury / LP / miner (exactly 3). Swapping treasury ↔ LP rejects under strict `==`. The miner output is read as `OUTPUTS(OUTPUTS.size - 1)` and must be last.

### 12. Height checks — **new in v1.1**
`heightIncreased`: `HEIGHT > SELF.creationInfo._1`. `heightCorrect`: `OUTPUTS(0).creationInfo._1 == HEIGHT`. Matches Ergo's belt-and-suspenders height enforcement.

### 13. Overflow — handled, verified
Largest intermediate: `reward × 85 = 6.375×10¹²` nanocoins (model; the contract itself only computes `reward × 10`). i64 headroom: 1.45M×. Genesis box value `1.773×10¹⁷`, headroom: 52×.

### 14. Monotonic, bounded rewards — handled, tested
Non-increasing in height. Always within `[MIN_REWARD, INITIAL_REWARD]`.

---

## Known limitations / design decisions

### L1. Miner output binding (RESOLVED in contract, 2026-11-10)

Until 2026-11-10 the miner share (85% on the normal path, the remainder on the terminal path) was unconstrained by the script and by consensus in the stock Scala node, and an emission spend needs no signature. The block producer chose where it went, and the contract header wrongly said a consensus layer enforced it.

**Fix.** emission.es now binds the miner output the way Ergo's `emissionBoxProp` does (`correctMinerOutput` at `ErgoTreePredef.scala`:90-93), with a reward delay of 2,880 blocks (24 h at 30 s) instead of Ergo's 720:

```
val minerRewardScript: Coll[Byte] =
  fromBase16("100204802d08cd") ++ CONTEXT.minerPubKey ++
  fromBase16("ea02d192a39a8cc7a70173007301")
val minerOut: Box = OUTPUTS(OUTPUTS.size - 1)
val minerOutBound: Boolean =
  minerOut.propositionBytes == minerRewardScript &&
  minerOut.creationInfo._1 == HEIGHT
// normal path
val validMiner: Boolean =
  OUTPUTS.size == 4 && minerOut.value == blockReward - treasuryReward - lpReward
// terminal path
val validTermMiner: Boolean =
  OUTPUTS.size == 3 && minerOut.value == remaining - termTreasury - termLP
sigmaProp(heightIncreased && minerOutBound && (normalPath || terminalPath))
```

**The script bytes.** `rewardOutputScript(delay, pk)` serializes to 54 bytes: `10 02` (header, 2 constants), `04 <delay>` (Int), `08 cd <33-byte pk>` (SigmaProp ProveDlog), then the body `ea02d192a39a8cc7a70173007301` (`proveDlog(pk) && HEIGHT >= SELF.creationInfo._1 + delay`). The delay is `a0 0b` for 720 and `80 2d` for 2,880. Derived by calling `ErgoTreePredef.rewardOutputScript` on the JVM with sigma-state 6.0.7 and 6.0.2 for six keys and both delays: the two versions agree, only the 33 pk bytes vary between keys, only the two delay bytes vary between delays, and Scala's own `substituteConstants` (what Ergo's `expectedMinerOutScriptBytesVal` uses) gives the same bytes as prefix ++ pk ++ suffix for every key tried. The 720 form matches a real mainnet reward box (block 1,786,180). Ergo builds the bytes with `substConstants`; plain concatenation is used here because it gives the same bytes for any valid 33-byte key, is cheaper, and does not re-parse a tree inside the script.

**Why the creation height is pinned.** The delay script reads `SELF.creationInfo._1` of the reward box itself. A miner output declaring creation height 0 would be spendable at once. Pinning it to `HEIGHT` gives the full 2,880 blocks. Test `reward_script_delay_counts_from_box_creation_height` shows the reward script reduces to false at `ch + 2879`, to `proveDlog(pk)` at `ch + 2880`, and to `proveDlog(pk)` at once for a box backdated to 0.

**Why the miner output is the last output.** The compiler hoists every repeated `OUTPUTS(i)` read into a top-level `ValDef`, evaluated eagerly on both paths. A fixed `OUTPUTS(3)` would be read on the 3-output terminal path too and fail there (mutation M9 below: every terminal accept breaks). `OUTPUTS(OUTPUTS.size - 1)` is one read that exists on both paths; the count checks fix it at index 3 (normal) and 2 (terminal).

**Is the output-count pin needed?** Not for the emission value. On either path the pinned outputs sum to exactly `SELF.value` (normal: `(SELF.value − R) + t + l + (R − t − l)`; terminal: `t + l + (v − t − l)`). So with the emission box as the only input, value conservation leaves nothing for an extra output, and an extra output can only carry value from an extra input, which is not emission value. The pin is kept anyway: it matches Ergo's `outputsNum` on its normal branch, it makes the layout exact for node builders, it makes script-only tests decisive, and together with the exact values it forces the emission tx to have exactly one input (any extra input adds value with nowhere to go). That last point also closes the token-donation route in L3.

**Adversarial answers.**

1. *Can anyone other than the block's miner obtain part of an emission spend?* No. Anyone can build an emission spend (no signature), but every output is pinned: successor, treasury and LP as before, and the miner output to the reward script of `CONTEXT.minerPubKey`, which is the public key in the header of the block that includes the tx. A tx built for key A is script-false in a block whose header names key B (tests `reject_reward_script_of_any_other_key` over all 12 ordered pairs of 4 keys, both paths; 05 `coinbase_header_key_decides_recipient`). Swapping the key in someone else's mined header does not help: Autolykos v2 hashes the header without its PoW solution (`AutolykosPowScheme.msgByHeader`, ergo `AutolykosPowScheme.scala`:239; `HeaderSerializer.bytesWithoutPow`), and the solution is where the key sits, but the emission tx (committed through the transactions root, which is in the hashed part) names the original key, so after a swap the script is false. Reasoned from source, not tested here.
2. *Can a miner route its share to a contract or another key?* Not to a contract: the script must be the exact 54-byte reward script (tests: arbitrary contract, plain P2PK of the miner, delays 0, 1, 720, 2879, 2881). To another key, only by putting that key in the header; the share then lands in that key's 2,880-block reward box. That is the accepted cost: an on-chain payout contract (for example a decentralized pool such as Lithos) cannot receive the share directly from the emission tx; it can only be paid from the reward box after maturity, by the key holder.
3. *Mining pools.* A pool sets the header key to its own key (the node's configured mining key), receives every share in its own reward boxes, and pays its miners off-chain after 2,880 blocks, as on Ergo (720 blocks there). Pool key in the header is accepted in every accept test (`PK_POOL`).
4. *Terminal block and halving boundaries.* The binding is height-independent. Tested on the real tree at all seven reward levels on both paths, at H−1, H, H+1 for every boundary 1 to 6 (including a neighbouring level's share rejecting), at value == reward on a tail height, and at the real-chain terminal spend 21,023,916 (1.0 YOLO box; 21,024,049 before the 2026-11-12 genesis split).
5. *Does `CONTEXT.minerPubKey` behave the same in sigma-rust and Scala?* The tree reads it as `PropertyCall(SContext, method 10)` (bytes `db 65 0a fe`), not the `MinerPubkey` global opcode. sigma-rust 0.28: `MINER_PUBKEY_EVAL_FN` returns `pre_header.miner_pk` serialized, 33 bytes SEC1 compressed (identity as 33 zero bytes). Scala: `CContext.minerPubKey` is `preHeader.minerPk.getEncoded` (`ErgoLikeContext.scala`:171), `GroupElementSerializer.toBytes`, also 33 bytes compressed. Checked by running the compiled tree in both interpreters: the Rust tests, and a scratch harness on the sigma-state 6.0.7 JIT interpreter (62 scenarios: 21 normal accepts at all levels with 4 keys, 15 terminal accepts, 25 script-false rejects covering wrong key, header key, P2PK, delay 720, contract script, value ±1, creation height, index, missing, siphon, split and NFT in the miner output, and the two-output terminal evaluation error) with the same outcome as sigma-rust in every case. Not checked against a running ergo node.
6. *NFT burn and extra tokens.* The miner output's tokens are not constrained. At the terminal step `nftBurned` still forbids the emission NFT in every output, the miner output included (test `miner_output_tokens_and_nft_interaction`, Python `test_reject_nft_in_bound_miner_output_at_terminal`); on the normal path moving the NFT to the miner output fails `nftPreserved`. Other tokens: with exactly one input (see the count pin above) nobody can donate tokens into the emission box any more; a builder can still mint a new token in the emission tx (token id = emission box id) into any output, and the script lets such a token be dropped later. An honest builder burns nothing but the emission NFT at the terminal step; the script does not enforce that for other tokens.
7. *Cost and size.* Tree 455 to 577 bytes (+122), constants 32 to 37 (prefix, suffix, the counts 4 and 3, and the 1 in `OUTPUTS.size - 1`). Scala JIT `fullReduction` cost on accepting spends: normal 174 to 225-235, terminal 157 to 210 (sigma-state 6.0.7; `MaxBlockCostDefault` is 1,000,000 in ergo `Parameters.scala`:318).
8. *Half the miners know a trick?* There is no unequal trick left: every miner faces the same 24 h lock and cannot shorten it (creation height pinned) or redirect the share (script pinned). A miner can only choose the header key, including a key whose secret is public, which gives away its own share and nothing else.
9. *Identity-point key.* A header whose miner key is the identity point (33 zero bytes) makes the miner's own share spendable by anyone after the delay. Only the block producer can choose that key, so it can only give away its own share, same as Ergo.
10. *Pool proxy with its own key.* A pool proxy that submits a solution carrying its own public key, different from the key the candidate's emission transaction pays, produces a block whose emission transaction is script-false. Pool setups must submit solutions without a key or with the candidate's key, same as Ergo.

**Node work this creates (not contract):** the candidate builder must emit exactly [successor, treasury, LP, miner] and [treasury, LP, miner] with the miner output at `rewardOutputScript(2880, pk)` and creation height = block height, and every mining node must run with `minerRewardDelay = 2880` (E8). A node whose emission builder uses a 720 delay cannot include the emission transaction: the stock Scala candidate generator drops the rejected transaction, so its blocks are valid but carry no coinbase when the mempool has other transactions, and it produces no candidate when the mempool is empty. The YOLO emission builder should hard-code 2880 for the coinbase miner output rather than read the node's fee-delay setting, assert the script template at startup, and log an error whenever the emission transaction is rejected during block assembly. The node wallet recognises rewards by `rewardOutputScript(miningRewardDelay, pk)` (`WalletCache.scala`:52), so 2880 there too.

**Mutation checks.** Each new clause was removed or weakened in the source, recompiled and run against the Rust suite: script check removed (M1), creation height check removed (M2) or weakened to `>= HEIGHT - 1` (M11), `size == 4` removed (M3), `size == 3` removed (M4), normal value check removed (M5) or weakened to `>=` (M10), terminal value check removed (M6), delay prefix 720 (M7), `minerOutBound` dropped from the root (M8), fixed `OUTPUTS(3)` index (M9). Every mutant fails at least one non-pin test. Same for the Python simulator (7 mutants, all caught).

Tests: Rust `phase0_load::{emission_tree_pins_miner_binding, reward_script_template_matches_scala_reward_output_script, reward_script_delay_counts_from_box_creation_height}` and `phase13_miner_binding` (16 tests); 05 `test10_miner_binding` (5 tests); Python `MinerBindingTests` (15 tests).

### L2. Zero-value terminal path accepts — acceptable
See handled-case #6.

### L3. New emission box at consensus min-box-value boundary — not checked by contract
When `SELF.value − blockReward < MIN_BOX_VALUE`, the new emission box is consensus-invalid on mainnet. Contract accepts the spend; consensus rejects it. A terminal-shaped TX accepts only when `SELF.value <= blockReward`.

**RESOLVED in contract (2026-11-10).** Found in the 30 s retune. With `insufficient = SELF.value < blockReward`, when `0 ≤ SELF.value − blockReward < MIN_BOX_VALUE` neither path was usable: the normal successor is consensus-invalid and the terminal path rejected. With the genesis box of the time (177,291,450 YOLO), height 0 is never mined, so the real box still held 75 YOLO at height 21,024,000 (end of year 20); 49 tail blocks took it to exactly 1.5 YOLO = one reward, and the box spent at height 21,024,049 could not be spent by the contract. It would have sat until miner-only storage rent applied, treasury and LP would have received none of it, and the miner taking the last of it would also have taken the emission NFT. The 20 s schedule had the same shape (50 YOLO residual, 1 YOLO tail).

Fix: `insufficient = SELF.value <= blockReward` (one-byte tree change, LT `0x8f` to LE `0x90` at offset 389; size stays 455 bytes; blake2b256 `24e1d32c...3f51` to `630f1337dd39bdea14dfd83bff6c02e4312292e98680b174a1f33cc84ee0d095`). This also matches the `LE` in Ergo's `lastCoins` bypass. The general case `0 < SELF.value − blockReward < MIN_BOX_VALUE` is still not checked by the contract, but it is unreachable: the genesis value and all seven reward levels are multiples of 31,250,000 nano (93,750,000 for the 177,291,450 YOLO box before 2026-11-12), so every reachable box value is too, and the gap is either 0 (handled) or at least 31,250,000 nano (above any minimum box value smaller than that). The schedule view (a box holding the full total supply, height 0 paid) reaches value == reward at 21,023,999; `<=` covers that case too.

**2026-11-12 update (genesis split).** The genesis emission box now holds 177,291,250 YOLO; 100 YOLO each went to the treasury and LP fund boxes, total supply unchanged. The real box no longer lands on the reward. The last normal spend is at 21,023,915 from a 2.5 YOLO box (successor 1.0 YOLO, consensus-valid); the terminal spend is at 21,023,916 from a 1.0 YOLO box, which is less than the 1.5 reward, so `insufficient` holds under `<` as well as `<=`. Split: treasury 100,000,000, LP 50,000,000, miner 850,000,000 nano. The `<=` guard stays (the contract still accepts value == reward, and the tests keep that case as a contract case at a tail height), but the real chain no longer depends on it.

**Second-order: what the last-block miner can do** (written for a box at value == reward; on the real chain since 2026-11-12 the last box holds 1.0 YOLO, and the terminal spend pays the miner 0.85 YOLO, less than a normal 1.275 YOLO tail share, with treasury and LP taking 10% and 5% of the 1.0 YOLO).
- With `<`: no spend is both script-valid and consensus-valid. The miner can do nothing with the box.
- With `<=`: the only consensus-valid spend is the terminal one, paying treasury 150,000,000, LP 75,000,000 and leaving 1,275,000,000 nano to the miner. That is exactly the normal split of a 1.5 YOLO tail block, so there is no new extraction. The terminal split depends only on `SELF.value`, so at value == reward it equals the normal split at every reward level, and it is integral at every level and for every reachable value (10% and 5% of 31,250,000 are whole).
- It cannot fire early: on reachable values, `SELF.value <= blockReward` holds only when one reward is left. `SELF.value == blockReward + 1` and `== 2 × blockReward` reject.
- Emission NFT: the terminal path requires that no output carries it, so it is burned (it is in no output). A miner keeping it rejects. There is no emission box afterwards.
- Other tokens: the normal path checks only `tokens(0)` of the successor. Before the miner binding a miner could add tokens of their own to the emission box; since 2026-11-10 the emission tx can have only one input (L1), so the only extra tokens possible are ones minted in the emission tx itself. The terminal path constrains only the emission NFT, so such a token may be dropped (burned) or taken by the last-block miner. At genesis the box holds only the NFT; no protocol value is at risk.
- Output count: since 2026-11-10 the terminal path requires exactly 3 outputs (treasury, LP, miner) and the normal path exactly 4. The tree also binds `OUTPUTS(2)` in a shared top-level `ValDef` (used by the normal path's LP check), so a two-output terminal tx fails evaluation in both sigma-rust and Scala instead of returning false (tested: `reject_missing_miner_output`). Either way it rejects.

Tests: Rust `phase12_year20_exhaustion::terminal_accepts_when_value_equals_reward` (was `terminal_rejects_when_value_equals_reward`), `terminal_at_value_equals_reward_rejects_nft_kept_by_miner`, `terminal_at_value_equals_reward_still_strict_on_splits`, `terminal_rejects_one_nano_above_reward`, `terminal_rejects_at_twice_reward`, `terminal_at_value_equals_reward_matches_normal_split_every_level`, `terminal_leaves_extra_emission_box_tokens_unconstrained`, `phase0_load::emission_tree_pins_terminal_le_fix` (now pins the 577-byte tree: LE at offset 505); 05 `test9_retune_30s::real_chain_last_block_terminal_coinbase_accepts` (value == reward, now a contract case); Python `TerminalLeFixTests`. Real-chain terminal since 2026-11-12: Rust `phase12_year20_exhaustion::{real_chain_last_normal_spend_at_21_023_915, real_chain_terminal_spend_at_21_023_916}` and `model_tests::real_box_runs_out_at_21_023_916`; 05 `test9_retune_30s::real_chain_terminal_coinbase_at_21_023_916`; Python `test_real_chain_box_runs_out_before_end_of_year_20` (was `test_real_chain_residual_after_year_20`), `TerminalLeFixTests::test_real_chain_last_normal_and_terminal_spend`, and a full real-chain walk from 21,023,800 to exhaustion: 116 normal blocks, last normal at 21,023,915 (2.5 to 1.0 YOLO), terminal at 21,023,916, box fully spent, total paid since height 1 equals the genesis box value, plus the two fund boxes equals the total supply.

**Still OPEN (node work, not contract):** a node's candidate builder needs a terminal-path emission tx (treasury, LP, miner bound as in L1; no successor; NFT burned) for the block where `SELF.value <= blockReward`, and a post-exhaustion mode that builds blocks with no emission tx once the emission box is gone. A builder that treats a failed or missing emission tx as fatal would stop producing blocks at height 21,023,916 (21,024,049 before the 2026-11-12 genesis split). The terminal block's box holds 1.0 YOLO, less than the reward, so a builder must size the terminal split from `SELF.value`, not from the reward.

### L4. HEIGHT semantics — standard
`HEIGHT` is the block being spent. Correct for coinbase TX. Double-spend prevented by UTXO model.

### L5. Block-time changes alter real-time halving schedule — by design, documented
Halvings in blocks, not seconds. Same as Bitcoin.

---

## Reject cases exhaustively tested

### Normal path rejects (23 tests, all passing)

| # | Mutation | Contract check that catches it |
|---|---|---|
| 1 | Treasury underpaid by 1 | `validTreasury: OUTPUTS(1).value == treasuryReward` |
| 2 | Treasury overpaid by 1 ✨NEW | same (strict `==`) |
| 3 | Treasury overpaid by 1 ERG ✨NEW | same |
| 4 | LP underpaid by 1 | `validLP: OUTPUTS(2).value == lpReward` |
| 5 | LP overpaid by 1 ✨NEW | same |
| 6 | Treasury to wrong script | `blake2b256(...) == treasuryScriptHash` |
| 7 | LP to wrong script | `blake2b256(...) == lpScriptHash` |
| 8 | Emission box value off by +1 | `valueCorrect` |
| 9 | Emission box value off by −1 | same |
| 10 | Emission NFT missing | `nftPreserved` |
| 11 | Emission NFT wrong ID | same |
| 12 | Emission NFT wrong amount | same |
| 13 | New emission box different script | `scriptPreserved` |
| 14 | R4 changed in new emission box | `registersPreserved` |
| 15 | R5 changed in new emission box | same |
| 16 | OUTPUTS(0) is not an emission box | `nftPreserved` + `scriptPreserved` fail |
| 17 | Treasury/LP outputs swapped | `validTreasury` (value mismatch) |
| 18 | Treasury output missing | out-of-bounds |
| 19 | LP output missing | out-of-bounds |
| 20 | Same-block spend ✨NEW | `heightIncreased` |
| 21 | HEIGHT < creation_height ✨NEW | same |
| 22 | Successor creation_height = H−1 ✨NEW | `heightCorrect` |
| 23 | Successor creation_height = H+1 ✨NEW | same |

### Miner output rejects (2026-11-10, both paths unless noted; all script-false with an accepting twin)

| # | Mutation | Contract check that catches it |
|---|---|---|
| 1 | Miner script for another key than the header key (all 12 ordered pairs of 4 keys) | `minerOutBound` (script) |
| 2 | Header key differs from the key the tx was built for | same |
| 3 | Plain P2PK of the miner, no delay | same |
| 4 | Reward script with delay 720 (Ergo mainnet) | same |
| 5 | Reward script with delay 0, 1, 2879, 2881 | same |
| 6 | Arbitrary contract as miner output | same |
| 7 | Miner value one nano low / high | `validMiner` / `validTermMiner` |
| 8 | Miner creation height 0, H−1, H+1, H−2880 | `minerOutBound` (height) |
| 9 | Miner output at index 0, 1 or 2 (normal), 0 or 1 (terminal) | script of the last output, or treasury/LP checks |
| 10 | Miner output missing (normal: 3 outputs; terminal: third output not a miner box) | `minerOutBound`, output count |
| 11 | Extra output siphoning value (before the miner short by x, before the miner exact, after the miner, extra reward box) | output count, value, last-output script |
| 12 | Miner share split across two outputs | output count, value |
| 13 | Neighbouring reward level's share at each halving boundary (normal) | `validMiner` |
| 14 | Emission NFT in the bound miner output (terminal) / moved to it (normal) | `nftBurned` / `nftPreserved` |
| 15 | Two-output terminal tx | evaluation error (hoisted `OUTPUTS(2)`), not false |

### Terminal path rejects (14 tests, all passing; rows 12 to 14 added 2026-11-10)

| # | Mutation | Contract check |
|---|---|---|
| 1 | Terminal treasury underpaid | `== termTreasury` |
| 2 | Terminal treasury overpaid ✨NEW | same |
| 3 | Terminal LP underpaid | `== termLP` |
| 4 | Terminal LP overpaid ✨NEW | same |
| 5 | Terminal treasury wrong script | `blake2b256(...) == R4` |
| 6 | Terminal LP wrong script | `blake2b256(...) == R5` |
| 7 | NFT smuggled to OUTPUTS(0) | `nftBurned` |
| 8 | NFT smuggled to OUTPUTS(1) | same |
| 9 | NFT smuggled to appended output | same |
| 10 | Terminal-shape when normal valid | `insufficient` fails |
| 11 | Terminal same-block spend ✨NEW | `heightIncreased` |
| 12 | Terminal at value == reward + 1 nano | `insufficient` fails |
| 13 | Terminal at value == 2 × reward | `insufficient` fails |
| 14 | Terminal at value == reward, NFT kept by miner | `nftBurned` |

---

## Remaining recommendations (optional, non-blocking)

1. **Simplify the lookup's dead-code floor clamp** — outer `if (computed > minReward) ...` is only reachable via `else minReward` branch. Can drop for ~3 ErgoTree bytes saved. Or keep as defense.

2. **Run sigma-rust harness on rustc 1.85+ machine** — see `emission_test.rs`. Python simulator tests same logic contract expresses; ErgoTree eval confirms deterministic compilation behaves identically.

3. ~~Document the miner-output design difference (L1).~~ Superseded 2026-11-10: the miner output is bound in the contract (L1). Node builders and `minerRewardDelay` must follow it (L1, node work).

---

## Verdict

Coverage criteria:
- [x] **Exhaustively tested at every halving boundary** — 59 boundary heights, epochs 0–19, 3 offsets each.
- [x] **Terminal path** — entry, steady state, 5 accept scenarios, 11 reject scenarios including NFT burn at 3 positions and same-block-spend.
- [x] **Rounding at each epoch** — sum-to-reward invariant across epochs 0–24.
- [x] **Ergo-parity review completed** — `ERGO_COMPARISON.md`. 3 adjustments applied (strict `==`, heightIncreased, heightCorrect), plus the 2026-11-10 miner output binding (Gap C, delay 2,880).

Outstanding: on-chain sigma-rust evaluation on a developer machine with rustc 1.85+.
