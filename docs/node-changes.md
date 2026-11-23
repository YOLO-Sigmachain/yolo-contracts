# What the YOLO node changes from Ergo

The YOLO SigmaChain node is a fork of the Ergo reference node. It starts from Ergo tag `v6.0.8`, commit `c1a69f65df2228ce26b7d1c2360b472ef79fc55c`. Every change is in the commits after that starting commit. This document lists each change, numbered as in the summary below: what Ergo does, what the YOLO node does, the setting or file that controls it, and whether it needs the YOLO switch.

Paths are relative to the root of the node repository. Line numbers are those of the current tree and can move.

## 1. Summary

"Switch" means the change acts only when `ergo.chain.yolo.enabled = true`. "Plain run" means it also acts on a node started with Ergo's own settings.

| # | Change | Applies to | Main files |
|---|---|---|---|
| 1 | Candidate generator rejects a late solution instead of failing | Plain run | `src/main/scala/org/ergoplatform/mining/CandidateGenerator.scala` |
| 2 | Candidate generator starts when the history holds a single header | Plain run | `CandidateGenerator.scala` |
| 3 | A locally mined block is announced section by section, once each is stored | Plain run | `ErgoNodeViewSynchronizer.scala`, `CandidateGenerator.scala`, `BlocksApiRoute.scala` |
| 4 | Two nodes that dial each other at once keep one connection | Plain run | `src/main/scala/scorex/core/network/NetworkController.scala` |
| 5 | The rent-claim mempool filter also covers transactions from rolled back blocks | Plain run, when `rejectStorageRentTxs = true` | `ErgoMemPool.scala`, `ErgoNodeViewHolder.scala` |
| 6 | Rule 308: the block producer attests storage rent claims in the extension | Plain run from block version 5; switch from genesis | `ErgoState.scala`, `Extension.scala`, `ValidationRules.scala` |
| 7 | The extra index covers the genesis boxes and their tokens | Plain run, when `extraIndex = true` | `ExtraIndexer.scala`, `ErgoHistory.scala` |
| 8 | YOLO switch, launch parameters (storage fee factor, block cost limit), start-up checks | Switch | `YoloSettings.scala`, `ChainSettings.scala`, `LaunchParameters.scala`, `ErgoSettingsReader.scala` |
| 9 | Mining table height is block height / 4 | Setting `powHeightDivisor`, tied to the switch | `AutolykosPowScheme.scala` |
| 10 | 30 second blocks, EIP-37 from genesis over 64-block periods, first-retarget guard, difficulty floor, 2 minute future-timestamp limit | Switch | `HeadersProcessor.scala`, `DifficultyAdjustment.scala`, `ChainSettings.scala` |
| 11 | YOLO emission rules, split 93 / 4 / 3, genesis boxes, launch check, contract pins, refusal of the public test signer keys | Switch | `YoloEmissionRules.scala`, `YoloGenesis.scala`, `YoloTreePins.scala` |
| 12 | YOLO reward transaction, miner output locked 2,880 blocks | Switch | `YoloEmissionTx.scala`, `CandidateGenerator.scala`, wallet files |
| 13 | Rule 308 from genesis, miner self-collection of storage rent turned on | Switch and settings | `ChainSettings.scala`, `yolo-testnet.conf` |
| 14 | `/emission` and `/mining/rewardAddress` report the YOLO values | Switch | `EmissionApiRoute.scala`, `MiningApiRoute.scala` |
| 15 | Testnet and devnet settings files | Settings | `src/main/resources/yolo-testnet.conf`, `src/main/resources/yolo-devnet.conf` |
| 16 | Relay policy: relay limit 1,000,000, capped by the block cost limit; fee outputs counted in the relay cost; fee outputs carrying tokens refused | Switch | `YoloFeeOutputPolicy.scala`, `ErgoMemPool.scala`, `CleanupWorker.scala`, `ErgoBaseApiRoute.scala`, `CandidateGenerator.scala` |
| 17 | Fee collection: only fee boxes whose tokens fit in the miner output, token-free first, nothing burned | Switch | `CandidateGenerator.scala` |
| 18 | Storage rent claim kept within a cost and a size budget; a claim that lost a race keeps its boxes in the rent index | Switch | `CandidateGenerator.scala`, `StorageRentClaimBuilder.scala` |
| 19 | Block assembly: recorded cost and abort limit, transactions that do not fit passed over, refused boxes and the P2PK rule, work budget per candidate, fee transaction built once | Switch | `YoloAssemblyPolicy.scala`, `YoloRefusedBoxes.scala`, `CandidateGenerator.scala`, `ErgoMemPool.scala`, `ErgoNodeViewHolder.scala` |

Changes 16 to 19 are node policy, not consensus (section 13).

## 2. How the switch works

The switch is `ergo.chain.yolo.enabled`.

- It is off in `src/main/resources/application.conf` (line 301), and its default in code is off (`ergo-core/src/main/scala/org/ergoplatform/settings/YoloSettings.scala`, line 32). A node started with Ergo's settings never turns it on.
- It is on in `src/main/resources/yolo-testnet.conf` (line 104). The devnet file includes the testnet file, so it is on there too.
- The node refuses to start with the switch on unless the network type is `testnet` (`src/main/scala/org/ergoplatform/settings/ErgoSettingsReader.scala`, line 255). The YOLO settings file sets `networkType = "testnet"` and is started with `--testnet`.
- No YOLO mainnet settings file exists yet. The repository holds only `yolo-testnet.conf` and `yolo-devnet.conf`.

With the switch on, the node also refuses to start when any of these holds (`ErgoSettingsReader.scala`, lines 255 to 290):

- the address prefix is Ergo's mainnet prefix;
- `ergo.chain.powScheme.powHeightDivisor` is 1. The reverse is refused too: a divisor other than 1 with the switch off;
- `useLastEpochs` is below 2;
- `eip37EpochLength` is not 64;
- the initial difficulty is below the YOLO floor of 2, or cannot be written exactly in nBits;
- EIP-27 re-emission is on (`checkReemissionRules = true`, or `reemission.activationHeight` is not 2147483647).

