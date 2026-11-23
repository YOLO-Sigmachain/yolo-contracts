"""
emission_model.py — Reference emission model for SigmaChain

This is the source of truth. The ErgoScript contract and all tests
are validated against this model. If this model and the contract
disagree, investigate both — but this model defines intent.

All values in nanocoins (1 coin = 1_000_000_000 nanocoins).
"""

# ============================================================
# PARAMETERS — change these, everything else recalculates
# ============================================================

NANOCOIN = 1_000_000_000

BLOCK_TIME_SECONDS = 30
BLOCKS_PER_YEAR = 365 * 24 * 3600 // BLOCK_TIME_SECONDS  # 1,051,200 (365-day year)

INITIAL_REWARD = 75 * NANOCOIN          # 75 coins/block in nanocoins
BLOCKS_PER_HALVING = BLOCKS_PER_YEAR    # halves every year
NUM_EXPLICIT_HALVINGS = 6                # halvings 0..5 produce 75, 37.5, 18.75,
                                         # 9.375, 4.6875, 2.34375 YOLO; halvings
                                         # >= 6 floor at MIN_REWARD. Integral
                                         # because 1,051,200 is divisible by 64
                                         # (2.34375 * 1,051,200 = 2,463,750).
                                         # The 20 s schedule had to stop at 5
                                         # (June 2026 ".5 fix"); 30 s restores
                                         # the sixth arm.
MAX_HALVINGS = 20                        # chain years in the schedule total
                                         # (6 arm years + 14 tail years). Also
                                         # the display / loop bound; the
                                         # contract-matching reward switch is
                                         # NUM_EXPLICIT_HALVINGS above.
MIN_REWARD = 1_500_000_000               # 1.5 coins: tail floor reward

# Total supply, locked 2026-11-09 (30 s block package): the sum of the
# schedule above over MAX_HALVINGS chain years. total_supply() DERIVES it;
# the assert at the bottom of the core functions pins it to this literal.
TOTAL_SUPPLY_LITERAL = 177_291_450_000_000_000  # 177,291,450 YOLO

# Genesis fund boxes (decided 2026-11-12): the treasury fund box
# and the LP fund box are each created at genesis holding 100 YOLO, and
# that 200 YOLO comes out of the emission box. Total supply is unchanged.
GENESIS_TREASURY_FUND_BOX = 100 * NANOCOIN
GENESIS_LP_FUND_BOX = 100 * NANOCOIN

# Genesis emission box: total supply minus the two fund boxes.
GENESIS_EMISSION_BOX_LITERAL = 177_291_250_000_000_000  # 177,291,250 YOLO

# Split percentages (must sum to 100)
MINER_PCT = 93
TREASURY_PCT = 4
# LP gets remainder: 100 - 93 - 4 = 3

# ============================================================
# CORE FUNCTIONS
# ============================================================

def block_reward(height: int) -> int:
    """Block reward in nanocoins at a given height.

    Matches emission.es: halvings 0..NUM_EXPLICIT_HALVINGS-1 (=0..5) shift
    right by 2^halvings; halvings >= NUM_EXPLICIT_HALVINGS floor at
    MIN_REWARD (1.5 YOLO). See NUM_EXPLICIT_HALVINGS doc.
    """
    halvings = height // BLOCKS_PER_HALVING
    if halvings >= NUM_EXPLICIT_HALVINGS:
        return MIN_REWARD
    reward = INITIAL_REWARD >> halvings  # integer right-shift = divide by 2^n
    return max(reward, MIN_REWARD)


def split_reward(reward: int) -> tuple[int, int, int]:
    """
    Split block reward into (miner, treasury, lp) in nanocoins.
    LP gets remainder to avoid rounding loss.
    """
    miner = reward * MINER_PCT // 100
    treasury = reward * TREASURY_PCT // 100
    lp = reward - miner - treasury
    return miner, treasury, lp


def total_supply() -> int:
    """
    Calculate exact total supply by summing every halving epoch.
    Does NOT simulate block-by-block — uses epoch math.

    Routes through `block_reward` so the contract-matching switch
    (NUM_EXPLICIT_HALVINGS) is the single source of truth.
    """
    total = 0
    for h in range(MAX_HALVINGS):
        representative_height = h * BLOCKS_PER_HALVING
        reward = block_reward(representative_height)
        total += reward * BLOCKS_PER_HALVING
    # Tail emission after MAX_HALVINGS (open-ended, but bound for display)
    # Not included — total_supply() returns the sum of the first MAX_HALVINGS epochs only
    return total


