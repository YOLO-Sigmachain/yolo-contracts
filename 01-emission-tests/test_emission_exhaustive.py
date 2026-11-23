"""
test_emission_exhaustive.py — Exhaustive test suite for the emission contract.

Uses contract_sim.py as the in-process evaluator. Every test exercises a full
Box/OUTPUTS context and calls evaluate_emission_contract() — not just the
reward math. Reject tests mutate exactly one field and assert rejection.

Coverage targets:
  1. Every halving boundary (H-1, H, H+1) for epochs 1..MAX_HALVINGS
  2. Terminal path entry and steady state
  3. Rounding exactness at each epoch
  4. All normal-path reject cases
  5. All terminal-path reject cases

Also validates contract_sim's block_reward against emission_model.py as a
cross-oracle check.

Run: python3 test_emission_exhaustive.py
"""

import sys
import unittest
from typing import List

import emission_model as model
from contract_sim import (
    NANOCOIN,
    BLOCKS_PER_HALVING,
    INITIAL_REWARD,
    MIN_REWARD,
    MAX_HALVINGS,
    TREASURY_PCT,
    LP_PCT,
    MINER_PCT,
    MIN_BOX_VALUE,
    Box,
    Token,
    blake2b256,
    block_reward_contract,
    split_contract,
    evaluate_emission_contract,
    make_self_box,
    make_next_emission_box,
    make_treasury_box,
    make_lp_box,
    make_normal_spend,
    make_terminal_spend,
    make_miner_box,
    miner_reward_script,
    MINER_REWARD_DELAY,
    MINER_SCRIPT_PREFIX,
    MINER_SCRIPT_PREFIX_720,
    MINER_SCRIPT_SUFFIX,
    PK_A,
    PK_B,
    PK_C,
    PK_POOL,
    EMISSION_SCRIPT_BYTES,
    EMISSION_NFT_ID,
    TREASURY_SCRIPT_BYTES,
    LP_SCRIPT_BYTES,
    TREASURY_HASH,
    LP_HASH,
)


TOTAL_SUPPLY = model.total_supply()        # 177,291,450 coins (schedule total)
GENESIS_VALUE = model.genesis_box_value()  # 177,291,250 coins (emission box, 2026-11-12)
FUND_BOXES = model.GENESIS_TREASURY_FUND_BOX + model.GENESIS_LP_FUND_BOX  # 200 coins

# Real chain (2026-11-12 genesis, height 0 never mined): the last normal
# spend is at 21,023,915 from a 2.5-coin box (successor 1.0); the terminal
# spend is at 21,023,916 from a 1.0-coin box, below the 1.5 reward.
REAL_LAST_NORMAL = 21_023_915
REAL_TERMINAL = 21_023_916

# A tail height for the SELF.value == blockReward terminal case. The real
# box never holds exactly one reward (see REAL_TERMINAL); the case is kept
# as a contract test because the script still accepts it. END + 49 was the
# real-chain terminal height before the 2026-11-12 genesis split.
EQ_H = MAX_HALVINGS * BLOCKS_PER_HALVING + 49


# ============================================================
# CROSS-ORACLE: contract_sim vs emission_model
# ============================================================

class SharePlainTests(unittest.TestCase):
    """emission.es v1.2 (2026-11 review, F-009): the four share outputs carry no
    tokens, no registers and creation height HEIGHT."""

    def _cases(self):
        h = 1_000
        s, outs = make_normal_spend(h, 200 * NANOCOIN)
        yield "normal", s, outs, h, (1, 2)
        s, outs = make_terminal_spend(MIN_REWARD, height=EQ_H)
        yield "terminal", s, outs, EQ_H, (0, 1)
        s, outs = make_terminal_spend(model.real_box_value_at(REAL_TERMINAL), height=REAL_TERMINAL)
        yield "terminal-real", s, outs, REAL_TERMINAL, (0, 1)

    def test_twins_accept(self):
        for name, s, outs, h, _ in self._cases():
            ok, why = evaluate_emission_contract(s, outs, h, TREASURY_HASH, LP_HASH)
            self.assertTrue(ok, f"{name}: {why}")

    def test_reject_token_register_or_height(self):
        import copy
        for name, s, outs, h, idx in self._cases():
            for i in idx:
                for what in ("token", "register", "height-1", "height+1"):
                    o = copy.deepcopy(outs)
                    if what == "token":
                        o[i].tokens = [Token(b"\x5a" * 32, 1000)]
                    elif what == "register":
                        o[i].r4 = b"\x00" * 3000
                    elif what == "height-1":
                        o[i].creation_height = h - 1
                    else:
                        o[i].creation_height = h + 1
                    ok, why = evaluate_emission_contract(s, o, h, TREASURY_HASH, LP_HASH)
                    self.assertFalse(ok, f"{name} output {i} {what} must reject")
                    self.assertIn("F-009", why)


class CrossOracleTests(unittest.TestCase):
    """
    The contract simulator and the reference model must agree at every
    height we care about. Disagreement means one of them is wrong.
    """

    def test_block_reward_agrees_at_all_boundaries(self):
        for epoch in range(MAX_HALVINGS + 5):
            for offset in (-1, 0, 1):
                h = epoch * BLOCKS_PER_HALVING + offset
                if h < 0:
                    continue
                contract_val = block_reward_contract(h)
                model_val = model.block_reward(h)
                self.assertEqual(
                    contract_val, model_val,
                    f"Disagreement at h={h}: contract={contract_val}, model={model_val}",
                )

    def test_block_reward_agrees_at_sampled_heights(self):
        # Sample every ~100k blocks across 20 epochs
        h = 0
        while h < MAX_HALVINGS * BLOCKS_PER_HALVING + 5 * BLOCKS_PER_HALVING:
            self.assertEqual(
                block_reward_contract(h), model.block_reward(h),
                f"Disagreement at h={h}",
            )
            h += 97_337  # prime-ish step to avoid boundary-only coverage

    def test_block_reward_lookup_matches_shift_for_defined_epochs(self):
        """Contract's if/else chain must produce the same as shift-based
        formula across the 6 explicit arms (halvings 0..5)."""
        for epoch in range(0, 6):
            h = epoch * BLOCKS_PER_HALVING
            contract_val = block_reward_contract(h)
            shift_val = INITIAL_REWARD >> epoch
            self.assertEqual(contract_val, max(shift_val, MIN_REWARD))

    def test_block_reward_lookup_hits_floor_for_epoch_6_plus(self):
        """halvings >= 6 drop to MIN_REWARD (1.5 YOLO). A pure shift would
        give 75 / 64 = 1.171875 YOLO at epoch 6; the floor applies instead."""
        for epoch in range(6, MAX_HALVINGS + 5):
            h = epoch * BLOCKS_PER_HALVING
            self.assertEqual(block_reward_contract(h), MIN_REWARD)


# ============================================================
# ROUNDING AT EACH EPOCH
# ============================================================

