{
  // ================================================================
  // TREASURY GOVERNANCE CONTRACT v2.2
  // ================================================================
  // Governs the 4% treasury share of emission. Every path needs 2 of
  // the 3 signers below.
  //
  // v2.0 (2026-11 review fixes): NFT id is a constant and SELF must hold
  // it (F-014, F-023); cancellation clears the migration flag (F-004);
  // payment execution needs the normal state and a matching proposal
  // hash (F-018); approvals record a height in [HEIGHT - 10, HEIGHT]
  // (F-016); execution and migration execute must happen inside a
  // window after the delay (F-007); migration approval records the
  // target script hash, and execute checks it and a value floor (F-006);
  // execution slack goes only to a miner fee output (F-021); output
  // registers are read only on paths whose successor is at this script,
  // and only execution reads a second output (F-024); freeze can be
  // undone by an unfreeze path (F-012).
  //
  // v2.1 (2026-11 retest): every path whose O0 stays at this script keeps
  // O0.value >= min(SELF.value, fundFloor) and O0's creation height at
  // least HEIGHT - 10, so the fund box cannot be drained below the floor
  // and each signed spend restarts its storage rent clock; a migration to
  // another script must arrive with R5 and R8 at 0 or absent; the
  // execution fee cap is 10,000,000 (was 1,000,000; migration slack stays
  // 1,000,000).
  //
  // v2.2 (2026-11 retest round 2, N-1): the successor freshness bound is
  // HEIGHT - 720 (signWindow), was HEIGHT - 10. A signed multisig
  // transaction stays valid for 720 blocks (about 6 hours) after the
  // creation height it was built with. Approval heights keep the
  // [HEIGHT - 10, HEIGHT] range (heightSlack).
  //
  // NOT changed in this version: the signer keys are still the public
  // test keys whose secrets are in the test suites (F-013). They must be
  // replaced before genesis. Both funds still share one key set (F-011).
  //
  // Constant: treasuryNftId = id of the one-unit treasury NFT, fixed when
  // the genesis state is built (placeholder TREASURY_NFT_ID_PLACEHOLDER,
  // substituted at compile time). Genesis box: that NFT at tokens(0),
  // R4 = any 32 bytes, R5 = 0, R6 = empty, R7 = 0, R8 = 0, all of the
  // types listed below (Long, not Int).
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
  //   - Never put the treasury NFT id in a rent builder token whitelist.
  //
  // Deposits: value sent straight to this script's address without the
  // NFT can never be spent (selfIsFund is false). Deposits go to the
  // treasury accumulation address.
  //
  // Registers (all typed, on SELF and on every successor at this script):
  //   R4: Coll[Byte] commitment. Payment proposal: blake2b256(R6 ++
  //       longToByteArray(R7)). Migration approved: blake2b256 of the
  //       target script's propositionBytes. Ignored when R5 = 0.
  //   R5: Long approval height (0 = nothing approved)
  //   R6: Coll[Byte] payment recipient propositionBytes (empty while a
  //       migration is approved)
  //   R7: Long payment amount in nanocoins (0 while a migration is
  //       approved)
  //   R8: Long state flag: 0 normal, 1 frozen, 2 migration approved
  //
  // Timing: delay D = 8,640 blocks (72 h at 30 s), window W = 20,160
  // blocks (7 days). An approval recorded at height a can be executed at
  // heights a + D + 1 through a + D + W. After that it can only be
  // cancelled. The recorded height may be up to 10 blocks below the
  // block that includes the approval, so the notice can be up to 10
  // blocks shorter than D.
  //
  // Paths. O0 = OUTPUTS(0). "Kept" means: O0 at this script, holding the
  // NFT at tokens(0) with amount 1, value >= SELF.value. On paths 1 to 7
  // and 8a O0 also needs value >= min(SELF.value, fundFloor) and creation
  // height >= HEIGHT - 720. Registers not named are copied from SELF
  // unchanged. Other tokens on O0 and outputs other than those named are
  // not checked.
  //   1. Approval: R8 = 0, R5 = 0. O0 kept, R8 = 0, R5 in [HEIGHT - 10,
  //      HEIGHT] and > 0, R6 and R7 = the new recipient and amount (any
  //      values), R4 = blake2b256(R6 ++ longToByteArray(R7)) of O0's own R6
  //      and R7.
  //   2. Execution: R8 = 0, R5 > 0, SELF.R4 matches SELF.R6 and SELF.R7,
  //      inside the window. OUTPUTS(1) at SELF.R6 with value >= SELF.R7.
  //      Outputs after OUTPUTS(1): at most one, at the miner fee script,
  //      value <= 10,000,000. O0 at this script with the NFT, value >=
  //      SELF.value - SELF.R7 - (that fee output's value) and >= the
  //      floor, R5 = 0, R8 = 0.
  //      Layout: [O0, payment] or [O0, payment, fee].
  //   3. Cancellation: R5 > 0. O0 kept, R5 = 0, R8 = 0 if SELF.R8 was 2,
  //      otherwise R8 unchanged.
  //   4. Consolidation (also how accumulation boxes are forwarded in):
  //      R8 != 1. O0 kept, all registers unchanged.
  //   5. Freeze: R8 != 1. O0 kept, R8 = 1.
  //   6. Unfreeze: R8 = 1. O0 kept, R8 = 0. A payment approval made
  //      before the freeze is still there and still bound by its window.
  //      A migration approval is not: after unfreeze the state is 0 with
  //      R5 > 0, so it cannot be executed, only cancelled and re-approved.
  //   7. Migration approval: R8 = 0, R5 = 0. O0 kept, R8 = 2, R5 in
  //      [HEIGHT - 10, HEIGHT] and > 0, R4 = 32-byte hash of the target
  //      script, R6 = empty, R7 = 0.
  //   8. Migration execute: R8 = 2, R5 > 0, inside the window. O0 holds
  //      the NFT at tokens(0), blake2b256(O0.propositionBytes) = SELF.R4,
  //      O0.value >= SELF.value - 1,000,000. If the target is this same
  //      script, O0 must arrive with R5 = 0 and R8 = 0 and R4, R6, R7
  //      unchanged (and the floor). For any other target O0 must arrive
  //      with R5 and R8 each 0 or absent (read as Long, after the checks
  //      above); no other register is read. So a target must not use R5
  //      or R8 as a non-Long type (the node throws on the read), and a
  //      target of another register design must be reviewed for the state
  //      it arrives in. The contract a fund migrates to needs a deposit
  //      path, or later emission stays at the accumulation script (F-008).
  //
  // Required output layout: O0 on every path; OUTPUTS(1) only on
  // execution. A one-output transaction is accepted on every other path.
  // ================================================================

  // ---- Signers (TEST KEYS, see F-013 above) ----
  val signers = Coll(
    PK("9gzkoMXatUr5s7jMBvjR7hzJyPqAyvZtdYxHZ9giJUEyvZ9nJde"),
    PK("9fZM68VSqtjH3HibZngnQ9sgZXudKBt146mkKHWuur3qV4DKDYk"),
    PK("9eYPis3GAjApr8RQKADhk4ZDaNQC7cM46pJi91jzJDa2RoymEzh")
  )
  val threshold = 2

  // ---- Constants ----
  val treasuryNftId: Coll[Byte] = fromBase16("TREASURY_NFT_ID_PLACEHOLDER")
  val timelockBlocks: Long = 8640L   // 72 h at 30 s blocks
  val windowBlocks: Long   = 20160L  // 7 days at 30 s blocks
  val heightSlack: Long    = 10L     // approval height range (F-016)
  val signWindow: Int      = 720     // successor freshness, about 6 h (v2.2)
  val feeCap: Long         = 10000000L // execution fee output cap
  val migrationFeeCap: Long = 1000000L // migration value slack
  val fundFloor: Long      = 100000000000L // 100 YOLO (storage rent, v2.1)
  // ErgoTreePredef.feeProposition(720), the standard miner fee script
  // (YOLO keeps Ergo's fee tree).
  val minerFeeScript: Coll[Byte] = fromBase16("1005040004000e36100204a00b08cd0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798ea02d192a39a8cc7a701730073011001020402d19683030193a38cc7b2a57300000193c2b2a57301007473027303830108cdeeac93b1a57304")

  val h: Long = HEIGHT.toLong

  // ---- SELF: must be the fund box (F-014, F-023) ----
  val selfIsFund: Boolean =
    SELF.tokens.size > 0 && SELF.tokens(0) == (treasuryNftId, 1L)

  val commitment: Coll[Byte] = SELF.R4[Coll[Byte]].get
  val approvalHeight: Long   = SELF.R5[Long].get
  val recipient: Coll[Byte]  = SELF.R6[Coll[Byte]].get
  val amount: Long           = SELF.R7[Long].get
  val stateFlag: Long        = SELF.R8[Long].get

  val hasActiveProposal: Boolean = approvalHeight > 0L
  val inWindow: Boolean =
    h > approvalHeight + timelockBlocks &&
    h <= approvalHeight + timelockBlocks + windowBlocks

  // ---- First output: no register reads here ----
  val out0 = OUTPUTS(0)
  val nftToOut0: Boolean =
    out0.tokens.size > 0 && out0.tokens(0) == (treasuryNftId, 1L)
  val toThisScript: Boolean = out0.propositionBytes == SELF.propositionBytes

  // Migration execute checks that do not read O0's registers.
  val migrationReady: Boolean =
    stateFlag == 2L && hasActiveProposal && inWindow &&
    blake2b256(out0.propositionBytes) == commitment &&
    nftToOut0 && out0.value >= SELF.value - migrationFeeCap

  // ---- Paths whose successor is at this script ----
  // O0's registers are read only inside this branch (F-024).
  val samePaths: Boolean = if (toThisScript) {
    val nCommit: Coll[Byte]    = out0.R4[Coll[Byte]].get
    val nHeight: Long          = out0.R5[Long].get
    val nRecipient: Coll[Byte] = out0.R6[Coll[Byte]].get
    val nAmount: Long          = out0.R7[Long].get
    val nFlag: Long            = out0.R8[Long].get

    val kept: Boolean = nftToOut0 && out0.value >= SELF.value
    // Every path in this branch (v2.1): value floor and fresh creation
    // height, so the box cannot be left for storage rent to take.
    val floorKept: Boolean =
      out0.value >= (if (SELF.value < fundFloor) SELF.value else fundFloor)
    val freshOut: Boolean = out0.creationInfo._1 >= HEIGHT - signWindow
    val sameProposal: Boolean =
      nCommit == commitment && nRecipient == recipient && nAmount == amount
    val heightRecorded: Boolean =
      nHeight >= h - heightSlack && nHeight <= h && nHeight > 0L

    // 1. Approval
    val isApproval: Boolean =
      stateFlag == 0L && !hasActiveProposal && kept && heightRecorded &&
      nFlag == 0L &&
      nCommit == blake2b256(nRecipient ++ longToByteArray(nAmount))

    // 2. Execution. Payment and fee outputs are read through slices,
    // which never fail on a short OUTPUTS.
    val feeOuts: Coll[Box] = OUTPUTS.slice(2, OUTPUTS.size)
    val feeTaken: Long = feeOuts.fold(0L, { (a: Long, o: Box) => a + o.value })
    val isExecution: Boolean =
      stateFlag == 0L && hasActiveProposal && inWindow &&
      commitment == blake2b256(recipient ++ longToByteArray(amount)) &&
      OUTPUTS.slice(1, 2).exists { (p: Box) =>
        p.propositionBytes == recipient && p.value >= amount
      } &&
      feeOuts.size <= 1 &&
      feeOuts.forall { (o: Box) =>
        o.propositionBytes == minerFeeScript && o.value <= feeCap
      } &&
      nftToOut0 && out0.value >= SELF.value - amount - feeTaken &&
      nHeight == 0L && nFlag == 0L && sameProposal

    // 3. Cancellation
    val isCancellation: Boolean =
      hasActiveProposal && kept && nHeight == 0L &&
      nFlag == (if (stateFlag == 2L) 0L else stateFlag) && sameProposal

    // 4. Consolidation
    val isConsolidation: Boolean =
      stateFlag != 1L && kept && nHeight == approvalHeight &&
      nFlag == stateFlag && sameProposal

    // 5. Freeze
    val isFreeze: Boolean =
      stateFlag != 1L && kept && nHeight == approvalHeight &&
      nFlag == 1L && sameProposal

    // 6. Unfreeze
    val isUnfreeze: Boolean =
      stateFlag == 1L && kept && nHeight == approvalHeight &&
      nFlag == 0L && sameProposal

    // 7. Migration approval
    val isMigrationApproval: Boolean =
      stateFlag == 0L && !hasActiveProposal && kept && heightRecorded &&
      nFlag == 2L && nCommit.size == 32 && nRecipient.size == 0 &&
      nAmount == 0L

    // 8a. Migration execute to this same script: arrive neutral.
    val isMigrateToSelf: Boolean =
      migrationReady && nHeight == 0L && nFlag == 0L && sameProposal

    floorKept && freshOut && (
      isApproval || isExecution || isCancellation || isConsolidation ||
      isFreeze || isUnfreeze || isMigrationApproval || isMigrateToSelf)
  } else {
    // 8b. Migration execute to another script. The target must arrive
    // with nothing approved: R5 and R8 each 0 or absent. Read only here,
    // after the readiness checks.
    migrationReady &&
    out0.R5[Long].getOrElse(0L) == 0L && out0.R8[Long].getOrElse(0L) == 0L
  }

  // ================================================================
  // ENTRY GUARD
  // ================================================================
  atLeast(threshold, signers) && sigmaProp(selfIsFund && samePaths)
}
