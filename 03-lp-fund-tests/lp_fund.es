{
  // ================================================================
  // LP FUND GOVERNANCE CONTRACT v2.2
  // ================================================================
  // Governs the 3% LP / market-making share of emission. Funds leave
  // only to whitelisted script hashes, each usable only after a delay
  // from the height it was added, or through a delayed migration to a
  // script whose hash was recorded at approval. Every path needs 2 of
  // the 3 signers below.
  //
  // v2.0 (2026-11 review fixes): NFT id is a constant and SELF must hold
  // it (F-014, F-023); whitelist entries carry the height they were
  // added and a spend to an entry waits the delay from that height
  // (F-005, option a); a spend may carry one miner fee output (F-017);
  // migration approval records a height in [HEIGHT - 10, HEIGHT]
  // (F-016) and the target script hash, and execute checks it and a
  // value floor (F-006); migration execute must happen inside a window
  // after the delay, and a migration approval can be cancelled (F-007);
  // freeze keeps R6 and R7 and can be undone by an unfreeze path (F-012,
  // F-020); output registers are read only on paths whose successor is
  // at this script (F-024).
  //
  // v2.1 (2026-11 retest): every path whose O0 stays at this script keeps
  // O0.value >= min(SELF.value, fundFloor) and O0's creation height at
  // least HEIGHT - 10, so the fund box cannot be drained below the floor
  // and each signed spend restarts its storage rent clock; a migration to
  // another script must arrive with R5 and R6 at 0 or absent; the
  // whitelist holds at most 64 entries.
  //
  // v2.2 (2026-11 retest round 2, N-1): the successor freshness bound is
  // HEIGHT - 720 (signWindow), was HEIGHT - 10. A signed multisig
  // transaction stays valid for 720 blocks (about 6 hours) after the
  // creation height it was built with. Whitelist entry heights and the
  // migration approval height keep the [HEIGHT - 10, HEIGHT] range
  // (heightSlack).
  //
  // NOT changed in this version: the signer keys are still the public
  // test keys whose secrets are in the test suites (F-013). They must be
  // replaced before genesis. They are the treasury's keys too (F-011).
  //
  // Constant: lpNftId = id of the one-unit LP fund NFT, fixed when the
  // genesis state is built (placeholder LP_NFT_ID_PLACEHOLDER, substituted
  // at compile time). Genesis box: that NFT at tokens(0), R4 = launch
  // whitelist (at most 64 entries), R5 = 0, R6 = 0, R7 = 32 zero bytes
  // (reserves the room a migration approval needs; inert, because
  // migration execute needs R5 = 2).
  //
  // Storage rent (v2.1). Anyone may spend a box idle for 1,051,200 blocks,
  // and if its value is at most fee-per-byte times its size the spender
  // takes it whole, tokens included, with no script check. The NFT would
  // then move to the spender and the accumulation stream with it. So:
  //   - fundFloor = 100 YOLO. No path at this script lets O0 drop below
  //     min(SELF.value, fundFloor); a box already below the floor may keep
  //     its value but not drop.
  //   - Fund the genesis box at or above fundFloor.
  //   - O0's creation height must be >= HEIGHT - 720, so each signed spend
  //     restarts the rent period (at most 720 blocks early).
  //   - Never put the LP NFT id in a rent builder token whitelist.
  //
  // Deposits: value sent straight to this script's address without the
  // NFT can never be spent (selfIsFund is false). Deposits go to the LP
  // accumulation address.
  //
  // Registers (all typed, on SELF and on every successor at this script):
  //   R4: Coll[(Coll[Byte], Long)] whitelist. Each entry is (blake2b256
  //       of an allowed destination's propositionBytes, height added).
  //       Genesis entries may carry any height; an entry with height 0
  //       is usable from block 8,641.
  //   R5: Long state flag: 0 normal, 1 frozen, 2 migration approved
  //   R6: Long migration approval height (0 = none)
  //   R7: Coll[Byte] migration target: blake2b256 of the target script's
  //       propositionBytes. Written only by migration approval.
  //
  // Timing: delay D = 8,640 blocks (72 h at 30 s), window W = 20,160
  // blocks (7 days). Recorded heights may be up to 10 blocks below the
  // block that includes them, so a notice can be up to 10 blocks shorter
  // than D.
  //
  // Paths. O0 = OUTPUTS(0). "Kept" means: O0 at this script, holding the
  // NFT at tokens(0) with amount 1, value >= SELF.value. On paths 1 to 7
  // and 8a O0 also needs value >= min(SELF.value, fundFloor) and creation
  // height >= HEIGHT - 720. Registers not named are copied from SELF
  // unchanged. Other tokens on O0 and outputs other than those named are
  // not checked.
  //   1. Spend: R5 != 1. O0 at this script with the NFT, value at least
  //      the floor, registers unchanged. Every output after O0 is either
  //      (a) at a script whose hash matches a whitelist entry (h, a) with
  //      HEIGHT > a + D, or (b) at the miner fee script with value <=
  //      10,000,000; at most one output of kind (b).
  //   2. Whitelist update: R5 != 1. O0 kept. New R4 = old R4 followed by
  //      one or more new entries, each with a 32-byte hash and a height
  //      in [HEIGHT - 10, HEIGHT] and > 0, at most 64 entries in all.
  //      Entries cannot be removed or changed.
  //   3. Consolidation (also how accumulation boxes are forwarded in):
  //      R5 != 1. O0 kept, all registers unchanged.
  //   4. Freeze: R5 != 1. O0 kept, R5 = 1.
  //   5. Unfreeze: R5 = 1. O0 kept, R5 = 0.
  //   6. Migration approval: R5 = 0. O0 kept, R5 = 2, R6 in [HEIGHT - 10,
  //      HEIGHT] and > 0, R7 = 32-byte hash of the target script.
  //   7. Migration cancel: R5 = 2. O0 kept, R5 = 0, R6 = 0.
  //   8. Migration execute: R5 = 2, R6 > 0, HEIGHT in [R6 + D + 1,
  //      R6 + D + W]. O0 holds the NFT at tokens(0),
  //      blake2b256(O0.propositionBytes) = SELF.R7, O0.value >=
  //      SELF.value - 1,000,000. If the target is this same script, O0
  //      must arrive with R5 = 0, R6 = 0 and R4, R7 unchanged (and the
  //      floor). For any other target O0 must arrive with R5 and R6 each
  //      0 or absent (read as Long, after the checks above); no other
  //      register is read. So a target must not use R5 or R6 as a
  //      non-Long type (the node throws on the read), and a target of
  //      another register design must be reviewed for the state it
  //      arrives in. The contract a fund migrates to needs a deposit path,
  //      or later emission stays at the accumulation script (F-008).
  //
  // Required output layout: O0 on every path. Outputs after O0 are
  // checked only on spend (each must be a matured whitelisted
  // destination or the one fee output). The other paths do not read
  // them, so consolidation, freeze and the rest accept extra outputs; a
  // forward of accumulation boxes needs them for change and fee. A
  // one-output transaction is accepted on every path.
  // ================================================================

  // ---- Signers (TEST KEYS, see F-013 above; same as treasury) ----
  val signers = Coll(
    PK("9gzkoMXatUr5s7jMBvjR7hzJyPqAyvZtdYxHZ9giJUEyvZ9nJde"),
    PK("9fZM68VSqtjH3HibZngnQ9sgZXudKBt146mkKHWuur3qV4DKDYk"),
    PK("9eYPis3GAjApr8RQKADhk4ZDaNQC7cM46pJi91jzJDa2RoymEzh")
  )
  val threshold = 2

  // ---- Constants ----
  val lpNftId: Coll[Byte] = fromBase16("LP_NFT_ID_PLACEHOLDER")
  val timelockBlocks: Long = 8640L   // 72 h at 30 s blocks
  val windowBlocks: Long   = 20160L  // 7 days at 30 s blocks
  val heightSlack: Long    = 10L     // recorded height range (F-016)
  val signWindow: Int      = 720     // successor freshness, about 6 h (v2.2)
  val maxWhitelist: Int    = 64      // whitelist entry cap (v2.1)
  val fundFloor: Long      = 100000000000L // 100 YOLO (storage rent, v2.1)
  val migrationFeeCap: Long = 1000000L
  val spendFeeCap: Long    = 10000000L
  // ErgoTreePredef.feeProposition(720), the standard miner fee script
  // (YOLO keeps Ergo's fee tree).
  val minerFeeScript: Coll[Byte] = fromBase16("1005040004000e36100204a00b08cd0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798ea02d192a39a8cc7a701730073011001020402d19683030193a38cc7b2a57300000193c2b2a57301007473027303830108cdeeac93b1a57304")

  val h: Long = HEIGHT.toLong

  // ---- SELF: must be the fund box (F-014, F-023) ----
  val selfIsFund: Boolean =
    SELF.tokens.size > 0 && SELF.tokens(0) == (lpNftId, 1L)

  val whitelist: Coll[(Coll[Byte], Long)] = SELF.R4[Coll[(Coll[Byte], Long)]].get
  val stateFlag: Long               = SELF.R5[Long].get
  val migrationHeight: Long         = SELF.R6[Long].get
  val migrationTarget: Coll[Byte]   = SELF.R7[Coll[Byte]].get

  // ---- First output: no register reads here ----
  val out0 = OUTPUTS(0)
  val nftToOut0: Boolean =
    out0.tokens.size > 0 && out0.tokens(0) == (lpNftId, 1L)
  val toThisScript: Boolean = out0.propositionBytes == SELF.propositionBytes

  // Migration execute checks that do not read O0's registers.
  val migrationReady: Boolean =
    stateFlag == 2L && migrationHeight > 0L &&
    h > migrationHeight + timelockBlocks &&
    h <= migrationHeight + timelockBlocks + windowBlocks &&
    blake2b256(out0.propositionBytes) == migrationTarget &&
    nftToOut0 && out0.value >= SELF.value - migrationFeeCap

  // ---- Paths whose successor is at this script ----
  // O0's registers are read only inside this branch (F-024).
  val samePaths: Boolean = if (toThisScript) {
    val nWhitelist: Coll[(Coll[Byte], Long)] = out0.R4[Coll[(Coll[Byte], Long)]].get
    val nFlag: Long         = out0.R5[Long].get
    val nMigHeight: Long    = out0.R6[Long].get
    val nTarget: Coll[Byte] = out0.R7[Coll[Byte]].get

    val kept: Boolean = nftToOut0 && out0.value >= SELF.value
    // Every path in this branch (v2.1): value floor and fresh creation
    // height, so the box cannot be left for storage rent to take.
    val floorKept: Boolean =
      out0.value >= (if (SELF.value < fundFloor) SELF.value else fundFloor)
    val freshOut: Boolean = out0.creationInfo._1 >= HEIGHT - signWindow
    val sameWhitelist: Boolean = nWhitelist == whitelist
    val sameMigration: Boolean =
      nMigHeight == migrationHeight && nTarget == migrationTarget

    // 1. Spend
    val spendOuts: Coll[Box] = OUTPUTS.slice(1, OUTPUTS.size)
    val isFeeOut = { (o: Box) =>
      o.propositionBytes == minerFeeScript && o.value <= spendFeeCap
    }
    val isMatured = { (o: Box) =>
      val dest: Coll[Byte] = blake2b256(o.propositionBytes)
      whitelist.exists { (e: (Coll[Byte], Long)) =>
        e._1 == dest && h > e._2 + timelockBlocks
      }
    }
    val isSpend: Boolean =
      stateFlag != 1L && nftToOut0 && sameWhitelist && nFlag == stateFlag &&
      sameMigration &&
      spendOuts.filter(isFeeOut).size <= 1 &&
      spendOuts.forall { (o: Box) => isFeeOut(o) || isMatured(o) }

    // 2. Whitelist update (append only, new entries stamped now)
    val isWhitelistUpdate: Boolean =
      stateFlag != 1L && kept && nFlag == stateFlag && sameMigration &&
      nWhitelist.size > whitelist.size && nWhitelist.size <= maxWhitelist &&
      nWhitelist.slice(0, whitelist.size) == whitelist &&
      nWhitelist.slice(whitelist.size, nWhitelist.size).forall {
        (e: (Coll[Byte], Long)) =>
          e._1.size == 32 && e._2 >= h - heightSlack && e._2 <= h && e._2 > 0L
      }

    // 3. Consolidation
    val isConsolidation: Boolean =
      stateFlag != 1L && kept && sameWhitelist && nFlag == stateFlag &&
      sameMigration

    // 4. Freeze
    val isFreeze: Boolean =
      stateFlag != 1L && kept && sameWhitelist && nFlag == 1L && sameMigration

    // 5. Unfreeze
    val isUnfreeze: Boolean =
      stateFlag == 1L && kept && sameWhitelist && nFlag == 0L && sameMigration

    // 6. Migration approval
    val isMigrationApproval: Boolean =
      stateFlag == 0L && kept && sameWhitelist && nFlag == 2L &&
      nMigHeight >= h - heightSlack && nMigHeight <= h && nMigHeight > 0L &&
      nTarget.size == 32

    // 7. Migration cancel
    val isMigrationCancel: Boolean =
      stateFlag == 2L && kept && sameWhitelist && nFlag == 0L &&
      nMigHeight == 0L && nTarget == migrationTarget

    // 8a. Migration execute to this same script: arrive neutral.
    val isMigrateToSelf: Boolean =
      migrationReady && sameWhitelist && nFlag == 0L && nMigHeight == 0L &&
      nTarget == migrationTarget

    floorKept && freshOut && (
      isSpend || isWhitelistUpdate || isConsolidation || isFreeze ||
      isUnfreeze || isMigrationApproval || isMigrationCancel || isMigrateToSelf)
  } else {
    // 8b. Migration execute to another script. The target must arrive
    // with nothing approved: R5 and R6 each 0 or absent. Read only here,
    // after the readiness checks.
    migrationReady &&
    out0.R5[Long].getOrElse(0L) == 0L && out0.R6[Long].getOrElse(0L) == 0L
  }

  // ================================================================
  // ENTRY GUARD
  // ================================================================
  atLeast(threshold, signers) && sigmaProp(selfIsFund && samePaths)
}