class RoundingTests(unittest.TestCase):
    """
    The miner share is computed as `reward - treasury - lp` to absorb
    integer-division remainders. The contract must produce no lost nanocoins
    at any epoch.
    """

    def test_split_sum_equals_reward_all_epochs(self):
        for epoch in range(MAX_HALVINGS + 5):
            h = epoch * BLOCKS_PER_HALVING
            reward = block_reward_contract(h)
            miner, treasury, lp = split_contract(reward)
            self.assertEqual(
                miner + treasury + lp, reward,
                f"Rounding loss at epoch {epoch}: {miner}+{treasury}+{lp} != {reward}",
            )

    def test_treasury_exactly_4_pct_floor(self):
        """treasury = reward * 4 // 100 — verify no overpay from the floor."""
        for epoch in range(MAX_HALVINGS + 5):
            h = epoch * BLOCKS_PER_HALVING
            reward = block_reward_contract(h)
            _, treasury, _ = split_contract(reward)
            self.assertEqual(treasury, reward * 4 // 100)
            self.assertLessEqual(treasury * 100, reward * 4)

    def test_lp_exactly_3_pct_floor(self):
        for epoch in range(MAX_HALVINGS + 5):
            h = epoch * BLOCKS_PER_HALVING
            reward = block_reward_contract(h)
            _, _, lp = split_contract(reward)
            self.assertEqual(lp, reward * 3 // 100)
            self.assertLessEqual(lp * 100, reward * 3)

    def test_miner_residual_always_between_92_and_94_pct(self):
        """Miner absorbs the residual. Should always be close to 93%."""
        for epoch in range(MAX_HALVINGS + 5):
            h = epoch * BLOCKS_PER_HALVING
            reward = block_reward_contract(h)
            miner, treasury, lp = split_contract(reward)
            # Residual can be at most (treasury_rem + lp_rem) / reward above 93%
            # With reward >= 1_000_000_000 and pct divisors of 100,
            # max residual gain is 2 nanocoins — basically zero.
            ratio = miner * 100 / reward
            self.assertGreaterEqual(ratio, 92.99, f"Miner ratio {ratio} too low at epoch {epoch}")
            self.assertLessEqual(ratio, 93.01, f"Miner ratio {ratio} too high at epoch {epoch}")

    def test_specific_rounding_at_each_epoch(self):
        """
        At MIN_REWARD = 1.5e9 the split is exact (60M / 45M). At
        INITIAL_REWARD = 7.5e10, same. Every arm through epoch 5 (/32 =
        2,343,750,000) is still a multiple of 100 nanocoins, so 4% and 3%
        divide exactly.
        """
        expected = [
            # (epoch, miner, treasury, lp) in nanocoins, from emission_model.py
            (0, 69_750_000_000, 3_000_000_000, 2_250_000_000),   # 75 coins
            (1, 34_875_000_000, 1_500_000_000, 1_125_000_000),   # 37.5 coins
            (2, 17_437_500_000,   750_000_000,   562_500_000),   # 18.75 coins
            (3,  8_718_750_000,   375_000_000,   281_250_000),   # 9.375 coins
            (4,  4_359_375_000,   187_500_000,   140_625_000),   # 4.6875 coins
            (5,  2_179_687_500,    93_750_000,    70_312_500),   # 2.34375 coins (6th arm)
            (6,  1_395_000_000,    60_000_000,    45_000_000),   # 1.5 coin floor
            (7,  1_395_000_000,    60_000_000,    45_000_000),   # 1.5 coins
            (10, 1_395_000_000,    60_000_000,    45_000_000),   # 1.5 coins
            (19, 1_395_000_000,    60_000_000,    45_000_000),   # 1.5 coins
        ]
        for epoch, em, et, el in expected:
            h = epoch * BLOCKS_PER_HALVING
            reward = block_reward_contract(h)
            miner, treasury, lp = split_contract(reward)
            self.assertEqual((miner, treasury, lp), (em, et, el),
                             f"Split mismatch at epoch {epoch}")


# ============================================================
# NORMAL PATH — ACCEPT TESTS AT EVERY HALVING BOUNDARY
# ============================================================

class NormalPathBoundaryTests(unittest.TestCase):
    """
    For every halving boundary H = epoch * BLOCKS_PER_HALVING, construct a
    canonical spend at H-1, H, H+1 and assert it accepts. The self-box value
    at each height is the Python model's emission_box_value_at(), which
    guarantees normalPath's sufficientFunds check passes.

    Note (v1.1): heightIncreased requires HEIGHT > SELF.creation_height.
    make_normal_spend() sets SELF.creation_height=0, so any height > 0 works.
    The very-first-block case (height = 0) needs SELF.creation_height < 0
    which is impossible — so the genesis block is skipped. In practice,
    the emission box is created at genesis (creation_height=0) and first
    spent at height=1 or later.
    """

    def _spend_at(self, height: int):
        # SELF.value = the model's predicted value at that height — always
        # comfortably above the block reward for any pre-terminal epoch.
        self_val = model.emission_box_value_at(height)
        self.assertGreater(self_val, 0, f"h={height}: box exhausted, skipping normal-path test")
        self_box, outputs = make_normal_spend(height, self_val)
        ok, why = evaluate_emission_contract(
            self_box, outputs, height, TREASURY_HASH, LP_HASH
        )
        self.assertTrue(ok, f"h={height}: unexpected reject ({why})")

    def test_boundaries_for_all_epochs(self):
        # Epochs 0..19. Beyond epoch 19 the box exhausts (terminal path).
        # Skip height 0 because heightIncreased requires HEIGHT > creation_height(=0).
        for epoch in range(0, MAX_HALVINGS):
            boundary = epoch * BLOCKS_PER_HALVING
            for offset in (-1, 0, 1):
                h = boundary + offset
                if h <= 0:
                    continue
                # Skip heights where box would be exhausted
                if model.emission_box_value_at(h) <= 0:
                    continue
                with self.subTest(epoch=epoch, offset=offset, h=h):
                    self._spend_at(h)

    def test_first_block_after_genesis(self):
        """
        v1.1: genesis-box is created at height 0. First possible spend is
        height 1 (heightIncreased: 1 > 0). The reward at height 1 is still
        the epoch-0 reward (75 coins).
        """
        self._spend_at(1)

    def test_last_block_before_first_halving(self):
        self._spend_at(BLOCKS_PER_HALVING - 1)

    def test_first_block_of_epoch_1(self):
        self._spend_at(BLOCKS_PER_HALVING)

    def test_reward_exactly_halves_between_H_minus_1_and_H(self):
        # Halving boundaries 1..5 are the geometric halvings (75→37.5,
        # 37.5→18.75, 18.75→9.375, 9.375→4.6875, 4.6875→2.34375). At
        # boundary 6 the contract steps down to MIN_REWARD = 1.5 YOLO (a
        # 36% cut, not a halving); see test_first_block_of_epoch_6_drops_to_tail.
        for epoch in range(1, 6):
            boundary = epoch * BLOCKS_PER_HALVING
            r_before = block_reward_contract(boundary - 1)
            r_at = block_reward_contract(boundary)
            self.assertEqual(r_before, r_at * 2,
                             f"Epoch {epoch}: r_before={r_before}, r_at={r_at}")

    def test_first_block_of_epoch_5_is_sixth_arm(self):
        # Boundary 5 halves 4.6875 -> 2.34375 YOLO. The 20 s schedule had
        # collapsed straight to the floor here (June 2026 ".5 fix"); the
        # 30 s schedule restores the arm. Pin it so a regression to the
        # 5-arm shape is caught.
        boundary = 5 * BLOCKS_PER_HALVING
        r_before = block_reward_contract(boundary - 1)
        r_at = block_reward_contract(boundary)
        self.assertEqual(r_before, 4_687_500_000)
        self.assertEqual(r_at, 2_343_750_000)

    def test_first_block_of_epoch_6_drops_to_tail(self):
        # Boundary 6: 2.34375 -> 1.5 YOLO tail floor (36% cut).
        boundary = 6 * BLOCKS_PER_HALVING
        r_before = block_reward_contract(boundary - 1)
        r_at = block_reward_contract(boundary)
        self.assertEqual(r_before, 2_343_750_000)
        self.assertEqual(r_at, MIN_REWARD)
        self.assertEqual(MIN_REWARD, 1_500_000_000)
        self.assertEqual((r_before - r_at) * 100 // r_before, 36)

    def test_reward_stable_between_H_and_H_plus_1(self):
        for epoch in range(1, MAX_HALVINGS):
            boundary = epoch * BLOCKS_PER_HALVING
            self.assertEqual(block_reward_contract(boundary),
                             block_reward_contract(boundary + 1))


# ============================================================
# NORMAL PATH — REJECT TESTS
# ============================================================

class NormalPathRejectTests(unittest.TestCase):
    """Mutate one field in a valid spend and confirm rejection."""

    def _base(self):
        h = 1000  # arbitrary height in epoch 0 (heightIncreased: 1000 > 0 ✓)
        self_box, outputs = make_normal_spend(h, GENESIS_VALUE)
        return h, self_box, outputs

    def test_underpay_treasury_by_one_nanocoin(self):
        h, self_box, outputs = self._base()
        outputs[1] = make_treasury_box(outputs[1].value - 1, creation_height=h)
        ok, why = evaluate_emission_contract(self_box, outputs, h, TREASURY_HASH, LP_HASH)
        self.assertFalse(ok, f"expected reject but got accept ({why})")
        self.assertIn("validTreasury", why)

    def test_underpay_lp_by_one_nanocoin(self):
        h, self_box, outputs = self._base()
        outputs[2] = make_lp_box(outputs[2].value - 1, creation_height=h)
        ok, why = evaluate_emission_contract(self_box, outputs, h, TREASURY_HASH, LP_HASH)
        self.assertFalse(ok, why)
        self.assertIn("validLP", why)

    def test_treasury_to_wrong_address(self):
        h, self_box, outputs = self._base()
        wrong_box = Box(value=outputs[1].value, proposition_bytes=b"\x00WRONG_SCRIPT",
                        creation_height=h)
        outputs[1] = wrong_box
        ok, why = evaluate_emission_contract(self_box, outputs, h, TREASURY_HASH, LP_HASH)
        self.assertFalse(ok, why)
        self.assertIn("validTreasury", why)
        self.assertIn("blake2b256", why)

    def test_lp_to_wrong_address(self):
        h, self_box, outputs = self._base()
        wrong_box = Box(value=outputs[2].value, proposition_bytes=b"\x00WRONG_LP_SCRIPT",
                        creation_height=h)
        outputs[2] = wrong_box
        ok, why = evaluate_emission_contract(self_box, outputs, h, TREASURY_HASH, LP_HASH)
        self.assertFalse(ok, why)
        self.assertIn("validLP", why)

    def test_emission_box_value_off_by_plus_one(self):
        """New emission box keeps one extra nanocoin — contract must reject."""
        h, self_box, outputs = self._base()
        outputs[0] = make_next_emission_box(self_box, outputs[0].value + 1, spend_height=h)
        ok, why = evaluate_emission_contract(self_box, outputs, h, TREASURY_HASH, LP_HASH)
        self.assertFalse(ok, why)
        self.assertIn("valueCorrect", why)

    def test_emission_box_value_off_by_minus_one(self):
        """New emission box burns one extra nanocoin — contract must reject."""
        h, self_box, outputs = self._base()
        outputs[0] = make_next_emission_box(self_box, outputs[0].value - 1, spend_height=h)
        ok, why = evaluate_emission_contract(self_box, outputs, h, TREASURY_HASH, LP_HASH)
        self.assertFalse(ok, why)
        self.assertIn("valueCorrect", why)

    def test_emission_nft_not_preserved_missing(self):
        h, self_box, outputs = self._base()
        outputs[0].tokens = []
        ok, why = evaluate_emission_contract(self_box, outputs, h, TREASURY_HASH, LP_HASH)
        self.assertFalse(ok, why)
        self.assertIn("nftPreserved", why)

    def test_emission_nft_not_preserved_wrong_id(self):
        h, self_box, outputs = self._base()
        outputs[0].tokens = [Token(bytes([0x11] * 32), 1)]
        ok, why = evaluate_emission_contract(self_box, outputs, h, TREASURY_HASH, LP_HASH)
        self.assertFalse(ok, why)
        self.assertIn("nftPreserved", why)

    def test_emission_nft_wrong_amount(self):
        h, self_box, outputs = self._base()
        outputs[0].tokens = [Token(EMISSION_NFT_ID, 2)]
        ok, why = evaluate_emission_contract(self_box, outputs, h, TREASURY_HASH, LP_HASH)
        self.assertFalse(ok, why)
        self.assertIn("nftPreserved", why)

    def test_new_emission_box_uses_different_script(self):
        h, self_box, outputs = self._base()
        outputs[0].proposition_bytes = b"\x00DIFFERENT_SCRIPT"
        ok, why = evaluate_emission_contract(self_box, outputs, h, TREASURY_HASH, LP_HASH)
        self.assertFalse(ok, why)
        self.assertIn("scriptPreserved", why)

    def test_new_emission_box_changes_r4(self):
        h, self_box, outputs = self._base()
        outputs[0].r4 = bytes([0xFF] * 32)
        ok, why = evaluate_emission_contract(self_box, outputs, h, TREASURY_HASH, LP_HASH)
        self.assertFalse(ok, why)
        self.assertIn("registersPreserved", why)

    def test_new_emission_box_changes_r5(self):
        h, self_box, outputs = self._base()
        outputs[0].r5 = bytes([0xFF] * 32)
        ok, why = evaluate_emission_contract(self_box, outputs, h, TREASURY_HASH, LP_HASH)
        self.assertFalse(ok, why)
        self.assertIn("registersPreserved", why)

    def test_no_emission_box_at_outputs_0(self):
        """
        Attacker tries to skip emission box recreation entirely.
        If OUTPUTS(0) is a treasury-shaped box with full value, normalPath
        fails nftPreserved/scriptPreserved. But terminalPath succeeds if
        self.value < reward. Here self.value is large, so terminal also fails.
        """
        h, self_box, outputs = self._base()
        outputs[0] = make_treasury_box(outputs[0].value, creation_height=h)
        ok, why = evaluate_emission_contract(self_box, outputs, h, TREASURY_HASH, LP_HASH)
        self.assertFalse(ok, why)

    def test_swap_treasury_and_lp_order(self):
        """
        OUTPUTS(1) must be treasury, OUTPUTS(2) must be LP. Swapping them
        should fail because treasury's 4% is larger than LP's 3%, so
        OUTPUTS(1) would have the LP value (!= treasuryReward under == check).
        """
        h, self_box, outputs = self._base()
        outputs[1], outputs[2] = outputs[2], outputs[1]
        ok, why = evaluate_emission_contract(self_box, outputs, h, TREASURY_HASH, LP_HASH)
        self.assertFalse(ok, why)

    def test_missing_treasury_output(self):
        h, self_box, outputs = self._base()
        outputs = [outputs[0], outputs[2], outputs[3]]  # emission + LP + miner
        ok, why = evaluate_emission_contract(self_box, outputs, h, TREASURY_HASH, LP_HASH)
        self.assertFalse(ok, why)

    def test_missing_lp_output(self):
        h, self_box, outputs = self._base()
        outputs = outputs[:2] + [outputs[3]]  # emission + treasury + miner
        ok, why = evaluate_emission_contract(self_box, outputs, h, TREASURY_HASH, LP_HASH)
        self.assertFalse(ok, why)

    # ---- NEW in v1.1: heightIncreased and heightCorrect reject tests ----

    def test_reject_same_block_spend(self):
        """
        heightIncreased: HEIGHT must be strictly greater than SELF.creation_height.
        If SELF was created at current block height, the spend must reject.
        Mirrors Ergo's `heightIncreased = GT(Height, boxCreationHeight(Self))`.
        """
        h = 1000
        self_box = make_self_box(GENESIS_VALUE, creation_height=h)  # created same block
        _, outputs = make_normal_spend(h, GENESIS_VALUE)
        # Fix up OUTPUTS(0) so it correctly references this SELF's R4/R5 (no change needed;
        # make_self_box uses same hashes). creation_height on new emission is already h.
        ok, why = evaluate_emission_contract(self_box, outputs, h, TREASURY_HASH, LP_HASH)
        self.assertFalse(ok, f"same-block spend should reject ({why})")
        self.assertIn("heightIncreased", why)

    def test_reject_earlier_block_height(self):
        """HEIGHT < SELF.creation_height must reject (impossible-time scenario)."""
        self_box = make_self_box(GENESIS_VALUE, creation_height=5000)
        _, outputs = make_normal_spend(1000, GENESIS_VALUE)
        ok, why = evaluate_emission_contract(self_box, outputs, 1000, TREASURY_HASH, LP_HASH)
        self.assertFalse(ok, why)
        self.assertIn("heightIncreased", why)

    def test_reject_new_emission_box_wrong_creation_height(self):
        """
        heightCorrect: OUTPUTS(0).creation_height must equal current HEIGHT.
        Mirrors Ergo's `heightCorrect = EQ(boxCreationHeight(rewardOut), Height)`.
        """
        h = 1000
        self_box, outputs = make_normal_spend(h, GENESIS_VALUE)
        # Set new emission box's creation_height to something other than h
        outputs[0].creation_height = h - 1
        ok, why = evaluate_emission_contract(self_box, outputs, h, TREASURY_HASH, LP_HASH)
        self.assertFalse(ok, why)
        self.assertIn("heightCorrect", why)

    def test_reject_new_emission_box_future_creation_height(self):
        """New emission box with creation_height > HEIGHT must also reject."""
        h = 1000
        self_box, outputs = make_normal_spend(h, GENESIS_VALUE)
        outputs[0].creation_height = h + 1
        ok, why = evaluate_emission_contract(self_box, outputs, h, TREASURY_HASH, LP_HASH)
        self.assertFalse(ok, why)
        self.assertIn("heightCorrect", why)


# ============================================================
# NORMAL PATH — OVERPAY NOW REJECTED (v1.1: strict ==)
# ============================================================

class NormalPathOverpayRejectTests(unittest.TestCase):
    """
    v1.1 tightens treasury/LP value checks from `>=` to `==` to match Ergo's
    strict-equality convention (see ErgoScriptPredef.scala emissionBoxProp:
    `EQ(coinsToIssue, Minus(ExtractAmount(Self), ExtractAmount(rewardOut)))`).
    Any overpay — even by 1 nanocoin — must now be rejected.
    """

    def _base(self):
        h = 100
        self_box, outputs = make_normal_spend(h, GENESIS_VALUE)
        return h, self_box, outputs

    def test_overpay_treasury_rejected(self):
        h, self_box, outputs = self._base()
        outputs[1] = make_treasury_box(outputs[1].value + 1, creation_height=h)
        ok, why = evaluate_emission_contract(self_box, outputs, h, TREASURY_HASH, LP_HASH)
        self.assertFalse(ok, f"overpay must be rejected under v1.1 == policy ({why})")
        self.assertIn("validTreasury", why)

    def test_overpay_lp_rejected(self):
        h, self_box, outputs = self._base()
        outputs[2] = make_lp_box(outputs[2].value + 1, creation_height=h)
        ok, why = evaluate_emission_contract(self_box, outputs, h, TREASURY_HASH, LP_HASH)
        self.assertFalse(ok, why)
        self.assertIn("validLP", why)

    def test_exact_treasury_accepts(self):
        """Boundary: value exactly equal to treasuryReward accepts."""
        h, self_box, outputs = self._base()
        ok, why = evaluate_emission_contract(self_box, outputs, h, TREASURY_HASH, LP_HASH)
        self.assertTrue(ok, why)

    def test_treasury_large_overpay_rejected(self):
        """Ergo-parity: strict == means ANY deviation rejects, not just ±1."""
        h, self_box, outputs = self._base()
        outputs[1] = make_treasury_box(outputs[1].value + 1_000_000_000, creation_height=h)
        ok, why = evaluate_emission_contract(self_box, outputs, h, TREASURY_HASH, LP_HASH)
        self.assertFalse(ok, why)


# ============================================================
# TERMINAL PATH — ACCEPT TESTS
# ============================================================

class TerminalPathAcceptTests(unittest.TestCase):
    """
    Terminal path triggers when SELF.value <= blockReward (`<=` since
    2026-11-10). At epochs 6+, blockReward = MIN_REWARD = 1.5e9. So terminal
    path requires SELF.value <= 1.5e9.

    v1.1: heightIncreased requires HEIGHT > SELF.creation_height. All terminal
    tests pass a height > 0 so SELF.creation_height=0 works.
    Strict == on term_treasury/term_lp values.
    """

    def test_terminal_with_half_min_reward_remaining(self):
        height = 7 * BLOCKS_PER_HALVING
        remaining = MIN_REWARD // 2
        self_box, outputs = make_terminal_spend(remaining, height=height)
        ok, why = evaluate_emission_contract(
            self_box, outputs, height, TREASURY_HASH, LP_HASH
        )
        self.assertTrue(ok, f"expected accept via terminal, got ({why})")
        self.assertIn("terminalPath", why)

    def test_terminal_with_zero_remaining(self):
        """
        Edge case: SELF.value = 0. termTreasury = 0, termLP = 0.
        Under v1.1 strict ==, OUTPUTS(0/1).value must be exactly 0.
        In practice, consensus MIN_BOX_VALUE makes this unreachable — theoretical
        case documenting contract semantics only.
        """
        height = 10 * BLOCKS_PER_HALVING
        remaining = 0
        self_box, outputs = make_terminal_spend(remaining, height=height)
        ok, why = evaluate_emission_contract(
            self_box, outputs, height, TREASURY_HASH, LP_HASH
        )
        self.assertTrue(ok, f"terminal with zero should accept ({why})")

    def test_terminal_reward_minus_one(self):
        """SELF.value = blockReward - 1 → terminal path."""
        height = 7 * BLOCKS_PER_HALVING
        reward = block_reward_contract(height)
        remaining = reward - 1
        self_box, outputs = make_terminal_spend(remaining, height=height)
        ok, why = evaluate_emission_contract(
            self_box, outputs, height, TREASURY_HASH, LP_HASH
        )
        self.assertTrue(ok, why)

    def test_normal_path_rejects_when_equal_to_reward(self):
        """
        Boundary: SELF.value == blockReward. sufficientFunds passes (>=),
        so the script accepts a normal-shaped spend whose new box has value 0.
        Consensus rejects that 0-value box; the terminal path (which also
        accepts here since the `<=` fix) is the usable spend. Despite the
        name, this pins script-level acceptance.
        """
        height = 7 * BLOCKS_PER_HALVING
        reward = block_reward_contract(height)
        remaining = reward
        self_box, outputs = make_normal_spend(height, remaining)
        self.assertEqual(outputs[0].value, 0)
        ok, why = evaluate_emission_contract(
            self_box, outputs, height, TREASURY_HASH, LP_HASH
        )
        self.assertTrue(ok, f"contract accepts 0-value next box ({why})")

    def test_terminal_entry_via_realistic_final_blocks(self):
        """
        Walk the last few blocks before emission exhaustion and verify
        normal path stays valid until SELF.value < reward, then terminal kicks in.
        """
        height = 20 * BLOCKS_PER_HALVING
        reward = MIN_REWARD
        value = reward * 3 + (reward // 2)
        blocks_ok = 0
        while value >= reward:
            self_box, outputs = make_normal_spend(height, value)
            ok, why = evaluate_emission_contract(
                self_box, outputs, height, TREASURY_HASH, LP_HASH
            )
            self.assertTrue(ok, f"block {blocks_ok}: expected accept ({why})")
            value -= reward
            height += 1
            blocks_ok += 1
            if blocks_ok > 10:
                self.fail("emission did not exhaust as expected")

        self.assertEqual(blocks_ok, 3, f"expected 3 normal blocks, got {blocks_ok}")
        # Now value < reward, terminal path should trigger
        self_box, outputs = make_terminal_spend(value, height=height)
        ok, why = evaluate_emission_contract(
            self_box, outputs, height, TREASURY_HASH, LP_HASH
        )
        self.assertTrue(ok, f"terminal failed: {why}")


# ============================================================
# TERMINAL PATH — REJECT TESTS
# ============================================================

class TerminalPathRejectTests(unittest.TestCase):
    def _base(self):
        height = 7 * BLOCKS_PER_HALVING
        remaining = MIN_REWARD // 2
        self_box, outputs = make_terminal_spend(remaining, height=height)
        # The unmodified base must accept, so a base that drifts from the
        # contract's split fails here and not silently in the reject tests.
        ok, why = evaluate_emission_contract(self_box, list(outputs), height, TREASURY_HASH, LP_HASH)
        assert ok, f"terminal base must accept: {why}"
        return height, self_box, outputs, remaining

    def test_underpay_terminal_treasury(self):
        h, self_box, outputs, remaining = self._base()
        term_treasury = remaining * TREASURY_PCT // 100
        outputs[0] = make_treasury_box(term_treasury - 1, creation_height=h)
        ok, why = evaluate_emission_contract(self_box, outputs, h, TREASURY_HASH, LP_HASH)
        self.assertFalse(ok, why)

    def test_underpay_terminal_lp(self):
        h, self_box, outputs, remaining = self._base()
        term_lp = remaining * LP_PCT // 100
        outputs[1] = make_lp_box(term_lp - 1, creation_height=h)
        ok, why = evaluate_emission_contract(self_box, outputs, h, TREASURY_HASH, LP_HASH)
        self.assertFalse(ok, why)

    def test_overpay_terminal_treasury_rejected(self):
        """v1.1: == not >=, so overpay also rejects."""
        h, self_box, outputs, remaining = self._base()
        term_treasury = remaining * TREASURY_PCT // 100
        outputs[0] = make_treasury_box(term_treasury + 1, creation_height=h)
        ok, why = evaluate_emission_contract(self_box, outputs, h, TREASURY_HASH, LP_HASH)
        self.assertFalse(ok, f"overpay must be rejected under v1.1 ({why})")

    def test_overpay_terminal_lp_rejected(self):
        h, self_box, outputs, remaining = self._base()
        term_lp = remaining * LP_PCT // 100
        outputs[1] = make_lp_box(term_lp + 1, creation_height=h)
        ok, why = evaluate_emission_contract(self_box, outputs, h, TREASURY_HASH, LP_HASH)
        self.assertFalse(ok, why)

    def test_terminal_treasury_wrong_script(self):
        h, self_box, outputs, _ = self._base()
        outputs[0] = Box(value=outputs[0].value, proposition_bytes=b"\x00WRONG",
                         creation_height=h)
        ok, why = evaluate_emission_contract(self_box, outputs, h, TREASURY_HASH, LP_HASH)
        self.assertFalse(ok, why)

    def test_terminal_lp_wrong_script(self):
        h, self_box, outputs, _ = self._base()
        outputs[1] = Box(value=outputs[1].value, proposition_bytes=b"\x00WRONG_LP",
                         creation_height=h)
        ok, why = evaluate_emission_contract(self_box, outputs, h, TREASURY_HASH, LP_HASH)
        self.assertFalse(ok, why)

    def test_terminal_nft_smuggled_to_output_0(self):
        """Emission NFT must not appear in any output during terminal path."""
        h, self_box, outputs, _ = self._base()
        outputs[0].tokens = [Token(EMISSION_NFT_ID, 1)]
        ok, why = evaluate_emission_contract(self_box, outputs, h, TREASURY_HASH, LP_HASH)
        self.assertFalse(ok, why)
        self.assertIn("nftBurned", why)

    def test_terminal_nft_smuggled_to_output_1(self):
        h, self_box, outputs, _ = self._base()
        outputs[1].tokens = [Token(EMISSION_NFT_ID, 1)]
        ok, why = evaluate_emission_contract(self_box, outputs, h, TREASURY_HASH, LP_HASH)
        self.assertFalse(ok, why)
        self.assertIn("nftBurned", why)

    def test_terminal_nft_smuggled_to_additional_output(self):
        """Attacker adds a third output carrying the NFT — must fail nftBurned."""
        h, self_box, outputs, _ = self._base()
        nft_carrier = Box(
            value=MIN_BOX_VALUE,
            proposition_bytes=b"\x00MINER",
            tokens=[Token(EMISSION_NFT_ID, 1)],
            creation_height=h,
        )
        outputs.append(nft_carrier)
        ok, why = evaluate_emission_contract(self_box, outputs, h, TREASURY_HASH, LP_HASH)
        self.assertFalse(ok, why)
        self.assertIn("nftBurned", why)

    def test_terminal_attempted_when_normal_path_valid(self):
        """
        Adversary sets up terminal-shaped outputs even though SELF.value >= reward.
        `insufficient` is false; terminal path rejects. Normal path also rejects
        because outputs aren't in normal-path shape. Result: reject.
        """
        h = 7 * BLOCKS_PER_HALVING
        reward = block_reward_contract(h)
        self_value = reward * 100
        self_box = make_self_box(self_value, creation_height=0)
        term_treasury = self_value * TREASURY_PCT // 100
        term_lp = self_value * LP_PCT // 100
        outputs = [
            make_treasury_box(term_treasury, creation_height=h),
            make_lp_box(term_lp, creation_height=h),
            make_miner_box(self_value - term_treasury - term_lp, creation_height=h),
        ]
        ok, why = evaluate_emission_contract(self_box, outputs, h, TREASURY_HASH, LP_HASH)
        self.assertFalse(ok, why)

    def test_terminal_same_block_spend_rejects(self):
        """
        v1.1 [Gap A]: heightIncreased applies in terminal path too.
        SELF created at same block as spend must reject.
        """
        h = 7 * BLOCKS_PER_HALVING
        remaining = MIN_REWARD // 2
        self_box = make_self_box(remaining, creation_height=h)  # same block
        _, outputs = make_terminal_spend(remaining, height=h)
        ok, why = evaluate_emission_contract(self_box, outputs, h, TREASURY_HASH, LP_HASH)
        self.assertFalse(ok, why)
        self.assertIn("heightIncreased", why)


# ============================================================
# PATH-SELECTION INVARIANT
# ============================================================

class PathSelectionTests(unittest.TestCase):
    """
    For a given (self, outputs, height) at most one path accepts, because the
    two paths need different output shapes (OUTPUTS(0) is the emission box
    with the NFT, or the treasury box with no NFT). `sufficientFunds` (>=) and
    `insufficient` (<=) overlap only at SELF.value == blockReward, where the
    normal shape needs a 0-value successor that consensus rejects.
    """

    def test_normal_and_terminal_cannot_both_accept_normal_spend(self):
        h = 1000
        self_box, outputs = make_normal_spend(h, GENESIS_VALUE)
        reward = block_reward_contract(h)
        self.assertGreaterEqual(self_box.value, reward)
        ok, why = evaluate_emission_contract(self_box, outputs, h, TREASURY_HASH, LP_HASH)
        self.assertTrue(ok)
        self.assertIn("normalPath", why)

    def test_normal_and_terminal_cannot_both_accept_terminal_spend(self):
        h = 10 * BLOCKS_PER_HALVING
        remaining = MIN_REWARD // 3
        self_box, outputs = make_terminal_spend(remaining, height=h)
        reward = block_reward_contract(h)
        self.assertLess(self_box.value, reward)
        ok, why = evaluate_emission_contract(self_box, outputs, h, TREASURY_HASH, LP_HASH)
        self.assertTrue(ok)
        self.assertIn("terminalPath", why)


# ============================================================
# OVERFLOW / BOUNDS
# ============================================================

class BoundsTests(unittest.TestCase):
    def test_no_overflow_at_max_reward(self):
        """reward * MINER_PCT (93) must stay under i64 max."""
        max_intermediate = INITIAL_REWARD * MINER_PCT
        self.assertLess(max_intermediate, 2**63 - 1)

    def test_no_overflow_at_genesis_box_value(self):
        """Genesis box value must fit in i64."""
        self.assertLess(GENESIS_VALUE, 2**63 - 1)

    def test_genesis_box_value_matches_spec(self):
        """Total supply across 6 explicit arms + 14 tail-rate epochs:

        1051200 x (75 + 37.5 + 18.75 + 9.375 + 4.6875 + 2.34375)
        + 14 x 1.5 x 1051200 = 155,216,250 + 22,075,200 = 177,291,450 YOLO.
        Integral because 1051200 is divisible by 64. (20 s schedule:
        176,525,325 YOLO.) Since 2026-11-12 the genesis emission box holds
        177,291,250 YOLO; the other 200 sit in the treasury fund box (100)
        and the LP fund box (100)."""
        expected_total = 177_291_450_000_000_000
        self.assertEqual(TOTAL_SUPPLY, expected_total)
        self.assertEqual(model.TOTAL_SUPPLY_LITERAL, expected_total)
        expected = 177_291_250_000_000_000
        self.assertEqual(GENESIS_VALUE, expected)
        self.assertEqual(model.GENESIS_EMISSION_BOX_LITERAL, expected)
        self.assertEqual(model.GENESIS_TREASURY_FUND_BOX, 100 * NANOCOIN)
        self.assertEqual(model.GENESIS_LP_FUND_BOX, 100 * NANOCOIN)
        self.assertEqual(GENESIS_VALUE + FUND_BOXES, TOTAL_SUPPLY)

    def test_reward_monotonically_non_increasing(self):
        prev = INITIAL_REWARD + 1
        h = 0
        while h < 25 * BLOCKS_PER_HALVING:
            r = block_reward_contract(h)
            self.assertLessEqual(r, prev, f"reward increased at h={h}")
            prev = r
            h += 10_000

    def test_reward_never_below_min(self):
        h = 0
        while h < 25 * BLOCKS_PER_HALVING:
            self.assertGreaterEqual(block_reward_contract(h), MIN_REWARD)
            h += 10_000


# ============================================================
# 30 s RETUNE: SCHEDULE, SIXTH ARM, TAIL BOUNDARY, YEAR-20 EXHAUSTION
# ============================================================

def _spend_paying(height: int, self_value: int, reward: int):
    """Normal-path-shaped spend at `height` that pays `reward` (possibly wrong)."""
    miner_amt, treasury_amt, lp_amt = split_contract(reward)
    self_box = make_self_box(self_value, creation_height=0)
    outputs = [
        make_next_emission_box(self_box, self_value - reward, spend_height=height),
        make_treasury_box(treasury_amt, creation_height=height),
        make_lp_box(lp_amt, creation_height=height),
        make_miner_box(miner_amt, creation_height=height),
    ]
    return evaluate_emission_contract(self_box, outputs, height, TREASURY_HASH, LP_HASH)


class RetuneScheduleTests(unittest.TestCase):
    """Locked 2026-11-09 30 s block package."""

    END = MAX_HALVINGS * BLOCKS_PER_HALVING  # 21,024,000: end of chain year 20

    def test_block_constants(self):
        self.assertEqual(model.BLOCK_TIME_SECONDS, 30)
        self.assertEqual(model.BLOCKS_PER_YEAR, 1_051_200)
        self.assertEqual(BLOCKS_PER_HALVING, 1_051_200)
        self.assertEqual(model.BLOCKS_PER_HALVING, BLOCKS_PER_HALVING)
        self.assertEqual(BLOCKS_PER_HALVING % 64, 0)
        self.assertEqual(INITIAL_REWARD, model.INITIAL_REWARD)
        self.assertEqual(INITIAL_REWARD, 75 * NANOCOIN)
        self.assertEqual(MIN_REWARD, model.MIN_REWARD)
        self.assertEqual(MAX_HALVINGS, model.MAX_HALVINGS)
        self.assertEqual(model.NUM_EXPLICIT_HALVINGS, 6)

    def test_seven_reward_levels_and_integral_splits(self):
        levels = [75_000_000_000, 37_500_000_000, 18_750_000_000, 9_375_000_000,
                  4_687_500_000, 2_343_750_000, 1_500_000_000]
        seen = sorted({block_reward_contract(e * BLOCKS_PER_HALVING)
                       for e in range(MAX_HALVINGS + 5)}, reverse=True)
        self.assertEqual(seen, levels)
        for r in levels:
            self.assertEqual(r * TREASURY_PCT % 100, 0, f"treasury split inexact for {r}")
            self.assertEqual(r * LP_PCT % 100, 0, f"LP split inexact for {r}")
            self.assertEqual(r * BLOCKS_PER_HALVING % NANOCOIN, 0, f"year total not whole for {r}")

    def test_total_supply_is_six_arm_years_plus_fourteen_tail_years(self):
        """The schedule total is the total supply; the emission box plus the
        two 100-coin fund boxes add up to it (2026-11-12)."""
        arm_years = sum((INITIAL_REWARD >> e) * BLOCKS_PER_HALVING for e in range(6))
        tail_years = (MAX_HALVINGS - 6) * MIN_REWARD * BLOCKS_PER_HALVING
        self.assertEqual(arm_years, 155_216_250 * NANOCOIN)
        self.assertEqual(tail_years, 22_075_200 * NANOCOIN)
        self.assertEqual(arm_years + tail_years, TOTAL_SUPPLY)
        self.assertEqual(GENESIS_VALUE + 200 * NANOCOIN, arm_years + tail_years)

    def test_sixth_arm_boundary_spends_accept(self):
        b = 5 * BLOCKS_PER_HALVING
        for h in (b - 1, b, b + 1):
            self_box, outputs = make_normal_spend(h, model.emission_box_value_at(h))
            ok, why = evaluate_emission_contract(self_box, outputs, h, TREASURY_HASH, LP_HASH)
            self.assertTrue(ok, f"h={h}: {why}")

    def test_sixth_arm_rejects_tail_floor_payout(self):
        b = 5 * BLOCKS_PER_HALVING
        ok, why = _spend_paying(b, model.emission_box_value_at(b), MIN_REWARD)
        self.assertFalse(ok, why)
        self.assertIn("validTreasury", why)

    def test_tail_boundary_spends_accept(self):
        b = 6 * BLOCKS_PER_HALVING
        for h in (b - 1, b, b + 1):
            self_box, outputs = make_normal_spend(h, model.emission_box_value_at(h))
            ok, why = evaluate_emission_contract(self_box, outputs, h, TREASURY_HASH, LP_HASH)
            self.assertTrue(ok, f"h={h}: {why}")

    def test_tail_rejects_sixth_arm_payout(self):
        b = 6 * BLOCKS_PER_HALVING
        ok, why = _spend_paying(b, model.emission_box_value_at(b), 2_343_750_000)
        self.assertFalse(ok, why)

    def test_schedule_total_exhausts_at_end_of_year_20(self):
        """Schedule view: a box holding the full total supply, with height 0
        paid, runs out exactly at the end of chain year 20. The real box
        holds 200 coins less and height 0 is never mined, so it runs out at
        21,023,916 instead (test_real_chain_box_runs_out_before_end_of_year_20)."""
        end = self.END
        self.assertEqual(end, 21_024_000)
        self.assertEqual(model.final_emission_height(TOTAL_SUPPLY), end)
        self.assertEqual(model.emission_box_value_at(end - 1, TOTAL_SUPPLY), MIN_REWARD)
        self.assertEqual(model.emission_box_value_at(end, TOTAL_SUPPLY), 0)
        # Simulator-only: the model drains the box to a 0-value successor via the normal
        # path, which the simulator accepts but consensus rejects (0-value successor is
        # consensus-invalid).
        self_box, outputs = make_normal_spend(end - 1, MIN_REWARD)
        self.assertEqual(outputs[0].value, 0)
        ok, why = evaluate_emission_contract(self_box, outputs, end - 1, TREASURY_HASH, LP_HASH)
        self.assertTrue(ok, why)

    def test_real_chain_box_runs_out_before_end_of_year_20(self):
        """2026-11-12 genesis: the real box holds the total supply minus
        200 coins, and height 0 is never mined (75 coins unpaid). Net, it is
        125 coins short of the year-20 schedule, so it runs out 84 blocks
        before the end of year 20 and never reaches value == reward.

        Walk the last 50 normal spends (box 76 coins at 21,023,866 down to
        2.5 at 21,023,915, successor 1.0), then the terminal spend at
        21,023,916 from a 1.0-coin box, split 4% / 3% / remainder."""
        self.assertEqual(model.real_box_value_at(self.END), -125 * NANOCOIN)
        self.assertEqual(self.END - REAL_TERMINAL, 84)
        h = REAL_TERMINAL - 50
        value = model.real_box_value_at(h)
        self.assertEqual(value, 76 * NANOCOIN)
        normal_blocks = 0
        while value > block_reward_contract(h):
            self.assertEqual(value, model.real_box_value_at(h))
            self_box, outputs = make_normal_spend(h, value)
            ok, why = evaluate_emission_contract(self_box, outputs, h, TREASURY_HASH, LP_HASH)
            self.assertTrue(ok and "normalPath" in why, f"h={h}: {why}")
            value = outputs[0].value
            h += 1
            normal_blocks += 1
        self.assertEqual(normal_blocks, 50)
        self.assertEqual(h, REAL_TERMINAL)
        self.assertEqual(value, 1 * NANOCOIN)
        self.assertLess(value, block_reward_contract(h))
        self_box, outputs = make_terminal_spend(value, height=h)
        ok, why = evaluate_emission_contract(self_box, outputs, h, TREASURY_HASH, LP_HASH)
        self.assertTrue(ok, why)
        self.assertIn("terminalPath", why)
        self.assertEqual([o.value for o in outputs], [40_000_000, 30_000_000, 930_000_000])


# ============================================================
# 2026-11-10 TERMINAL FIX: `insufficient` is `SELF.value <= blockReward`
# ============================================================

def _terminal_tx(height: int, remaining: int, nft_to_miner: bool = False):
    """Terminal coinbase: treasury, LP, bound miner remainder (OUTPUTS(2))."""
    self_box, outputs = make_terminal_spend(remaining, height=height)
    assert outputs[2].value == remaining - outputs[0].value - outputs[1].value
    if nft_to_miner:
        outputs[2].tokens = [Token(EMISSION_NFT_ID, 1)]
    return self_box, outputs


def _consensus_min_value_ok(outputs: List[Box]) -> bool:
    """Stand-in for the consensus dust rule: every output >= MIN_BOX_VALUE."""
    return all(o.value >= MIN_BOX_VALUE for o in outputs)


class TerminalLeFixTests(unittest.TestCase):
    """The `<=` guard. Cases at SELF.value == blockReward are contract tests
    at the tail height EQ_H; the real-chain terminal spend (box 1.0 coin,
    below the reward) is tested at REAL_TERMINAL."""
    END = MAX_HALVINGS * BLOCKS_PER_HALVING   # 21,024,000
    EQ_H = EQ_H                               # tail height, value == reward case

    def test_terminal_accepts_at_value_equals_reward(self):
        h = self.EQ_H
        self.assertEqual(block_reward_contract(h), MIN_REWARD)
        self_box, outputs = _terminal_tx(h, MIN_REWARD)
        ok, why = evaluate_emission_contract(self_box, outputs, h, TREASURY_HASH, LP_HASH)
        self.assertTrue(ok, why)
        self.assertIn("terminalPath", why)
        self.assertEqual([o.value for o in outputs], [60_000_000, 45_000_000, 1_395_000_000])
        self.assertEqual(sum(o.value for o in outputs), MIN_REWARD)
        self.assertTrue(_consensus_min_value_ok(outputs))
        for o in outputs:
            self.assertFalse(any(t.token_id == EMISSION_NFT_ID for t in o.tokens))

    def test_terminal_rejects_one_nano_above_reward(self):
        h = self.EQ_H
        self_box, outputs = _terminal_tx(h, MIN_REWARD + 1)
        ok, why = evaluate_emission_contract(self_box, outputs, h, TREASURY_HASH, LP_HASH)
        self.assertFalse(ok, why)
        self.assertIn("insufficient", why)

    def test_terminal_rejects_at_twice_reward(self):
        for h, r in ((self.END + 48, MIN_REWARD), (2, INITIAL_REWARD)):
            self.assertEqual(block_reward_contract(h), r)
            self_box, outputs = _terminal_tx(h, 2 * r)
            ok, why = evaluate_emission_contract(self_box, outputs, h, TREASURY_HASH, LP_HASH)
            self.assertFalse(ok, f"h={h}: {why}")
            self.assertIn("insufficient", why)

    def test_terminal_rejects_nft_kept_by_miner_at_last_block(self):
        self_box, outputs = _terminal_tx(self.EQ_H, MIN_REWARD, nft_to_miner=True)
        ok, why = evaluate_emission_contract(self_box, outputs, self.EQ_H, TREASURY_HASH, LP_HASH)
        self.assertFalse(ok, why)
        self.assertIn("nftBurned", why)

    def test_last_block_miner_options_le_versus_lt(self):
        """Second-order: what the miner of a value == reward box can do (a
        contract case; the real box never holds exactly one reward).

        At value == reward the normal-shaped tx passes the script but its
        successor is 0-value (below MIN_BOX_VALUE), so consensus rejects it.
        The terminal tx is the only consensus-valid spend, and it pays the
        miner exactly what a normal block would: no new extraction."""
        h = self.EQ_H
        n_self, n_out = make_normal_spend(h, MIN_REWARD)
        ok, why = evaluate_emission_contract(n_self, n_out, h, TREASURY_HASH, LP_HASH)
        self.assertTrue(ok, why)
        self.assertEqual(n_out[0].value, 0)
        self.assertFalse(_consensus_min_value_ok(n_out))
        t_self, t_out = _terminal_tx(h, MIN_REWARD)
        ok, why = evaluate_emission_contract(t_self, t_out, h, TREASURY_HASH, LP_HASH)
        self.assertTrue(ok, why)
        self.assertTrue(_consensus_min_value_ok(t_out))
        normal_miner = MIN_REWARD - n_out[1].value - n_out[2].value
        self.assertEqual((t_out[0].value, t_out[1].value), (n_out[1].value, n_out[2].value))
        self.assertEqual(t_out[2].value, normal_miner)

    def test_terminal_split_at_value_equals_reward_every_level(self):
        """No reward level makes a non-integral or non-normal terminal split."""
        for e in range(model.NUM_EXPLICIT_HALVINGS + 1):
            h = e * BLOCKS_PER_HALVING + 1
            r = block_reward_contract(h)
            self.assertEqual(r * TREASURY_PCT % 100, 0)
            self.assertEqual(r * LP_PCT % 100, 0)
            self_box, outputs = _terminal_tx(h, r)
            ok, why = evaluate_emission_contract(self_box, outputs, h, TREASURY_HASH, LP_HASH)
            self.assertTrue(ok, f"epoch {e}: {why}")
            miner, treasury, lp = split_contract(r)
            self.assertEqual([o.value for o in outputs], [treasury, lp, miner], f"epoch {e}")

    def test_every_reachable_terminal_value_splits_exactly(self):
        """Every box value the chain can reach is a multiple of 31,250,000
        nano (the gcd of the 177,291,250-coin genesis value and all seven
        reward levels; it was 93,750,000 for the 177,291,450-coin box before
        2026-11-12), and 4% and 3% of that unit are whole, so any terminal
        remainder splits with no rounding."""
        from math import gcd
        levels = {block_reward_contract(e * BLOCKS_PER_HALVING) for e in range(MAX_HALVINGS + 5)}
        g = GENESIS_VALUE
        for r in levels:
            g = gcd(g, r)
        self.assertEqual(g, 31_250_000)
        self.assertEqual(g * TREASURY_PCT % 100, 0)
        self.assertEqual(g * LP_PCT % 100, 0)

    def test_full_real_chain_walk_to_exhaustion(self):
        """Walk the real chain from 21,023,800 (200 blocks before the end of
        year 20) to exhaustion, choosing the only consensus-valid spend each
        block (normal while the successor clears MIN_BOX_VALUE, else
        terminal). Ends with the box fully spent, and total paid since
        height 1 equals the genesis box value (177,291,250 coins); adding the
        two 100-coin fund boxes gives the total supply."""
        start = self.END - 200
        # Paid on the real chain over heights 1 .. start-1 (height 0 unpaid),
        # summed per epoch from the contract's own reward function.
        paid_before = (sum(block_reward_contract(e * BLOCKS_PER_HALVING) * BLOCKS_PER_HALVING
                           for e in range(MAX_HALVINGS)) - block_reward_contract(0)
                       - (self.END - start) * MIN_REWARD)
        value = GENESIS_VALUE - paid_before
        self.assertEqual(value, 175 * NANOCOIN)
        self.assertEqual(value, model.real_box_value_at(start))
        h, paid_walk, normal_blocks, terminal_height = start, 0, 0, None
        emission_box_alive = True
        while emission_box_alive:
            r = block_reward_contract(h)
            if value - r >= MIN_BOX_VALUE:
                self_box, outputs = make_normal_spend(h, value)
                ok, why = evaluate_emission_contract(self_box, outputs, h, TREASURY_HASH, LP_HASH)
                self.assertTrue(ok and "normalPath" in why, f"h={h}: {why}")
                self.assertTrue(_consensus_min_value_ok(outputs), f"h={h}")
                paid = value - outputs[0].value
                self.assertEqual(paid, r)
                last_normal = (h, value, outputs[0].value)
                value = outputs[0].value
                normal_blocks += 1
            else:
                self_box, outputs = _terminal_tx(h, value)
                ok, why = evaluate_emission_contract(self_box, outputs, h, TREASURY_HASH, LP_HASH)
                self.assertTrue(ok and "terminalPath" in why, f"h={h}: {why}")
                self.assertTrue(_consensus_min_value_ok(outputs), f"h={h}")
                self.assertFalse(any(t.token_id == EMISSION_NFT_ID for o in outputs for t in o.tokens))
                paid = sum(o.value for o in outputs)
                self.assertEqual(paid, value)
                terminal_split = [o.value for o in outputs]
                value = 0
                terminal_height = h
                emission_box_alive = False
            paid_walk += paid
            h += 1
            if h > self.END + 1000:
                self.fail("walk did not terminate")
        self.assertEqual(normal_blocks, 116)
        self.assertEqual(last_normal, (REAL_LAST_NORMAL, 2_500_000_000, 1_000_000_000))
        self.assertEqual(terminal_height, REAL_TERMINAL)
        self.assertEqual(terminal_split, [40_000_000, 30_000_000, 930_000_000])
        self.assertEqual(value, 0)
        self.assertEqual(paid_walk, 175 * NANOCOIN)
        self.assertEqual(paid_before + paid_walk, GENESIS_VALUE)
        self.assertEqual(paid_before + paid_walk + FUND_BOXES, TOTAL_SUPPLY)

    def test_real_chain_last_normal_and_terminal_spend(self):
        """2026-11-12 genesis. Height 21,023,915: the box holds 2.5 coins,
        the normal path pays the 1.5 reward and leaves a 1.0-coin successor;
        the terminal path rejects (2.5 > 1.5). Height 21,023,916: the box
        holds 1.0 coin, below the reward, so only the terminal path can
        spend it: treasury 0.04, LP 0.03, miner 0.93, NFT burned."""
        self.assertEqual(model.real_terminal_height(), REAL_TERMINAL)
        self.assertEqual(REAL_LAST_NORMAL + 1, REAL_TERMINAL)
        h = REAL_LAST_NORMAL
        v = model.real_box_value_at(h)
        self.assertEqual(v, 2_500_000_000)
        self.assertEqual(block_reward_contract(h), MIN_REWARD)
        self_box, outputs = make_normal_spend(h, v)
        ok, why = evaluate_emission_contract(self_box, outputs, h, TREASURY_HASH, LP_HASH)
        self.assertTrue(ok and "normalPath" in why, why)
        self.assertEqual(outputs[0].value, 1_000_000_000)
        self.assertTrue(_consensus_min_value_ok(outputs))
        t_self, t_out = _terminal_tx(h, v)
        ok, why = evaluate_emission_contract(t_self, t_out, h, TREASURY_HASH, LP_HASH)
        self.assertFalse(ok, why)
        self.assertIn("insufficient", why)

        h = REAL_TERMINAL
        v = model.real_box_value_at(h)
        self.assertEqual(v, 1_000_000_000)
        self.assertLess(v, block_reward_contract(h))
        self_box, outputs = _terminal_tx(h, v)
        ok, why = evaluate_emission_contract(self_box, outputs, h, TREASURY_HASH, LP_HASH)
        self.assertTrue(ok and "terminalPath" in why, why)
        self.assertEqual([o.value for o in outputs], [40_000_000, 30_000_000, 930_000_000])
        self.assertEqual(sum(o.value for o in outputs), v)
        self.assertTrue(_consensus_min_value_ok(outputs))
        for o in outputs:
            self.assertFalse(any(t.token_id == EMISSION_NFT_ID for t in o.tokens))
        # The old equality-case split (1.5-coin box) must reject on the 1.0 box.
        self_box, outputs = _terminal_tx(h, v)
        outputs[0].value, outputs[1].value, outputs[2].value = 60_000_000, 45_000_000, 1_395_000_000
        ok, why = evaluate_emission_contract(self_box, outputs, h, TREASURY_HASH, LP_HASH)
        self.assertFalse(ok, why)

    def test_extra_token_in_emission_box_is_unprotected_at_terminal(self):
        """Second-order token check, rewritten for v1.3. The normal successor
        must hold exactly one token, so a miner can no longer add a token to
        it; the miner output must hold no tokens. On the terminal path a
        token other than the NFT on SELF (only from a hand-built box) may be
        dropped but cannot go to the miner output."""
        extra = Token(bytes([0xAB] * 32), 7)
        h = self.EQ_H
        self_box, outputs = make_normal_spend(h - 1, 2 * MIN_REWARD)
        outputs[0].tokens = [Token(EMISSION_NFT_ID, 1), extra]
        ok, why = evaluate_emission_contract(self_box, outputs, h - 1, TREASURY_HASH, LP_HASH)
        self.assertFalse(ok, why)
        self.assertIn("nftPreserved", why)
        t_self, t_out = _terminal_tx(h, MIN_REWARD)
        t_self.tokens = [Token(EMISSION_NFT_ID, 1), extra]
        ok, why = evaluate_emission_contract(t_self, t_out, h, TREASURY_HASH, LP_HASH)
        self.assertTrue(ok, f"extra token dropped: {why}")
        t_out[2].tokens = [extra]
        ok, why = evaluate_emission_contract(t_self, t_out, h, TREASURY_HASH, LP_HASH)
        self.assertFalse(ok, f"extra token to miner: {why}")
        self.assertIn("miner output holds tokens", why)


# ============================================================
# GAP C: MINER OUTPUT BINDING (2026-11-10)
# ============================================================

class MinerBindingTests(unittest.TestCase):
    """The last output must be rewardOutputScript(2880, CONTEXT.minerPubKey),
    created at HEIGHT, holding exactly the miner share; 4 outputs on the
    normal path, 3 on the terminal path. Each reject has an accepting twin."""

    END = MAX_HALVINGS * BLOCKS_PER_HALVING
    EQ_H = EQ_H  # tail height for the value == reward terminal case
    KEYS = (PK_A, PK_B, PK_C, PK_POOL)

    def _eval(self, self_box, outputs, h, pk=PK_A):
        return evaluate_emission_contract(self_box, outputs, h, TREASURY_HASH, LP_HASH, miner_pk=pk)

    def _cases(self):
        """(name, self_box, outputs, height, miner share, miner index) for a
        normal spend at h = 1000, a terminal spend at value == reward (tail
        height EQ_H, a contract case) and the real-chain terminal spend
        (1.0-coin box at 21,023,916)."""
        h = 1000
        n_self, n_out = make_normal_spend(h, model.real_box_value_at(h))
        t_self, t_out = make_terminal_spend(MIN_REWARD, height=self.EQ_H)
        r_self, r_out = make_terminal_spend(model.real_box_value_at(REAL_TERMINAL), height=REAL_TERMINAL)
        return [
            ("normal", n_self, n_out, h, n_out[3].value, 3),
            ("terminal", t_self, t_out, self.EQ_H, t_out[2].value, 2),
            ("terminal-real", r_self, r_out, REAL_TERMINAL, r_out[2].value, 2),
        ]

    def _twin(self, self_box, outputs, h, pk=PK_A):
        ok, why = self._eval(self_box, outputs, h, pk)
        self.assertTrue(ok, f"twin must accept: {why}")

    def _swap_miner(self, name, mutate, expect_reason="minerOutBound"):
        for case, self_box, outputs, h, m, i in self._cases():
            self._twin(self_box, outputs, h)
            bad = list(outputs)
            bad[i] = mutate(m, h)
            ok, why = self._eval(self_box, bad, h)
            self.assertFalse(ok, f"{case}: {name}: {why}")
            if expect_reason:
                self.assertIn(expect_reason, why, f"{case}: {name}")

    def test_reward_script_bytes_match_scala(self):
        # sigma-state ErgoTreePredef.rewardOutputScript(2880, ProveDlog(G)).
        self.assertEqual(
            miner_reward_script(PK_A).hex(),
            "100204802d08cd0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
            "ea02d192a39a8cc7a70173007301")
        self.assertEqual(len(miner_reward_script(PK_B)), 54)
        self.assertEqual(MINER_REWARD_DELAY, 2880)
        # Int constant 04 + VLQ(zigzag(2880)) = 04 80 2d; 720 = 04 a0 0b.
        z = 2880 * 2
        self.assertEqual(bytes([0x80 | (z & 0x7F), z >> 7]), MINER_SCRIPT_PREFIX[3:5])
        z = 720 * 2
        self.assertEqual(bytes([0x80 | (z & 0x7F), z >> 7]), MINER_SCRIPT_PREFIX_720[3:5])

    def test_accept_all_reward_levels_both_paths(self):
        n = 0
        for e in range(7):
            base = e * BLOCKS_PER_HALVING
            for h in (max(base, 1), base + BLOCKS_PER_HALVING // 2, base + BLOCKS_PER_HALVING - 1):
                pk = self.KEYS[n % 4]
                n += 1
                self_box, outputs = make_normal_spend(h, model.emission_box_value_at(h))
                outputs[3] = make_miner_box(outputs[3].value, h, pk)
                ok, why = self._eval(self_box, outputs, h, pk)
                self.assertTrue(ok, f"normal e={e} h={h}: {why}")
            h = base + 1
            r = block_reward_contract(h)
            for v in (r, r - 93_750_000):
                self_box, outputs = make_terminal_spend(v, height=h)
                ok, why = self._eval(self_box, outputs, h)
                self.assertTrue(ok and "terminalPath" in why, f"terminal e={e} v={v}: {why}")

    def test_binding_holds_at_every_halving_boundary(self):
        for e in range(1, 7):
            b = e * BLOCKS_PER_HALVING
            for h in (b - 1, b, b + 1):
                self_box, outputs = make_normal_spend(h, model.emission_box_value_at(h))
                self._twin(self_box, outputs, h)
                bad = list(outputs)
                bad[3] = make_miner_box(outputs[3].value, h, PK_C)
                self.assertFalse(self._eval(self_box, bad, h)[0], f"wrong key h={h}")
                other = block_reward_contract(b) if h < b else block_reward_contract(b - 1)
                bad[3] = make_miner_box(split_contract(other)[0], h)
                self.assertFalse(self._eval(self_box, bad, h)[0], f"neighbour share h={h}")

    def test_reject_wrong_pk(self):
        self._swap_miner("script for key B", lambda m, h: make_miner_box(m, h, PK_B))

    def test_reject_any_other_key_and_header_decides(self):
        for hk in self.KEYS:
            for sk in self.KEYS:
                for case, self_box, outputs, h, m, i in self._cases():
                    bad = list(outputs)
                    bad[i] = make_miner_box(m, h, sk)
                    ok, why = self._eval(self_box, bad, h, hk)
                    self.assertEqual(ok, hk == sk, f"{case} header {hk.hex()[:6]} script {sk.hex()[:6]}: {why}")

    def test_reject_p2pk_without_delay(self):
        self._swap_miner("P2PK", lambda m, h: Box(value=m, proposition_bytes=bytes.fromhex("0008cd") + PK_A,
                                                   creation_height=h))

    def test_reject_delay_720(self):
        self._swap_miner("delay 720", lambda m, h: Box(
            value=m, proposition_bytes=miner_reward_script(PK_A, MINER_SCRIPT_PREFIX_720), creation_height=h))

    def test_reject_contract_script(self):
        self._swap_miner("contract", lambda m, h: Box(value=m, proposition_bytes=TREASURY_SCRIPT_BYTES,
                                                       creation_height=h))

    def test_reject_value_one_nano_high_or_low(self):
        self._swap_miner("value - 1", lambda m, h: make_miner_box(m - 1, h), expect_reason="Miner: miner value")
        self._swap_miner("value + 1", lambda m, h: make_miner_box(m + 1, h), expect_reason="Miner: miner value")

    def test_reject_creation_height_not_height(self):
        for off in (lambda h: 0, lambda h: h - 1, lambda h: h + 1, lambda h: h - MINER_REWARD_DELAY):
            self._swap_miner("creation height", lambda m, h: make_miner_box(m, off(h)))

    def test_reject_wrong_index(self):
        for case, self_box, outputs, h, m, i in self._cases():
            for j in range(i):
                bad = list(outputs)
                bad[i], bad[j] = bad[j], bad[i]
                self.assertFalse(self._eval(self_box, bad, h)[0], f"{case}: miner at index {j}")

    def test_reject_missing_miner_output(self):
        for case, self_box, outputs, h, m, i in self._cases():
            self._twin(self_box, outputs, h)
            ok, why = self._eval(self_box, outputs[:i], h)
            self.assertFalse(ok, f"{case}: no miner output: {why}")

    def test_reject_extra_output_siphoning_value(self):
        x = 100_000_000
        siphon = lambda h: Box(value=x, proposition_bytes=bytes.fromhex("0008cd") + PK_B, creation_height=h)
        for case, self_box, outputs, h, m, i in self._cases():
            self._twin(self_box, outputs, h)
            for bad in (
                outputs[:i] + [siphon(h), make_miner_box(m - x, h)],
                outputs[:i] + [siphon(h), make_miner_box(m, h)],
                outputs + [siphon(h)],
                outputs[:i] + [make_miner_box(x, h), make_miner_box(m, h)],
            ):
                ok, why = self._eval(self_box, bad, h)
                self.assertFalse(ok, f"{case}: siphon: {why}")

    def test_reject_share_split_across_two_outputs(self):
        for case, self_box, outputs, h, m, i in self._cases():
            self._twin(self_box, outputs, h)
            bad = outputs[:i] + [make_miner_box(m // 2, h), make_miner_box(m - m // 2, h)]
            ok, why = self._eval(self_box, bad, h)
            self.assertFalse(ok, f"{case}: split share: {why}")

    def test_reject_nft_in_bound_miner_output_at_terminal(self):
        self_box, outputs = make_terminal_spend(MIN_REWARD, height=self.EQ_H)
        self._twin(self_box, outputs, self.EQ_H)
        outputs[2].tokens = [Token(EMISSION_NFT_ID, 1)]
        ok, why = self._eval(self_box, outputs, self.EQ_H)
        self.assertFalse(ok, why)
        self.assertIn("nftBurned", why)


class V13TokenPinTests(unittest.TestCase):
    """emission.es v1.3 (2026-11 retest): constant NFT id with SELF.tokens(0)
    == (id, 1); no tokens on the miner output on both paths; the normal
    successor holds exactly one token and no R6. Each reject has a twin."""

    EQ_H = EQ_H  # tail height for the value == reward terminal case

    def _cases(self):
        h = 1_000
        s, outs = make_normal_spend(h, 200 * NANOCOIN)
        yield "normal", s, outs, h
        s, outs = make_terminal_spend(MIN_REWARD, height=self.EQ_H)
        yield "terminal", s, outs, self.EQ_H

    def _eval(self, s, o, h):
        return evaluate_emission_contract(s, o, h, TREASURY_HASH, LP_HASH)

    def test_honest_twins_accept(self):
        for name, s, o, h in self._cases():
            ok, why = self._eval(s, o, h)
            self.assertTrue(ok, f"{name}: {why}")

    def test_reject_lookalike_other_token_id(self):
        import copy
        for name, s, o, h in self._cases():
            s2, o2 = copy.deepcopy(s), copy.deepcopy(o)
            s2.tokens = [Token(bytes([0x42] * 32), 1)]
            if name == "normal":
                o2[0].tokens = [Token(bytes([0x42] * 32), 1)]
            ok, why = self._eval(s2, o2, h)
            self.assertFalse(ok, name)
            self.assertIn("selfIsEmission", why)

    def test_reject_two_units_on_self(self):
        import copy
        for name, s, o, h in self._cases():
            s2 = copy.deepcopy(s)
            s2.tokens = [Token(EMISSION_NFT_ID, 2)]
            ok, why = self._eval(s2, o, h)
            self.assertFalse(ok, name)
            self.assertIn("selfIsEmission", why)

    def test_reject_token_on_miner_output_both_paths(self):
        import copy
        for name, s, o, h in self._cases():
            o2 = copy.deepcopy(o)
            o2[-1].tokens = [Token(bytes([0x5A] * 32), 1)]
            ok, why = self._eval(s, o2, h)
            self.assertFalse(ok, name)
            self.assertIn("miner output holds tokens", why)

    def test_reject_extra_token_or_r6_on_successor(self):
        import copy
        _, s, o, h = next(self._cases())
        o2 = copy.deepcopy(o)
        o2[0].tokens = [Token(EMISSION_NFT_ID, 1), Token(bytes([0x5A] * 32), 1000)]
        ok, why = self._eval(s, o2, h)
        self.assertFalse(ok)
        self.assertIn("nftPreserved", why)
        o3 = copy.deepcopy(o)
        o3[0].r6 = b"\x00" * 2000
        ok, why = self._eval(s, o3, h)
        self.assertFalse(ok)
        self.assertIn("R6 present", why)


# ============================================================
# Test runner with summary
# ============================================================

if __name__ == "__main__":
    loader = unittest.TestLoader()
    suite = unittest.TestSuite()
    for cls in [
        CrossOracleTests,
        RoundingTests,
        NormalPathBoundaryTests,
        NormalPathRejectTests,
        NormalPathOverpayRejectTests,
        TerminalPathAcceptTests,
        TerminalPathRejectTests,
        PathSelectionTests,
        BoundsTests,
        RetuneScheduleTests,
        TerminalLeFixTests,
        MinerBindingTests,
        SharePlainTests,
        V13TokenPinTests,
    ]:
        suite.addTests(loader.loadTestsFromTestCase(cls))

    runner = unittest.TextTestRunner(verbosity=2)
    result = runner.run(suite)

    print()
    print("=" * 72)
    print(f"TOTAL: {result.testsRun}  FAILURES: {len(result.failures)}  ERRORS: {len(result.errors)}")
    print("=" * 72)
    sys.exit(0 if result.wasSuccessful() else 1)