def genesis_box_value() -> int:
    """
    How many nanocoins the genesis emission box holds: total supply minus
    the 100 YOLO treasury fund box and the 100 YOLO LP fund box
    (2026-11-12). 177,291,250 YOLO.
    """
    return total_supply() - GENESIS_TREASURY_FUND_BOX - GENESIS_LP_FUND_BOX


def emission_box_value_at(height: int, start: int = None) -> int:
    """
    Expected emission box value at a given height, model view: every height
    in [0, height) is paid. Starts at `start` (default genesis_box_value()),
    decreases by block_reward each block. Pass start=total_supply() for the
    schedule view (the full total supply, which runs out exactly at the end
    of chain year 20). The real chain never mines height 0; see
    real_box_value_at().
    """
    value = genesis_box_value() if start is None else start
    # Sum rewards for all complete epochs before current
    epoch = height // BLOCKS_PER_HALVING
    blocks_in_partial = height % BLOCKS_PER_HALVING

    for h in range(min(epoch, MAX_HALVINGS)):
        reward = block_reward(h * BLOCKS_PER_HALVING)
        value -= reward * BLOCKS_PER_HALVING

    # Partial epoch
    reward = block_reward(height) if epoch < MAX_HALVINGS else MIN_REWARD
    value -= reward * blocks_in_partial

    return value


def final_emission_height(start: int = None) -> int:
    """
    Height at which emission box reaches 0 or below, model view (height 0
    paid). Starts at `start` (default genesis_box_value()). With
    start=total_supply() this is 21,024,000, the end of chain year 20.
    After this, no more emission (or tail emission from fees/rent).
    """
    remaining = genesis_box_value() if start is None else start
    height = 0
    for h in range(MAX_HALVINGS):
        reward = block_reward(h * BLOCKS_PER_HALVING)
        epoch_total = reward * BLOCKS_PER_HALVING
        if remaining <= epoch_total:
            # exhaustion happens in this epoch
            blocks_left = remaining // reward
            return height + blocks_left
        remaining -= epoch_total
        height += BLOCKS_PER_HALVING
    # tail emission — will exhaust eventually if MIN_REWARD > 0
    if MIN_REWARD > 0:
        blocks_left = remaining // MIN_REWARD
        return height + blocks_left
    return height


