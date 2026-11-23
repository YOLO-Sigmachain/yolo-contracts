{
  // ================================================================
  // TREASURY ACCUMULATION CONTRACT v2.2
  // ================================================================
  // Holds the 4% treasury share of each block until it is forwarded to
  // the box that holds the treasury NFT. No signature is needed on this
  // script; what it allows is fixed by the two paths below.
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
  // Constant:
  //   treasuryNftId = id of the one-unit treasury NFT, fixed when the
  //   genesis state is built (placeholder TREASURY_NFT_ID_PLACEHOLDER,
  //   substituted at compile time). The stream follows the NFT, so a
  //   migrated treasury keeps receiving it if its contract has a
  //   deposit path.
  //
  // accSum = sum of the values of all inputs at this script.
  //
  // PATH 1, CONSOLIDATION (unsigned, for a keeper):
  //   [accumulation boxes, fee input] -> [O0, change, fee]
  //   O0: this script; value >= accSum; no tokens; no registers;
  //       creation height in [HEIGHT - 10, HEIGHT].
  //   SELF: no tokens, and created more than 20,160 blocks (7 days) ago
  //   (F-025). Because O0 may be created up to 10 blocks in the past, a
  //   merged box can be merged again after 20,150 blocks: the effective
  //   cooldown is the constant minus 10.
  //   The fee and any change come from a separate input.
  //
  // PATH 2, TRANSFER (forward into the treasury NFT holder):
  //   [NFT holder, accumulation boxes, fee input] -> [O0, change, fee]
  //   INPUTS(0) and O0 both hold the treasury NFT at tokens(0) (amount 1).
  //   INPUTS(0) is not at this script.
  //   O0.value >= the sum of the values of all inputs at this script
  //   or at INPUTS(0)'s script (INPUTS(0) included).
  //   INPUTS(0)'s own script decides everything else; on the governance
  //   script an honest forward uses its consolidation path (2 of 3).
  //
  // Required layout: O0 as above; other outputs are not read. Reads that
  // can fail sit inside the branch that needs them.
  // ================================================================

  val treasuryNftId: Coll[Byte] = fromBase16("TREASURY_NFT_ID_PLACEHOLDER")
  val cooldownBlocks: Int = 20160

  val accSum: Long = INPUTS.filter { (b: Box) =>
    b.propositionBytes == SELF.propositionBytes
  }.fold(0L, { (a: Long, b: Box) => a + b.value })

  val out0 = OUTPUTS(0)

  // Holder test: the NFT at tokens(0) with amount 1. The size test comes
  // first, so a box without tokens gives false, not an error.
  val holdsNft = { (b: Box) =>
    b.tokens.size > 0 && b.tokens(0) == (treasuryNftId, 1L)
  }

  // ---- PATH 1: CONSOLIDATION ----
  val isConsolidation: Boolean =
    out0.propositionBytes == SELF.propositionBytes &&
    out0.value >= accSum &&
    SELF.tokens.size == 0 &&
    out0.tokens.size == 0 &&
    HEIGHT > SELF.creationInfo._1 + cooldownBlocks &&
    out0.creationInfo._1 >= HEIGHT - 10 &&
    out0.creationInfo._1 <= HEIGHT &&
    out0.R4[Coll[Byte]].isEmpty

  // ---- PATH 2: TRANSFER TO THE TREASURY NFT HOLDER ----
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