Then the launch check runs (`YoloGenesis.launchError`, called from `ErgoSettingsReader.scala` lines 303 and 339). It is described in section 9.

Two start-up messages do not stop the node. With `allowTestSignerKeys = true` and a fund tree that holds a public test signer key, the node logs an error at every start (`ErgoSettingsReader.scala`, lines 341 to 344; section 9). When the relay limit is above the room a full block leaves for one pool transaction, it logs a warning (line 346; section 13.1).

Unknown keys inside `ergo.chain.yolo` and `ergo.chain.yolo.emission` stop the node at start (`ErgoSettingsReader.scala`, lines 46 and 208). The settings reader would otherwise read a misspelt key, such as `enable`, as its default. This check runs whenever the `yolo` block is present, and `application.conf` always has one, so it also applies to a plain run.

The PoW height divisor is a separate setting, not part of the switch. The checks above force the two to agree: a node with one set and not the other would reject every block of the other kind.

## 3. General fixes (changes 1 to 5)

These five change behaviour for any node, YOLO or not. Each is kept as its own commit so it can be offered to Ergo.

### 3.1 Late mining solution (change 1)

**Ergo.** When a solution does not fit the current candidate, the node falls back to the previous candidate with `cachedPreviousCandidate.get`. If there is no previous candidate, this throws.

**YOLO node.** The fallback is an `Option`. With no previous candidate the node logs the case and replies with an error: "Solution does not fit the current candidate and there is no previous candidate". The current candidate stays. (`CandidateGenerator.scala`, lines 259 to 302.)

The same block holds one switch-only change. Ergo puts the node's own key into a solution that carries no key. With the switch on, the node instead uses the key that the chosen candidate was built for, because the YOLO coinbase pays that key (lines 261 to 272).

### 3.2 Start with a single header (change 2)

**Ergo.** At start the candidate generator averages the gaps between recent header timestamps. With exactly one header, the pairing step gets a group of one and fails on a pattern match.

**YOLO node.** Groups of one are skipped. With fewer than two timestamps the average is zero. (`CandidateGenerator.scala`, lines 444 to 454; called at line 114.)

### 3.3 Announce a mined block when its sections are stored (change 3)

**Ergo.** As soon as a block is mined, by the internal miner or through the API, the node sends an inventory message for the header and every section to all peers. This happens before anything is stored. A nearby peer can ask for a section the node cannot serve yet, then wait for the delivery timeout.

**YOLO node.** The header and section ids go into a list (`minedSectionsToAnnounce`). Each is announced when it is stored in history, which is still before the block is applied to state. (`src/main/scala/org/ergoplatform/network/ErgoNodeViewSynchronizer.scala`, lines 176, 1516 to 1532 and 1593 to 1597.) The API route now publishes the event before it sends the block to the node view, so no stored section is missed (`src/main/scala/org/ergoplatform/http/api/BlocksApiRoute.scala`, line 133).

### 3.4 Two nodes that dial each other at once (change 4)

**Ergo.** When a handshake arrives for a peer that already has a connection, the new connection is closed and the peer is removed from the peer database. If two nodes dial each other at the same moment, each side can close a different one of the two connections, so both can be lost.

**YOLO node.** On a mutual dial, both sides keep the connection opened by the node with the lower session id (`NetworkController.keepNewOfMutualDial`, line 630). The existing connection is replaced only when both connections come from the same IP address and, if the existing one has finished its handshake, carry the same session id. This stops a third party that claims a peer's address from closing the live connection. A plain duplicate is closed, but the peer stays in the peer database. (`NetworkController.scala`, lines 447 to 495.)

### 3.5 Rent-claim filter for rolled back transactions (change 5)

**Ergo.** The setting `ergo.node.rejectStorageRentTxs` makes the mempool decline storage rent claim transactions. Transactions from rolled back blocks go back into the pool without that check.

**YOLO node.** They go back through `putRolledBack`, which applies the same filter when the setting is on (`src/main/scala/org/ergoplatform/nodeView/mempool/ErgoMemPool.scala`, line 124; called from `ErgoNodeViewHolder.scala`, line 438). With the setting off, as in Ergo's defaults (`application.conf`, line 124), nothing changes.

## 4. Storage rent attestation, rule 308 (changes 6 and 13)

Ergo v6.0.8 has storage rent and a miner-side builder for rent claims (`src/main/scala/org/ergoplatform/mining/StorageRentClaimBuilder.scala`), but no rule that ties rent claims to the block extension. The fork adds validation rule 308, `bsStorageRentAttestation`, as named in EIP-0052.

**What counts as a rent claim.** An input is a rent claim when the box is at least `StoragePeriod` blocks old, the spending proof is empty, and context variable 127 is a `Short` that points at an existing output (`ergo-core/src/main/scala/org/ergoplatform/modifiers/mempool/ErgoTransaction.scala`, line 549). These are the conditions under which the interpreter runs the rent check.

**The rule.** Let C be the ids, in block order, of the block's transactions that have at least one rent-claim input. If C is not empty, the block extension must hold exactly one field with key `0x03 0x00`, and its value must be the Blake2b256 hash of the ids in C joined in order. If C is empty, the extension must hold no field with that key. (`ergo-core/src/main/scala/org/ergoplatform/modifiers/history/extension/Extension.scala`, lines 86 to 106; check in `src/main/scala/org/ergoplatform/nodeView/state/ErgoState.scala`, lines 140 to 214.) The check runs in both UTXO and digest state. The rule can be disabled by soft-fork voting (`mayBeDisabled = true`, `ValidationRules.scala` line 186). Where the rule applies, the candidate generator adds the field to each candidate that carries rent claims, including the miner's own (`CandidateGenerator.scala`, lines 485 and 974).

Nodes that do not know the key space `0x03` accept the field, because outside the known key spaces extension validation checks only sizes, duplicates and non-emptiness (comment at `Extension.scala`, lines 74 to 85).

**When it applies.** `ChainSettings.storageRentAttestationRequired` decides (`ergo-core/src/main/scala/org/ergoplatform/settings/ChainSettings.scala`, line 91):