def real_box_value_at(height: int) -> int:
    """
    Real-chain emission box value immediately before the spend at `height`
    (height >= 1). Height 0 is never mined, so heights 1 .. height-1 are
    paid. Negative once the box is spent out (after the terminal spend).
    """
    paid = 0
    for e in range(height // BLOCKS_PER_HALVING + 1):
        lo = max(1, e * BLOCKS_PER_HALVING)
        hi = min(height - 1, (e + 1) * BLOCKS_PER_HALVING - 1)
        if hi >= lo:
            paid += block_reward(lo) * (hi - lo + 1)
    return genesis_box_value() - paid


def real_terminal_height() -> int:
    """
    First height at which the real-chain box holds at most one block reward,
    i.e. the height of the terminal-path spend (emission.es `insufficient`).
    21,023,916 for the 2026-11-12 genesis (box 1.0 YOLO, reward 1.5).
    """
    lo, hi = 1, (MAX_HALVINGS + 1) * BLOCKS_PER_HALVING
    while lo < hi:
        mid = (lo + hi) // 2
        if real_box_value_at(mid) <= block_reward(mid):
            hi = mid
        else:
            lo = mid + 1
    return lo


# Derive-then-pin: the schedule must reproduce the locked total supply, and
# the emission box plus the two fund boxes must equal it.
assert total_supply() == TOTAL_SUPPLY_LITERAL, (
    f"derived total supply {total_supply()} != locked literal "
    f"{TOTAL_SUPPLY_LITERAL}"
)
assert genesis_box_value() == GENESIS_EMISSION_BOX_LITERAL, (
    f"derived genesis {genesis_box_value()} != locked literal "
    f"{GENESIS_EMISSION_BOX_LITERAL}"
)
assert (GENESIS_EMISSION_BOX_LITERAL + GENESIS_TREASURY_FUND_BOX
        + GENESIS_LP_FUND_BOX == TOTAL_SUPPLY_LITERAL)


# ============================================================
# VALIDATION HELPERS
# ============================================================

def validate_split(height: int) -> bool:
    """Verify miner + treasury + lp == block_reward at height."""
    reward = block_reward(height)
    miner, treasury, lp = split_reward(reward)
    return miner + treasury + lp == reward


def validate_no_overflow(height: int) -> bool:
    """Verify no intermediate calculation exceeds i64 range."""
    reward = block_reward(height)
    max_i64 = (2**63) - 1
    # Worst case intermediate: reward * 93
    return (reward * MINER_PCT) < max_i64


# ============================================================
# REPORTING
# ============================================================

def print_schedule():
    """Print the full emission schedule."""
    print("=" * 72)
    print("EMISSION SCHEDULE")
    print("=" * 72)
    print(f"Block time:          {BLOCK_TIME_SECONDS}s")
    print(f"Blocks per year:     {BLOCKS_PER_YEAR:,}")
    print(f"Blocks per halving:  {BLOCKS_PER_HALVING:,}")
    print(f"Initial reward:      {INITIAL_REWARD / NANOCOIN} coins")
    print(f"Min reward:          {MIN_REWARD / NANOCOIN} coins")
    print(f"Max halvings:        {MAX_HALVINGS}")
    print(f"Split:               {MINER_PCT}% miner / {TREASURY_PCT}% treasury / {100-MINER_PCT-TREASURY_PCT}% LP")
    print()

    print(f"{'Epoch':<6} {'Year':<6} {'Height Range':<28} {'Reward/Block':<16} {'Epoch Supply':<20} {'Cumulative':<20}")
    print("-" * 100)

    cumulative = 0
    for h in range(MAX_HALVINGS):
        start = h * BLOCKS_PER_HALVING
        reward = block_reward(start)
        end = (h + 1) * BLOCKS_PER_HALVING - 1
        epoch_supply = reward * BLOCKS_PER_HALVING
        cumulative += epoch_supply
        coins = reward / NANOCOIN
        epoch_coins = epoch_supply / NANOCOIN
        cum_coins = cumulative / NANOCOIN

        print(f"{h:<6} {h+1:<6} {start:>12,} - {end:>12,}  {coins:>12.5f}    {epoch_coins:>16,.1f}  {cum_coins:>16,.1f}")

        if reward == MIN_REWARD:
            print(f"  ... (minimum reward reached, tail emission continues)")
            break

    print()
    print(f"Total supply:        {total_supply() / NANOCOIN:,.1f} coins")
    print(f"Genesis box value:   {genesis_box_value() / NANOCOIN:,.1f} coins "
          f"(+ {GENESIS_TREASURY_FUND_BOX / NANOCOIN:,.0f} treasury fund box, "
          f"+ {GENESIS_LP_FUND_BOX / NANOCOIN:,.0f} LP fund box)")
    print(f"Real-chain terminal height: {real_terminal_height():,} "
          f"(box {real_box_value_at(real_terminal_height()) / NANOCOIN} coins)")
    print(f"Final emission ~height: {final_emission_height():,}")
    print(f"Final emission ~year:   {final_emission_height() / BLOCKS_PER_YEAR:.1f}")
    print()

    # Validate splits at every halving boundary
    print("SPLIT VALIDATION")
    print("-" * 72)
    for h in range(min(MAX_HALVINGS + 1, 22)):
        height = h * BLOCKS_PER_HALVING
        reward = block_reward(height)
        miner, treasury, lp = split_reward(reward)
        ok = "OK" if miner + treasury + lp == reward else "FAIL"
        print(f"  Height {height:>12,}: reward={reward/NANOCOIN:>10.5f}  "
              f"miner={miner/NANOCOIN:>10.4f}  treasury={treasury/NANOCOIN:>10.4f}  "
              f"lp={lp/NANOCOIN:>10.4f}  sum_check={ok}")

    # Overflow check
    print()
    print("OVERFLOW VALIDATION")
    print("-" * 72)
    ok = validate_no_overflow(0)
    print(f"  Height 0 (max reward): {'OK' if ok else 'FAIL'}")
    print(f"  Max intermediate value: {INITIAL_REWARD * MINER_PCT:,} (i64 max: {(2**63)-1:,})")


def print_halving_boundaries():
    """Print detailed values at every halving boundary ±1 block."""
    print()
    print("HALVING BOUNDARY DETAIL")
    print("=" * 72)
    for h in range(min(NUM_EXPLICIT_HALVINGS + 1, MAX_HALVINGS)):
        boundary = h * BLOCKS_PER_HALVING
        for offset in [-1, 0, 1]:
            height = boundary + offset
            if height < 0:
                continue
            reward = block_reward(height)
            miner, treasury, lp = split_reward(reward)
            box_val = emission_box_value_at(height)
            print(f"  H={height:>12,}  reward={reward/NANOCOIN:>10.5f}  "
                  f"miner={miner/NANOCOIN:>10.4f}  treasury={treasury/NANOCOIN:>10.4f}  "
                  f"lp={lp/NANOCOIN:>10.4f}  box={box_val/NANOCOIN:>16,.1f}")
        print()


if __name__ == "__main__":
    print_schedule()
    print_halving_boundaries()
