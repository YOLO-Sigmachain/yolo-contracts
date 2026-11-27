{
  // ================================================================
  // LP FUND ACCUMULATION CONTRACT v2.3
  // ================================================================
  // Holds the 3% LP share of each block until it is forwarded to the box
  // that holds the LP fund NFT. No signature is needed on this script;
  // what it allows is fixed by the two paths below.
  //
  // v2.0 (2026-11 review fixes F-001, F-002, F-003, F-008, F-019, F-023,
  // F-025): the destination is the holder of a constant NFT, not a script
  // hash, and both paths conserve the sum of every input at this script.
  //
  // v2.1 (2026-11 retest): the holder must carry the NFT at tokens(0), not
  // at any index; the holder (INPUTS(0)) must not itself be at this script,
  // so a box here holding the NFT is never a holder; the merge cooldown is
  // 20,160 blocks (was 720). A fund whose NFT is sent to this script (for
  // example by a signer-approved migration) is then locked permanently,
  // and so is every later forward. Signers must never approve either
  // accumulation script, or the emission script, as a migration target.
  //
  // v2.2 (2026-11 retest round 2, EA-N1): the transfer value bound counts
  // every input at this script or at the holder's own script, so a holder
  // script that also sums its own-script inputs (as the DAO treasury.es
  // MERGE path does) cannot be paired with this path to skim the smaller
  // of the two sums.
  //
  // v2.3 (2026-11 EKB audit, storage rent): the consolidation path may pay
  // a merge fee out of the merged value, so that a merge can pay its own
  // transaction fee and boxes are not left to pay storage rent after a
  // year (two boxes yield 0.0005 YOLO, half the node's default minimum
  // fee of 0.001; three or more cover it). The
  // fee is at most mergeFeePerBox for each box merged beyond the first,
  // counting only boxes of at least feeEligibleMin, and whatever O0 is
  // short of accSum must be in outputs at the miner fee script. Each share
  // box can be merged away once, so the fee taken over the life of the
  // stream is at most mergeFeePerBox per share box. A box below
  // feeEligibleMin earns no fee, so sending dust here to raise the count
  // gains nothing. A merge of a single box pays no fee, as before.
  //
  // Constant:
  //   lpNftId = id of the one-unit LP fund NFT, fixed when the genesis
  //   state is built (placeholder LP_NFT_ID_PLACEHOLDER, substituted at
  //   compile time). It differs from the treasury NFT, so this script
  //   and the treasury accumulation script compile to different trees.
  //   The stream follows the NFT, so a migrated LP fund keeps receiving
  //   it if its contract has a deposit path.
  //
  // accSum = sum of the values of all inputs at this script.
  //
  // PATH 1, CONSOLIDATION (unsigned, for a keeper):
  //   [accumulation boxes, optional fee input] -> [O0, fee, optional change]
  //   O0: this script; value >= accSum - mergeFee; no tokens; no registers;
  //       creation height in [HEIGHT - 10, HEIGHT].
  //   mergeFee = mergeFeePerBox * (eligible boxes - 1), 0 if fewer than two;
  //   an eligible box is an input at this script of at least feeEligibleMin.
  //   Outputs at the miner fee script hold at least accSum - O0.value.
  //   SELF: no tokens, and created more than 20,160 blocks (7 days) ago
  //   (F-025). Because O0 may be created up to 10 blocks in the past, a
  //   merged box can be merged again after 20,150 blocks: the effective
  //   cooldown is the constant minus 10.
  //   A fee above mergeFee, and any change, come from a separate input.
  //
  // PATH 2, TRANSFER (forward into the LP fund NFT holder):
  //   [NFT holder, accumulation boxes, fee input] -> [O0, change, fee]
  //   INPUTS(0) and O0 both hold the LP fund NFT at tokens(0) (amount 1).
  //   INPUTS(0) is not at this script.
  //   O0.value >= the sum of the values of all inputs at this script
  //   or at INPUTS(0)'s script (INPUTS(0) included).
  //   INPUTS(0)'s own script decides everything else; on the LP fund
  //   script an honest forward uses its consolidation path (2 of 3).
  //
  // Required layout: O0 as above; the consolidation path also sums the
  // outputs at the miner fee script, and reads no other output. Reads that
  // can fail sit inside the branch that needs them.
  // ================================================================

  val lpNftId: Coll[Byte] = fromBase16("LP_NFT_ID_PLACEHOLDER")
  val cooldownBlocks: Int = 20160
  // 0.0005 YOLO for each box merged beyond the first.
  val mergeFeePerBox: Long = 500000L
  // 0.025 YOLO. On the planned schedule no share the emission contract
  // pays is below 0.03 YOLO, so every real share box counts.
  val feeEligibleMin: Long = 25000000L
  // YOLO's standard miner fee script, the same bytes the two fund
  // contracts use.
  val minerFeeScript: Coll[Byte] = fromBase16("1005040004000e36100204a00b08cd0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798ea02d192a39a8cc7a701730073011001020402d19683030193a38cc7b2a57300000193c2b2a57301007473027303830108cdeeac93b1a57304")

  val out0 = OUTPUTS(0)

  // Holder test: the NFT at tokens(0) with amount 1. The size test comes
  // first, so a box without tokens gives false, not an error.
  val holdsNft = { (b: Box) =>
    b.tokens.size > 0 && b.tokens(0) == (lpNftId, 1L)
  }

  // ---- PATH 1: CONSOLIDATION ----
  // Everything this path needs is computed inside it, behind the test that
  // O0 is at this script, so a forward (path 2) does not pay for it.
  // Whatever O0 holds below accSum must be matched by outputs at the miner
  // fee script. The test is on the amount: a keeper who pays the fee from
  // an own input may take the shortfall as change, which leaves the keeper
  // no better off than paying nothing. Only the miner of the block gains,
  // by at most mergeFee.
  val isConsolidation: Boolean = if (out0.propositionBytes == SELF.propositionBytes) {
    val accBoxes = INPUTS.filter { (b: Box) =>
      b.propositionBytes == SELF.propositionBytes
    }
    val accSum: Long = accBoxes.fold(0L, { (a: Long, b: Box) => a + b.value })
    val eligible = accBoxes.filter { (b: Box) => b.value >= feeEligibleMin }.size
    val mergeFee: Long = if (eligible > 1) (eligible - 1).toLong * mergeFeePerBox else 0L
    val feePaid: Long = OUTPUTS.fold(0L, { (a: Long, o: Box) =>
      if (o.propositionBytes == minerFeeScript) a + o.value else a
    })
    out0.value >= accSum - mergeFee &&
    feePaid >= accSum - out0.value &&
    SELF.tokens.size == 0 &&
    out0.tokens.size == 0 &&
    HEIGHT > SELF.creationInfo._1 + cooldownBlocks &&
    out0.creationInfo._1 >= HEIGHT - 10 &&
    out0.creationInfo._1 <= HEIGHT &&
    out0.R4[Coll[Byte]].isEmpty
  } else { false }

  // ---- PATH 2: TRANSFER TO THE LP FUND NFT HOLDER ----
  val isTransfer: Boolean =
    holdsNft(INPUTS(0)) &&
    INPUTS(0).propositionBytes != SELF.propositionBytes &&
    holdsNft(out0) &&
    out0.value >= INPUTS.fold(0L, { (a: Long, b: Box) =>
      if (b.propositionBytes == SELF.propositionBytes ||
          b.propositionBytes == INPUTS(0).propositionBytes) a + b.value else a
    })

  sigmaProp(isConsolidation || isTransfer)
}