- plain run: from block version 5 (`Header.StorageRentAttestationVersion`, `Header.scala` line 158). An Ergo v6.0.8 node runs protocol version 4 (`protocolVersion = 4`, `mainnet.conf` line 9), so the rule is dormant there until a version 5 soft fork. Ergo's own v6.0.8 node does not have this rule;
- switch on: from genesis, whatever the block version.

**Rent claims on the YOLO test networks (change 13).** Two existing Ergo settings are turned on in `yolo-testnet.conf`:

- `rejectStorageRentTxs = true` (line 31): the mempool declines rent claims, including those from rolled back blocks (section 3.5);
- `storageRentCollection = true` with `extraIndex = true` (lines 59 and 60): the miner builds its own rent claim and adds it to its candidates. With the switch on, that claim is kept within budgets (section 13.4).

So on these networks only the block's miner includes rent claims. This comes from node settings, not from a consensus rule: rule 308 checks that the claims a block carries are attested, not who built them.

The storage period itself is unchanged: 1,051,200 blocks (`ergo-wallet/src/main/scala/org/ergoplatform/wallet/protocol/Constants.scala`, line 19, `4 * BlocksPerYear` with 720 blocks a day). At Ergo's 2 minute blocks that is four years; at 30 second blocks it is one year.

## 5. Extra index covers the genesis boxes (change 7)

**Ergo.** The extra index skips transaction inputs at height 1, because the genesis boxes that block 1 spends are not in any block and so were never indexed.

**YOLO node.** The genesis boxes, from `ErgoState.genesisBoxes`, are indexed at height 1 before the transactions of block 1, emission box first, so it is box number 0. Tokens held by genesis boxes get their token rows here, because no transaction mints them. Inputs are resolved at every height, including height 1. (`src/main/scala/org/ergoplatform/nodeView/history/extra/ExtraIndexer.scala`, lines 71, 492 and 534; `src/main/scala/org/ergoplatform/nodeView/history/ErgoHistory.scala`, line 321.)

This is not behind the switch. On a plain Ergo node with `extraIndex = true`, Ergo's three genesis boxes are indexed too. The index schema version goes from 7 to 8 (`ExtraIndexer.scala`, line 1077), so an existing extra index is rebuilt once on upgrade (`ErgoHistory.scala`, lines 375 to 393).

## 6. Chain switch, launch parameters and start-up checks (change 8)

Everything here acts only with the switch on.

| Item | Ergo | YOLO node | Where |
|---|---|---|---|
| Launch parameters | By network type | `YoloLaunchParameters`, whatever the network type: Ergo testnet's table (block version 4, rules 215 and 409 off) with the storage fee factor set to 300,000 and the block cost limit to 1,500,000 | `ErgoSettings.scala` line 36; `LaunchParameters.scala` lines 41 to 45; `YoloSettings.scala` lines 59 and 66 |
| Storage fee factor | 1,250,000 | 300,000, a multiple of the 25,000 voting step | `Parameters.scala` lines 296 and 299; `YoloSettings.scala` line 59 |
| Block cost limit at genesis | 1,000,000 | 1,500,000. Voteable afterwards, as on Ergo | `Parameters.scala` line 318; `YoloSettings.MaxBlockCost`, `YoloSettings.scala` line 66; `LaunchParameters.scala` line 44 |
| Rule 110, data inputs unique | From height 1,885,000 | From genesis | `ChainSettings.scala` line 71; `ErgoTransaction.scala` lines 434 and 523 |
| Rule 125, rent outputs distinct | From height 1,885,000 | From genesis | `ChainSettings.scala` line 78; `ValidationRules.scala` line 278 |
| Soft-fork voting start | Hard-coded heights per network | `ergo.chain.yolo.softForkVotingStartHeight`, 0 in the YOLO files | `CandidateGenerator.scala` lines 629 to 637; `yolo-testnet.conf` line 107 |
| Genesis block version | 1, so the genesis header is Autolykos v1 | The launch block version (4), so the whole chain is Autolykos v2 and Sigma 6.0 from genesis | `ChainSettings.scala` line 102; `CandidateGenerator.scala` line 742 |

The start-up checks are listed in section 2.

## 7. Proof of work: table height is block height / 4 (change 9)

**Ergo.** Autolykos v2 uses the header height for three things: the table size N, the table seed `h`, and the `h` field of the work message sent to external miners.

**YOLO node.** All three use `height / powHeightDivisor` instead (`ergo-core/src/main/scala/org/ergoplatform/mining/AutolykosPowScheme.scala`, lines 46, 106 to 109, 191, 321 and 448). The header message `msg` still commits to the real height (line 254, unchanged). The divisor is read from `ergo.chain.powScheme.powHeightDivisor` (`PowSchemeReaders.scala`, line 17). It is 1 in `application.conf` (line 262), which is Ergo's behaviour, and 4 in `yolo-testnet.conf` (line 93). The two-argument constructor is kept, so code built against `ergo-core` still works.

**Effect on table growth.** Ergo starts growing N at height 614,400 (`IncreaseStart`, line 65) and stops at 4,198,400 (line 76). With divisor 4 those points are reached at real heights 2,457,600 and 16,793,600. At 30 second blocks, growth starts after about 2.3 years.

**Known limit.** Sigma's `Header.checkPow`, the ErgoScript method, uses the real header height, so it returns `false` for YOLO headers. Contracts on YOLO must not use it. They can use `Global.powHit` with `height / 4` and `calcN(height / 4)`. A test pins this behaviour (`ergo-core/src/test/scala/org/ergoplatform/mining/YoloSigmaCrossSpec.scala`, lines 20 to 24 and 54).

## 8. Block time and difficulty (change 10)

**Block time.** `blockInterval = 30s` in `yolo-testnet.conf` (line 70). Ergo testnet uses 45 s, Ergo mainnet 2 minutes.

**Ergo.** EIP-37 difficulty runs on mainnet from height 844,673 with 128-block epochs. Other networks use the older calculation. (`src/main/scala/org/ergoplatform/nodeView/history/storage/modifierprocessors/HeadersProcessor.scala`, lines 338 and 357 onward.)

**YOLO node.** With the switch on, EIP-37 runs from genesis (`HeadersProcessor.scala`, lines 340 to 356):

