{
  // ================================================================
  // EMISSION CONTRACT v1.4
  // ================================================================
  // v1.4 (2026-11-19): reward split changed from 85 / 10 / 5 to 93 / 4 / 3
  //   (miner / treasury / LP), on both paths. Four constants; nothing
  //   else changed. Every share is an exact number of nanocoins at every
  //   reward level (75 down to 1.5) and for the 1.0 final box.
  // v1.3 (2026-11 retest): the emission NFT id is a constant and SELF must
  //   hold it with amount 1 (F-022); the miner output holds no tokens on
  //   both paths; the normal-path successor holds exactly one token and no
  //   R6 (so no minted token and no padding ride along).
  // v1.2 (2026-11 review): share outputs pinned (F-009); header names the
  //   accumulation scripts for R4 and R5 (F-015).
  //
  // Changes from v1.0 (Ergo-parity pass):
  //   - [Finding 1]  Treasury/LP value checks tightened from `>=` to `==`
  //                  to match Ergo's `EQ(coinsToIssue, consumedFromEmission)`
  //                  convention in emissionBoxProp.
  //   - [Gap A]      Added `heightIncreased` check: HEIGHT > SELF.creationInfo._1
  //                  Mirrors Ergo's belt-and-suspenders defense against
  //                  same-block spend of the emission box.
  //   - [Gap B]      Added `heightCorrect` check: OUTPUTS(0).creationInfo._1 == HEIGHT
  //                  Mirrors Ergo's creation-height lock on the successor box.
  //
  // 30 s retune (2026-11-09 locked block package): blocksPerHalving
  //   1577880 -> 1051200, initialReward 50 -> 75 YOLO, minReward 1 -> 1.5
  //   YOLO, sixth halving arm (initialReward / 32L) restored. Total supply 177,291,450
  //   YOLO (20 chain years) = emission box 177,291,250 + 100 in each fund box (2026-11-12).
  //
  // Miner output binding (2026-11-10, Gap C): the miner output is pinned in
  //   this contract, as Ergo's `correctMinerOutput` does. No node rule
  //   constrains it, so this contract is the only thing that does. Its
  //   script must be the stock Ergo miner reward script
  //   rewardOutputScript(2880, minerPk) for the block's header key, its
  //   creation height must be HEIGHT (the script's delay counts from it),
  //   it must hold no tokens (v1.3), and its value must be exactly the
  //   miner share. Output count pinned:
  //   4 on the normal path, 3 on the terminal path. The miner output is
  //   the last output in both.
  //
  // Reference: ErgoScriptPredef.scala `emissionBoxProp` at commit bd1906e,
  // ScorexFoundation/sigmastate-interpreter.
  //
  // Share outputs (2026-11 review, F-009): each of the four treasury and
  //   LP share outputs (normal and terminal path) must carry no tokens and
  //   no registers and have creation height HEIGHT, so a miner cannot pad
  //   a share box to raise the storage rent it pays while it waits to be
  //   forwarded.
  //
  // NFT ids (v1.3). emissionNftId is a constant (placeholder
  //   EMISSION_NFT_ID_PLACEHOLDER, substituted at compile time like the
  //   treasury and LP ids). The three NFT ids (emission, treasury, LP) must
  //   be distinct, and none may equal the id of any box that can ever be
  //   spent: consensus mints a new token whenever its id equals
  //   INPUTS(0).id, so the first spender of such a box could mint a second
  //   unit. Use ids that are not box ids (for example blake2b256 of distinct
  //   domain strings).
  //
  // Box layout:
  //   SELF          = current emission box
  //   SELF.tokens(0)= (emissionNftId, 1), checked on every spend
  //   SELF.R4       = Coll[Byte]: blake2b256 of the TREASURY ACCUMULATION
  //                   script tree (treasury_accumulation.es), not of the
  //                   treasury governance script
  //   SELF.R5       = Coll[Byte]: blake2b256 of the LP ACCUMULATION script
  //                   tree (lp_accumulation.es), not of the LP fund script
  //   This script never checks R4 and R5; it only copies them to the
  //   successor. They are written once, in the genesis box, so a wrong
  //   value there pays every share into the wrong script for the life of
  //   the chain (F-015). Launch check: compile both accumulation scripts
  //   from the release commit and compare their hashes with R4 and R5 of
  //   the genesis box read back from the running chain.
  //   The emission NFT id is a constant and SELF must hold it at
  //   tokens(0) with amount 1, so a lookalike box at this script with
  //   another token cannot be spent (F-022). The node should still track
  //   the real emission box by its NFT id.
  //
  // Transaction layout (normal path), exactly 4 outputs:
  //   OUTPUTS(0)    = new emission box (value decreased by blockReward),
  //                   exactly one token (the NFT), no R6
  //   OUTPUTS(1)    = treasury reward output
  //   OUTPUTS(2)    = LP fund reward output
  //   OUTPUTS(3)    = miner reward output, value == blockReward - treasury - LP,
  //                   no tokens
  //
  // Transaction layout (terminal path, exhaustion), exactly 3 outputs:
  //   When SELF.value <= blockReward, all remaining value is distributed
  //   proportionally and the emission NFT is burned.
  //   OUTPUTS(0)    = treasury, value == SELF.value * 4 / 100
  //   OUTPUTS(1)    = LP fund,  value == SELF.value * 3 / 100
  //   OUTPUTS(2)    = miner reward output, value == SELF.value - treasury - LP,
  //                   no tokens
  //   `<=` (2026-11-10): at SELF.value == blockReward the normal path would
  //   need a 0-value successor, which consensus rejects, so with `<` the
  //   box could never be spent. At equality the terminal split equals the
  //   normal split.
  // ================================================================

  // ---- Constants ----
  val blocksPerHalving: Int = 1051200     // 1 year (365 d) at 30 s blocks
  val initialReward: Long   = 75000000000L // 75 coins in nanocoins
  val minReward: Long        = 1500000000L  // 1.5 coins: tail emission floor

  // ---- Singleton NFT identity (constant, v1.3) ----
  val emissionNftId: Coll[Byte] = fromBase16("EMISSION_NFT_ID_PLACEHOLDER")
  // SELF must hold the one-unit emission NFT at tokens(0).
  val selfIsEmission: Boolean = SELF.tokens(0) == (emissionNftId, 1L)

  // ---- Height-based defenses (new in v1.1, match Ergo emissionBoxProp) ----
  // [Gap A] HEIGHT must have advanced since this box was created.
  // Prevents any same-block re-spend scenario; redundant with NFT singleton
  // but matches Ergo's belt-and-suspenders pattern.
  val heightIncreased: Boolean = HEIGHT > SELF.creationInfo._1

  // ---- Determine block reward from HEIGHT ----
  val halvings: Int = HEIGHT / blocksPerHalving

  val blockReward: Long = {
    // Six explicit arms (halvings 0..5): 75, 37.5, 18.75, 9.375, 4.6875,
    // 2.34375 YOLO/block, then the contract floors at minReward = 1.5
    // YOLO/block. The halvings == 5 arm is integral at 30 s blocks:
    // initialReward / 32 = 2,343,750,000 nano, and 2.34375 x 1051200 =
    // 2,463,750 YOLO (1051200 is divisible by 64). The 20 s schedule had
    // dropped this arm in the June 2026 ".5 fix" because 1577880 is not
    // divisible by 16.
    val computed: Long = if (halvings <= 0) initialReward
      else if (halvings == 1) initialReward / 2L
      else if (halvings == 2) initialReward / 4L
      else if (halvings == 3) initialReward / 8L
      else if (halvings == 4) initialReward / 16L
      else if (halvings == 5) initialReward / 32L
      else minReward

    // Floor: never below minReward
    if (computed > minReward) computed else minReward
  }

  // ---- Split calculation ----
  // 4% treasury, 3% LP, miner gets the residual (93%), checked below
  val treasuryReward: Long = blockReward * 4L / 100L
  val lpReward: Long       = blockReward * 3L / 100L

  // ---- Register-stored destination hashes ----
  // Hashes of the two accumulation scripts, set in the genesis box.
  val treasuryScriptHash: Coll[Byte] = SELF.R4[Coll[Byte]].get
  val lpScriptHash: Coll[Byte]       = SELF.R5[Coll[Byte]].get

  // ---- Share outputs carry nothing but value (F-009) ----
  // No tokens, no registers (R4 absent means none: registers are dense
  // from R4), creation height HEIGHT. Applied to all four share outputs.
  val plainShare = { (b: Box) =>
    b.tokens.size == 0 && b.R4[Coll[Byte]].isEmpty && b.creationInfo._1 == HEIGHT
  }

  // ---- Miner output binding (Gap C, both paths) ----
  // Expected script = serialized ErgoTreePredef.rewardOutputScript(2880, pk):
  //   10 02 | 04 80 2d (Int 2880) | 08 cd | <33-byte pk> | ea02d192a39a8cc7a70173007301
  // i.e. `proveDlog(pk) && HEIGHT >= SELF.creationInfo._1 + 2880` (24 h
  // at 30 s). Only the pk bytes vary. CONTEXT.minerPubKey is the block
  // header key, 33 bytes compressed. Ergo builds the same bytes with
  // substConstants; concatenation gives identical bytes for a valid key.
  val minerRewardScript: Coll[Byte] =
    fromBase16("100204802d08cd") ++ CONTEXT.minerPubKey ++
    fromBase16("ea02d192a39a8cc7a70173007301")
  // Last output in both layouts. Indexing from OUTPUTS.size keeps one
  // shared read that exists on both paths (a fixed OUTPUTS(3) would be
  // read on the 3-output terminal path too and fail there).
  val minerOut: Box = OUTPUTS(OUTPUTS.size - 1)
  // Creation height pinned: the delay counts from the box's creation height.
  // No tokens (v1.3): a token minted in this transaction cannot leave on it.
  val minerOutBound: Boolean =
    minerOut.propositionBytes == minerRewardScript &&
    minerOut.creationInfo._1 == HEIGHT &&
    minerOut.tokens.size == 0

  // ================================================================
  // PATH 1: NORMAL EMISSION (SELF.value >= blockReward)
  // ================================================================
  val normalPath: Boolean = {
    val sufficientFunds: Boolean = SELF.value >= blockReward

    // -- Output 0: new emission box --
    val nextBox = OUTPUTS(0)
    // Exactly one token (v1.3). The size test comes first, so on the
    // terminal path (O0 is a tokenless share box) nothing below is read.
    val nftPreserved: Boolean = {
      nextBox.tokens.size == 1 &&
      nextBox.tokens(0)._1 == emissionNftId &&
      nextBox.tokens(0)._2 == 1L
    }
    val scriptPreserved: Boolean = nextBox.propositionBytes == SELF.propositionBytes
    val valueCorrect: Boolean    = nextBox.value == SELF.value - blockReward
    // No R6 (v1.3): registers are dense, so R6 absent means only R4, R5.
    val registersPreserved: Boolean = {
      nextBox.R4[Coll[Byte]].get == treasuryScriptHash &&
      nextBox.R5[Coll[Byte]].get == lpScriptHash &&
      nextBox.R6[Coll[Byte]].isEmpty
    }
    // [Gap B] New emission box must declare creation height equal to current HEIGHT.
    val heightCorrect: Boolean = nextBox.creationInfo._1 == HEIGHT

    val validEmissionBox: Boolean =
      nftPreserved && scriptPreserved && valueCorrect && registersPreserved && heightCorrect

    // -- Output 1: treasury --  [Finding 1: == not >=; F-009 plainShare]
    val validTreasury: Boolean = {
      OUTPUTS(1).value == treasuryReward &&
      blake2b256(OUTPUTS(1).propositionBytes) == treasuryScriptHash &&
      plainShare(OUTPUTS(1))
    }

    // -- Output 2: LP fund --  [Finding 1: == not >=; F-009 plainShare]
    val validLP: Boolean = {
      OUTPUTS(2).value == lpReward &&
      blake2b256(OUTPUTS(2).propositionBytes) == lpScriptHash &&
      plainShare(OUTPUTS(2))
    }

    // -- Output 3: miner (script and height bound above) --
    val validMiner: Boolean =
      OUTPUTS.size == 4 && minerOut.value == blockReward - treasuryReward - lpReward

    sufficientFunds && validEmissionBox && validTreasury && validLP && validMiner
  }

  // ================================================================
  // PATH 2: TERMINAL EMISSION (SELF.value <= blockReward)
  // ================================================================
  // When the emission box holds at most one block reward,
  // distribute all remaining value proportionally and end emission.
  // The emission NFT is burned (not preserved in any output).
  // At SELF.value == blockReward both paths pass this script; only the
  // terminal one is consensus-valid (the normal successor would be 0-value).
  val terminalPath: Boolean = {
    val insufficient: Boolean = SELF.value <= blockReward
    val remaining: Long       = SELF.value

    // Proportional split of whatever's left
    val termTreasury: Long = remaining * 4L / 100L
    val termLP: Long       = remaining * 3L / 100L
    // Miner gets the rest, in OUTPUTS(2) (validTermMiner below)

    // [Finding 1: == not >=; F-009 plainShare]
    val validTermTreasury: Boolean = {
      OUTPUTS(0).value == termTreasury &&
      blake2b256(OUTPUTS(0).propositionBytes) == treasuryScriptHash &&
      plainShare(OUTPUTS(0))
    }
    // [Finding 1: == not >=; F-009 plainShare]
    val validTermLP: Boolean = {
      OUTPUTS(1).value == termLP &&
      blake2b256(OUTPUTS(1).propositionBytes) == lpScriptHash &&
      plainShare(OUTPUTS(1))
    }

    // Explicitly verify NFT is destroyed: it cannot be smuggled into any output
    val nftBurned: Boolean = OUTPUTS.forall { (o: Box) =>
      o.tokens.forall { (t: (Coll[Byte], Long)) => t._1 != emissionNftId }
    }

    // -- Output 2: miner (script and height bound above) --
    val validTermMiner: Boolean =
      OUTPUTS.size == 3 && minerOut.value == remaining - termTreasury - termLP

    insufficient && validTermTreasury && validTermLP && nftBurned && validTermMiner
  }

  // [Gap A] heightIncreased, [Gap C] minerOutBound and the SELF NFT check
  // (v1.3) are required on EVERY spend (both paths).
  sigmaProp(selfIsEmission && heightIncreased && minerOutBound && (normalPath || terminalPath))
}