- epoch length is `eip37EpochLength`, which must be 64, so 32 minutes at 30 s blocks (`YoloSettings.Eip37EpochLength`, `YoloSettings.scala` line 84; `yolo-testnet.conf` line 78). `epochLength`, the epoch of Ergo's older calculation, is not used with the switch on; `yolo-testnet.conf` sets it to 64 as well, in place of 128 from `testnet.conf` (line 79). The voting epoch is a separate setting and stays 128 blocks (section 14);
- **first-retarget guard.** A retarget happens after parent height `h` only when `h` is a multiple of the epoch length and `h / epochLength >= 2` (`DifficultyAdjustment.scala`, line 118). At parent height 64 the calculation would ask for the header at height 0, which does not exist, and EIP-37 needs two headers (comment at lines 106 to 111). So blocks 1 to 128 keep the genesis difficulty, and block 129 is the first retarget;
- **floor.** Every retarget result is raised to at least 2 (`YoloSettings.MinDifficulty`, `YoloSettings.scala` line 75; `DifficultyAdjustment.scala` line 131). At difficulty 1 EIP-37 cannot climb, because 1 x 3 / 2 rounds to 1, and after a slow epoch it can fall to 0. The floor is a constant, not a setting, so no node can restart with a different one.

The EIP-37 calculation itself is Ergo's, unchanged: each retarget moves at most to 1.5 times or half the last difficulty (`DifficultyAdjustment.scala`, lines 80 to 102).

The genesis difficulty in `yolo-testnet.conf` is `"02"` (line 86). The file marks it as a placeholder to be set before launch.

**Future timestamps.** Validation rule 211 (`hdrFutureTimestamp`) refuses a header whose timestamp is too far ahead of the node's clock. Ergo allows 10 block intervals. With the switch on, the YOLO node allows 4 block intervals, so 2 minutes at 30 s blocks (`YoloSettings.MaxTimeDriftBlocks`, `YoloSettings.scala` line 91; `ChainSettings.maxTimeDrift`, `ChainSettings.scala` lines 111 and 112). `HeadersProcessor.scala` applies this limit to the genesis header and to every other header (lines 58, 436 and 451). A header past the limit is refused as a recoverable error, so it can be accepted later, once the node's clock has caught up. With the switch off the limit stays 10 block intervals, as in Ergo. The rule's message is Ergo's text and still says 20 minutes (`ValidationRules.scala`, line 142). With the switch on, the detail after it gives this chain's limit in milliseconds (`HeadersProcessor.scala`, lines 406 to 412). A node whose clock runs more than 2 minutes slow refuses new headers, so the node's clock must be kept correct.

## 9. Emission, genesis boxes and contract pins (change 11)

Ergo's emission rules stay in the code. With the switch on, the coinbase and the `/emission` routes use the YOLO rules instead, from `ergo-core/src/main/scala/org/ergoplatform/mining/emission/YoloEmissionRules.scala`. The file states that it computes, in integer arithmetic, what the emission contract `emission.es` from the YOLO contracts repository computes (lines 17 to 20).

- **Curve.** The reward per block, the halving every 1,051,200 blocks and the tail floor are constants at lines 99 to 137.
- **Split.** Treasury `reward * 4 / 100`, liquidity fund `reward * 3 / 100`, miner the rest: 93% / 4% / 3% (lines 114, 117 and 143 to 147).
- **Last spend.** When the emission box holds no more than one reward, the whole box is split the same way and the emission NFT is burned (`terminalHeight`, line 46; `isTerminal`).

**Genesis boxes.** With the switch on, `ErgoState.genesisBoxes` returns the YOLO boxes instead of Ergo's (`ErgoState.scala`, line 321; `src/main/scala/org/ergoplatform/nodeView/state/YoloGenesis.scala`, lines 25 to 35 and 117 to 120):

1. the emission box: emission contract, the emission NFT, R4 and R5 the hashes of the treasury and liquidity accumulation contracts;
2. the treasury fund box: treasury fund contract, the treasury NFT, R4 to R8 set to empty starting values;
3. the liquidity fund box: liquidity fund contract, the liquidity NFT, R4 the configured whitelist, R5 to R7 starting values;
4. Ergo's no-premine box, built as in Ergo. It is always included (line 119). Whether to keep it for mainnet is still to be decided; dropping it would change the genesis state digest (lines 32 and 33).

There is no founders box. All inputs come from `ergo.chain.yolo.emission`: five contract trees, three NFT ids, three values, the whitelist, and `allowTestSignerKeys` (`application.conf`, lines 308 to 332).

**Emission box tracking.** Ergo finds the emission box by its script and, after EIP-27, by an NFT, and drops any candidate under 100,000 coins as spam. The YOLO node requires both the emission contract tree and the emission NFT at `tokens(0)` with amount 1. The 100,000 coin filter is off, because the YOLO emission box holds less than that near its end. After the last spend, an empty stored id means "no emission box". (`src/main/scala/org/ergoplatform/nodeView/state/UtxoStateReader.scala`, lines 84, 101 and 129; `YoloSettings.scala` line 176.)

**Launch check.** With the switch on, the node refuses to start unless all ten of these conditions hold, checked in this order (`YoloGenesis.launchError`, lines 506 to 526). The code numbers them 1 to 8, with 4b and 4c after 4:

1. all five trees parse and every id and hash is well formed (line 135);
2. the three NFT ids differ and none is a placeholder with all bytes equal (line 166);
3. no NFT id equals the id of a genesis box, since the first spender of that box could mint a second unit (line 187);
4. each tree holds its own NFT id as a constant and no other NFT id, and the five trees differ (line 207);
5. (4b) each tree is the pinned contract for its role (`YoloTreePins.scala`): same header byte, same template hash and the same value for every constant, except the NFT id and, for the two fund contracts, three different signer keys (line 247);
6. (4c) neither fund tree holds any of the three public test signer keys that the contract sources are written with, unless `ergo.chain.yolo.emission.allowTestSignerKeys = true` (line 267). The keys are listed in `YoloTreePins.TestSignerKeys` (`YoloTreePins.scala`, line 119). The setting is false by default (`application.conf` line 331; `YoloSettings.scala` line 139) and true only in `yolo-devnet.conf` (line 60). The error names the tree, the constant index and the setting. With the setting true, the node starts but logs an error at every start that names each fund guarded by a test key (`YoloGenesis.testSignerKeysWarning`, line 302). This is a start-up check, not a consensus rule;
7. (5) the emission box has exactly the listed token and registers (line 353);
8. (6) both fund boxes have exactly the listed tokens and registers; the whitelist has at most 64 entries, each at height 0 (line 370);
9. (7) each fund box holds at least the contracts' fund floor (`FundFloor`, line 47), and the genesis emission value is positive and does not strand the emission box (lines 432 and 470). "Stranded" means the last emission spends would fall below the dust limit at the largest `minValuePerByte` that voting can reach;
10. (8) `ergo.chain.genesisStateDigestHex` equals the digest of the genesis boxes. The error message prints the computed digest (line 492).

The pins must be regenerated whenever a contract source or the compiler changes. `YoloTreePinsSpec` derives them from the trees in `yolo-devnet.conf` and fails if they differ (comment at `YoloTreePins.scala`, lines 20 to 23).

## 10. Reward transaction (change 12)

**Ergo.** The coinbase spends the emission box into two outputs: a new emission box and one miner output at `rewardOutputScript(minerRewardDelay, minerPk)`, with `minerRewardDelay = 720`. After EIP-27 activates, the emission box also carries re-emission tokens (`CandidateGenerator.scala`, lines 1264 onward).

**YOLO node.** `YoloEmissionTx.build` (`src/main/scala/org/ergoplatform/mining/YoloEmissionTx.scala`, line 61) builds the transaction the emission contract accepts. It has one input, the emission box with an empty proof, and no data inputs. Every output has the block height as its creation height.

- **Normal spend, 4 outputs:** the new emission box (value less the reward, same tree, only the emission NFT, R4 and R5 copied), the treasury share, the liquidity share, and the miner share.
- **Last spend, 3 outputs:** treasury, liquidity and miner, split from the box value. The emission NFT is in no output, so it is burned.

The miner output is locked for 2,880 blocks, 24 hours at 30 seconds: `rewardOutputScript(2880, minerPk)` (`YoloEmissionTx.CoinbaseMinerRewardDelay`, line 40). The fee output keeps `minerRewardDelay = 720` (`testnet.conf` line 44). `CandidateGenerator.scala` uses this builder in place of Ergo's coinbase with the switch on (lines 1255 to 1263).

Two more checks with the switch on. When the candidate falls back to an emission-only block, the emission transaction is validated first (`yoloFallbackEmissionCheck`, lines 1081 to 1095, called at line 1013). If a candidate is built without the emission transaction, the node logs the reason at error level (lines 912 to 921).

**Wallet.** With the switch on, the node wallet tracks two mining scripts per key: the fee script at 720 and the coinbase script at 2,880. Each box is released to the spendable balance after the delay of its own script. (`src/main/scala/org/ergoplatform/nodeView/wallet/WalletCache.scala`, lines 67 to 87; `WalletVars.scala` line 58; `WalletScanLogic.scala` lines 93 and 210.)

## 11. API (change 14)

All of these change only with the switch on. With it off the routes answer as in Ergo.

- **`GET /emission/at/{height}`** returns `height`, `minerReward` (the 93% share), `treasuryReward`, `lpReward`, `blockReward` (the sum), `totalCoinsIssued`, `totalRemainCoins`, and `reemitted`, always 0 (`src/main/scala/org/ergoplatform/http/api/EmissionApiRoute.scala`, lines 43 to 46 and 116 to 158).
- **`GET /emission/scripts`** returns the configured `emission`, `treasuryAccumulation`, `lpAccumulation`, `treasuryFund` and `lpFund` contracts as P2S addresses. Ergo's re-emission scripts are left out (lines 53 to 54 and 164 to 170).
- **`GET /mining/rewardAddress`** adds `coinbaseRewardAddress`, the 2,880-block coinbase address. `rewardAddress` stays the 720-block fee address (`MiningApiRoute.scala`, lines 95 to 103).

`src/main/resources/api/openapi.yaml` documents the new `/emission` fields. It does not list `coinbaseRewardAddress` yet.

## 12. Settings files (change 15)

**`src/main/resources/yolo-testnet.conf`** includes Ergo's `testnet.conf` first (line 17), then overrides it:

| Setting | Value | Line |
|---|---|---|
| `ergo.directory` | `.yolo`, so a YOLO node never opens an Ergo database | 23 |
| `node.headerChainDiff` | 400 (200 minutes at 30 s) | 28 |
| `node.rejectStorageRentTxs` | true | 31 |
| `node.maxTransactionCost` | 1,000,000, the relay limit (`testnet.conf`: 800,000). The value in use is capped by the block cost limit (section 13.1) | 56 |
| `node.storageRentCollection`, `node.extraIndex` | true, true | 59, 60 |
| `node.blocksToKeep`, `node.utxo.utxoBootstrap` | -1, false | 62, 63 |
| `chain.blockInterval` | 30s | 70 |
| `chain.eip37EpochLength` | 64 | 78 |
| `chain.epochLength` | 64, not used with the switch on (`testnet.conf`: 128) | 79 |
| `chain.initialDifficultyHex` | `"02"`, placeholder | 86 |
| `chain.powScheme.powHeightDivisor` | 4 | 93 |
| `chain.reemission.activationHeight` | 2147483647, EIP-27 never activates | 98 |
| `chain.yolo.enabled` | true | 104 |
| `chain.yolo.softForkVotingStartHeight` | 0 | 107 |
| `scorex.network.magicBytes` | `[89, 79, 76, 79]`, "YOLO" in ASCII | 121 |
| `scorex.network.bindAddress` | port 9120 | 123 |
| `scorex.network.nodeName` | `yolo-` plus the version | 125 |
| `scorex.network.knownPeers` | none yet | 129 |
| `scorex.restApi.bindAddress` | port 9153, local interface only | 135 |

The comment above `maxTransactionCost` (lines 33 to 55) explains the relay limit and the block assembly policy of section 13.

The address prefix stays 16, from `testnet.conf` line 32. The file notes that YOLO mainnet will use 32 (line 67).

The genesis inputs in `ergo.chain.yolo.emission` are deliberately left unset. A node started on this file alone refuses to start and names the missing fields (lines 109 to 113). The file does not set `allowTestSignerKeys`, so it stays false.

**`src/main/resources/yolo-devnet.conf`** includes `yolo-testnet.conf` (line 16) and adds what a local devnet needs: `allowLocal = true`, no seed peers, a genesis state digest, and filled-in genesis inputs. Its two fund contracts embed public test signer keys, and the file says never to copy them to a public network (lines 34 to 36). It sets `allowTestSignerKeys = true` so that the launch check accepts them (line 60).

## 13. Node policy: relay, fee collection, rent claim and block assembly (changes 16 to 19)

Everything in this section acts only with the switch on. All of it is node policy, not consensus. It decides which transactions a node accepts and relays, and what the node puts into its own block candidates. Block and transaction validity do not change: block validation does not call this code, so a block built under other rules is still valid on a YOLO node (`YoloFeeOutputPolicy.scala`, lines 12 to 15; `YoloAssemblyPolicy.scala`, lines 23 to 26). Other nodes may run other policy.

Paths: `YoloFeeOutputPolicy.scala`, `YoloRefusedBoxes.scala` and `ErgoMemPool.scala` are in `src/main/scala/org/ergoplatform/nodeView/mempool/`; `YoloAssemblyPolicy.scala`, `CandidateGenerator.scala` and `StorageRentClaimBuilder.scala` are in `src/main/scala/org/ergoplatform/mining/`.

### 13.1 Relay limit (change 16)

**Ergo.** `ergo.node.maxTransactionCost` is the largest cost of a transaction the mempool accepts and relays. The mempool cleanup and the API's transaction check use the same value. It is 1,000,000 in `application.conf` (line 50) and 800,000 in `testnet.conf` (line 19).

**YOLO node.** `yolo-testnet.conf` sets it to 1,000,000 (line 56). With the switch on, the value in use is never above the room a full block leaves for one pool transaction: the block cost limit, less the assembly safe gap, less 300,000 kept for the emission transaction, the storage rent claim and the fee transaction, less 2 (`YoloAssemblyPolicy.relayLimitInUse`, lines 71 and 72; `CandidateGenerator.YoloMandatoryTxsCostReserve`, line 1137; `CandidateGenerator.safeGap`, lines 1107 to 1115). At the launch block cost limit of 1,500,000 the safe gap is 150,000, so the cap is 1,049,998 and 1,000,000 is the value in use. A vote that lowers the block cost limit lowers the cap with it. The cap applies at mempool acceptance, at the cleanup's revalidation and at the API check, which all go through one function (`YoloFeeOutputPolicy.validateWithCost`, lines 122 to 137; called from `ErgoMemPool.scala` line 403, `src/main/scala/org/ergoplatform/local/CleanupWorker.scala` line 138 and `src/main/scala/org/ergoplatform/http/api/ErgoBaseApiRoute.scala` line 140).

The node warns at start when `maxTransactionCost` is above the cap for the launch block cost limit (`CandidateGenerator.yoloRelayLimitWarning`, lines 1178 to 1185; logged from `ErgoSettingsReader.scala` line 346). It checks again whenever votes change the block cost limit in use, and warns once per change (`CandidateGenerator.scala`, lines 831 to 839 and 1192 to 1200). It is a warning only: the node starts, and the cap applies anyway.

### 13.2 Fee outputs at relay (change 16)

A fee output is an output locked by the chain's fee proposition. The miner of the block collects these into its fee transaction (`YoloFeeOutputPolicy.isFeeOutput`, lines 46 and 47).

**Ergo.** The relay limit counts the transaction's own validation cost only. A fee output may carry tokens.

**YOLO node.** With the switch on, a transaction is refused at relay when:

- one of its fee outputs carries any token (`YoloFeeOutputPolicy.checkTokens`, lines 73 to 87). This check also applies on a node without a UTXO set, where no cost is known (`ErgoMemPool.scala`, line 428; `ErgoBaseApiRoute.scala`, line 146);
- its own cost plus what its fee outputs add to the fee transaction is above the relay limit in use (`checkCost`, lines 93 to 111). Each fee output counts the input cost, 100 for the fee script, and 4 times the token access cost per token entry (`feeBoxCollectionCost`, lines 57 and 58; `FeeScriptCostBound`, line 43). Since fee outputs with tokens are refused, each one counts the input cost plus 100, which is 2,100 at the launch parameters (`InputCostDefault`, `ergo-core/src/main/scala/org/ergoplatform/settings/Parameters.scala` line 308).

Without the cost part, one relayed transaction with many small fee outputs could cost little itself while its fee transaction never fits in a block (comment at lines 21 to 23).

Such a refusal is a mempool decline, not a validation failure, so no peer is penalised. The transaction id is cached as invalidated, so the same transaction is not processed again (`ErgoMemPool.scala`, lines 410 to 415).

### 13.3 Fee collection (change 17)

**Ergo.** The fee transaction spends every fee box created in the block into one miner output. The output keeps at most 100 token entries (`MaxAssetsPerBox`, 100 in sigma-state v6.0.7, the version the node is built with). Tokens past those are in no output, so they are burned (`CandidateGenerator.scala`, lines 1343 to 1350, the branch kept for the switch off).

**YOLO node.** Only the fee boxes whose tokens fit in the one miner output are collected (`yoloFeeCollection`, lines 1388 to 1440; used at lines 1335 to 1342):

- token-free fee boxes are taken first. They only add value, so they always fit;
- fee boxes with tokens come next, in the order given (by box id, as in Ergo). Each is taken only if the miner output, with its tokens merged by id, stays a valid box: at most 100 token entries, no amount past `Long.MaxValue`, at most the maximum box size, and enough value for its size;
- a fee box that does not fit is left out. Nothing is burned: every token of a collected fee box is in the miner output in full.

So a sender cannot use tokens on its own fee boxes to push token-free fee boxes out of the fee transaction (comment at lines 1363 to 1387). A fee box left out stays in the UTXO set. The fee script would let a later miner spend it, but a node collects only the fee boxes created in the block it is building, so its value is not a fee any miner can be relied on to collect (lines 1376 to 1379). With the token check of 13.2, such boxes reach a candidate only through transactions that did not pass that check, such as transactions the miner is told to include.

### 13.4 Storage rent claim budgets (change 18)

**Ergo.** The miner's own rent claim sweeps up to 100 eligible boxes (`StorageRentClaimBuilder.MaxClaims`, line 54). Its cost grows with the tokens the boxes carry and has no bound of its own. If assembly rejects the claim, all its input boxes are removed from the rent index (`CandidateGenerator.scala`, lines 939 to 944, the branch kept for the switch off).

**YOLO node.**

- The claim is kept within a cost budget of 270,000: the 300,000 reserve less 15,000 each for the emission and fee transactions (`YoloRentClaimCostBudget`, `CandidateGenerator.scala` line 1145; lines 1121, 1130 and 1137). It is also kept within a size budget of 65,536 bytes (`YoloAssemblyPolicy.RentClaimSizeBudget`, line 52). Both are passed to the builder at `CandidateGenerator.scala` lines 806 to 809.
- The builder keeps a running upper bound of the claim's cost and size, and stops at the first box that would take it past a budget. That box and the ones after it stay in the rent index for a later candidate. The first claimable box is always taken, so the index always moves (`StorageRentClaimBuilder.scala`, lines 61 to 92 and 187 to 202). The cost bound includes what each recreated box locked by the fee proposition adds to the fee transaction (`CandidateGenerator.scala`, lines 808 and 809).
- If the claim still does not fit, it is left out of the candidate. It is not reported invalid, so its boxes stay in the index (`YoloAssemblyPolicy.scala`, lines 349 to 353).
- If assembly rejects the claim because it lost a race, so that an input is already spent, in the UTXO set or by a transaction placed before it in the candidate, all its boxes stay in the rent index. Otherwise each box is checked in a claim of its own, and only the boxes that fail alone are removed. If every box passes alone, all inputs are removed, as in Ergo (`yoloRejectedClaimEviction`, `CandidateGenerator.scala` lines 1028 to 1066; used at lines 926 to 938).

With the switch off no budget is passed, and the builder works as in Ergo (`StorageRentClaimBuilder.scala`, lines 166 to 169, defaults of none).

### 13.5 Block assembly (change 19)

**Ergo.** `collectTxs` validates the candidate's transactions in order: the emission transaction, prioritized transactions, the miner's rent claim, then the pool. Each is validated with the whole block budget as its cost limit. After each pool transaction the fee transaction is built and validated again. The first transaction that does not fit within the block limits ends assembly, and so does a fee transaction that fails. A transaction that fails validation is removed from the pool. (`CandidateGenerator.scala`, line 908; `collectTxs`, lines 1462 to 1559.)

**YOLO node.** With the switch on, `YoloAssemblyPolicy.collect` chooses the transactions instead (`CandidateGenerator.yoloCollect`, lines 850 to 889, called at line 900; `YoloAssemblyPolicy.collect`, lines 259 to 474). With the switch off, `collectTxs` runs exactly as in v6.0.8: its code is unchanged, and tests compare the result with a reference copy of the v6.0.8 code (`src/test/scala/org/ergoplatform/mining/UpstreamV608Assembly.scala`).

- **Recorded cost.** When the mempool accepts a transaction, it records the cost it measured, under a key made from the block version and the cost and validity parameters (`ErgoMemPool.scala`, lines 404 to 407; `UnconfirmedTransaction.withRecordedCost`, `src/main/scala/org/ergoplatform/modifiers/mempool/UnconfirmedTransaction.scala` lines 46 to 56; `YoloAssemblyPolicy.costKey`, lines 59 and 60). The cleanup's revalidation and the generator's own measurements can raise the record under the same key, never lower it (`CleanupWorker.scala`, lines 93 to 98; `YoloAssemblyMemory`, lines 486 to 525). A record under another key is not used.
- **Abort limit.** A pool transaction with a record r is validated with a cost limit L of r plus r / 10 plus 5,000. L is never above the room the transaction is guaranteed as the first pool transaction: the assembly budget, less 1, less 300,000, less what its fee outputs add to the fee transaction (`slack`, line 40; `guaranteedRoom`, line 75; `abortLimit`, lines 81 to 84; budget, line 63). A transaction with no record is validated at the relay limit in use. Validation runs in a copy of the context whose block cost limit is L (lines 392 to 401).
- **Passed over, not ended.** These are passed over without being validated, and assembly goes on with the next one: a transaction whose L, or with no record its cost before any script, is above the room left; one too large for the size left; one whose work tier is spent; one that spends or reads an output of a transaction passed over (lines 366 to 386). Only the emission and prioritized transactions end assembly when they do not fit, as in Ergo (lines 328 to 364).
- **Cost more than recorded, or fail.** A transaction with a record that goes over L, or fails validation within L, is removed from the pool, and its input boxes are refused (section 13.6; lines 414 to 426). A transaction without a record that fails is removed, and its inputs are not refused. One whose input or data input is missing, or that double spends, is removed as in Ergo, with no refusal (lines 387 to 390). One whose record is above its guaranteed room is removed without being validated, with no refusal (lines 377 to 379).
- **Work budget.** Work spent on pool transactions that end up out of the candidate is capped per candidate, in three tiers by L: up to 50,000, up to 250,000, and above. Each tier has 1,000,000, 1,000,000 and 1,050,000 cost units (`ZTiers`, lines 42 to 46). A failed transaction is charged its L. A valid one with no record that does not fit is charged its cost (lines 405 to 416). When a pass ran short of its budget and removed a transaction, the generator assembles again at once, at most 3 passes per parent block (`MaxZPassesPerParent`, line 49; `CandidateGenerator.scala`, lines 873 to 881).
- **Fee transaction built once.** While pool transactions are taken, room is kept for the fee transaction by an upper bound of its cost and size (lines 96 to 101 and 299 to 307). The fee transaction is built and validated once, at the end. If it fails, pool transactions are dropped from the end until it is valid (lines 439 to 466).

### 13.6 Refused boxes and the P2PK rule (change 19)

**Ergo.** There is no such rule. A new transaction that spends the inputs of a transaction removed at assembly is accepted as any other.

**YOLO node.** The boxes refused in 13.5 go into a set held by the node's mempool (`YoloRefusedBoxes.scala`; `ErgoMemPool.yoloRefuse`, lines 203 to 214). Data inputs are never refused, nor the inputs of the emission transaction, the rent claim or prioritized transactions (`YoloRefusedBoxes.scala`, lines 15 to 18). Only block assembly adds boxes to the set, so only a node that builds block candidates refuses boxes, and the set is not sent to other nodes. It is kept on disk in a `yolo-refused-boxes` folder in the node's data directory (`ErgoNodeViewHolder.scala`, lines 461 and 462).

- **How long.** A box leaves the set when it is spent in an applied block. An entry whose box is not in the UTXO set is dropped after 2,880 blocks. An entry whose box is still unspent after 2,880 blocks is renewed from that height, so the box stays refused until it is spent on chain (`AgeLimit`, line 80; `aged`, lines 66 to 74; `ErgoNodeViewHolder.scala`, lines 438 to 449).
- **The P2PK rule.** A transaction that spends a refused box is admitted only if it also spends a sponsor: a box that is not refused, whose script is exactly one public key (the script of a P2PK address), with a non-empty proof on its input (`admits`, lines 44 to 49; `isPlainP2PK`, lines 87 to 90). A P2PK box cannot be spent without a valid signature, and the transaction is fully validated after this check. A contract box or a multisig box is not a sponsor, whatever proof it carries. So contract or multisig boxes alone cannot release a refused box. When a sponsored transaction is caught in turn, all its inputs are refused, the sponsor too (comment at lines 22 to 28).
- **The decline.** A transaction declined by this rule is not a validation failure. No peer is penalised and the id is not cached, so the same transaction can be sent again once the box is spent on chain. The message says to add a plain key-signed box (`ErgoMemPool.scala`, lines 354 to 365).
- **The pool.** When boxes are refused, pool transactions that spend them without a sponsor are removed (`ErgoMemPool.yoloRefuse`), and the cleanup does not put back a rechecked transaction that now fails the rule (`ErgoNodeViewHolder.scala`, lines 749 to 757).

**What a wallet or dApp developer must know.**

- None of this is consensus. A block from any miner that spends a refused box, or that carries a fee output with tokens, is valid.
- A fee output must carry no tokens, or the transaction is not relayed. Each fee output adds 2,100 to the transaction's relay cost at the launch parameters, so one fee output is best.
- A transaction that costs more at block assembly than the node recorded for it, plus the slack, or that fails there, gets its input boxes refused by that node. This includes a transaction that becomes invalid while it waits in the pool, for example one whose script holds only before some height, if block assembly finds it before the pool cleanup removes it.
- A refused shared box, such as a DEX pool box or an accumulation box, can still be spent by a transaction that also spends a P2PK box of the sender with its signature. A bot that spends shared contract boxes with no signature must be able to add such an input while a box it spends is refused. A spend of the refused box with only contract or multisig inputs is declined.

## 14. Values still inherited from Ergo

These come from Ergo's settings unchanged. They work on the test networks but have not been chosen for a YOLO mainnet.

| Value | Current | Where |
|---|---|---|
| Max transaction size in the mempool | 98,304 bytes | `application.conf` line 53 |
| Block size limit, voteable | 524,288 bytes | `Parameters.scala` line 314 |
| Voting: epoch length, soft-fork epochs, activation epochs | 128, 32, 32 | `testnet.conf` lines 49 to 55 |
| `reemission.reemissionStartHeight` | 1,860,400 | `testnet.conf` line 76 |
| REST API key hash | Ergo's default | `testnet.conf` line 109 |
| `scorex.network.agentName` | `ergoref` | `application.conf` line 520 |
| `keepVersions` | 200 | `application.conf` line 75 |
| No-premine box strings | Ergo's 2019 strings | `application.conf` lines 224 to 230 |

## 15. Tested on a live chain or not

These were changed or added after the live test chains ran:

- the 64-block difficulty period and the 2 minute future-timestamp limit (section 8);
- the 1,500,000 block cost limit (section 6);
- the refusal of the public test signer keys (section 9);
- the 1,000,000 relay limit and its cap (sections 12 and 13.1);
- everything else in section 13: the relay cost of fee outputs, the refusal of fee outputs with tokens, fee collection, the rent claim budgets, block assembly and refused boxes.

All of these are in the code and covered by unit tests only, for example `YoloDifficultyHistorySpec`, `YoloFutureTimestampSpec`, `YoloGenesisSpec`, `YoloConfigSpec`, `YoloFeeOutputPolicyRouteSpec`, `YoloAssemblyPolicySpec` and `YoloBlockAssemblySpec` under `src/test/scala/org/ergoplatform/`. A live test chain ran at these settings on 23 and 24 Nov 2026 (results below). It did not exercise the timestamp refusal, the fee output policy or the block assembly defence, which stay unit tested only.

The unit test suites pass on the current tree with none failing: 1,167 tests in the node, 149 in `ergo-core`, 57 in `ergo-wallet` and 19 in `avldb`.

Live-chain results at the current settings, from a private test chain of 3,370 blocks mined by one GPU through a stratum bridge: no block rejected; the first difficulty change at block 129 and a change at every period boundary after it, 51 changes in all; a 31 second average block time; the 93 / 4 / 3 split on every block; a spend of a block reward refused before 2,880 blocks and mined after; and forwards of 189 accumulation boxes in one transaction for each fund, with 190 refused at cost 1,004,788 against the 1,000,000 relay limit.

Earlier live-chain results were at the old settings: a 128-block difficulty period, a 5 minute (10 block interval) timestamp limit, a block cost limit of 1,000,000 and a relay limit of 800,000. Those results are: GPU mining through a stratum bridge; the first difficulty change at block 257, nine changes through block 1,348 and a 29.7 second average block time; and forwards from the accumulation boxes of up to 163 boxes in one transaction.
