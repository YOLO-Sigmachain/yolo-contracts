# Building on YOLO: what differs from Ergo

**Status: 22 Nov 2026.** YOLO has run only on private test chains. There is no public testnet yet, so this guide lists no endpoints, seed nodes or downloads.

## The short version

YOLO is a fork of the Ergo reference node (v6.0.8). Contracts are ErgoScript, transactions and boxes have the same format, and the node API is the same. If your code works on Ergo, most of it works on YOLO after you change the node address and re-check anything that counts blocks.

What changes:

| | Ergo | YOLO |
|---|---|---|
| Block time | 120 s | 30 s |
| Blocks per day | 720 | 2,880 |
| Script version at launch | 6.0 after a soft fork | 6.0 from block 1 |
| Coinbase reward lock | 720 blocks | 2,880 blocks |
| Fee box lock | 720 blocks | 720 blocks (same script as Ergo) |
| Storage rent period | 1,051,200 blocks (4 years) | 1,051,200 blocks (1 year) |
| Storage fee factor at launch | 1,250,000 | 300,000 |
| Storage rent claims | anyone can make one | each claim must be attested by the block's miner (rule 308) |
| Address prefix | testnet 16, mainnet 0 | testnet 16, mainnet 32 |
| Proof-of-work height input | block height | block height / 4 |
| Default API port | 9053 mainnet, 9052 testnet | 9153 |
| Default peer port | 9030 mainnet | 9120 |

## Time: everything that counts blocks runs 4 times faster

Contracts measure time in block height. A constant copied from an Ergo contract lasts a quarter as long on YOLO.

| Wall-clock time | Ergo blocks | YOLO blocks |
|---|---|---|
| 1 hour | 30 | 120 |
| 1 day | 720 | 2,880 |
| 1 week | 5,040 | 20,160 |
| 1 year | 262,800 | 1,051,200 |

Check every height constant when you port a contract: timelocks, oracle epochs, auction lengths, refund windows, vote periods. Also check off-chain code that waits for confirmations. Ten confirmations is 20 minutes on Ergo and 5 minutes on YOLO.

## Addresses

- Testnet addresses use the same prefix as Ergo testnet (16), so an Ergo testnet address and a YOLO testnet address for the same key look identical. They are different chains; do not mix them up.
- Mainnet uses prefix 32. Ergo mainnet addresses are not valid YOLO mainnet addresses, and libraries that only know prefixes 0 and 16 need the new value. Prefix 32 is covered by unit tests in the YOLO web wallet and in the YOLO fork of sigma-rust. No wallet has run on a prefix-32 chain yet.
- Wallets use Ergo's key path (m/44'/429'), so one seed phrase gives the same keys on Ergo and YOLO.

## Scripts

- **Sigma 6.0 from the first block.** The 6.0 types and methods are available without waiting for activation. Not every 6.0 method has been run on a YOLO test chain yet.
- **`Header.checkPow` does not work for YOLO headers.** It computes proof-of-work with the block height, and YOLO uses height / 4. A contract that verifies YOLO headers must use `Global.powHit` and pass the height divided by 4 itself, and the table size N for that same value.
- **`CONTEXT.headers` and `preHeader`** work as on Ergo.
- **EIP-27 re-emission does not exist on YOLO.** Its rules are switched off for the life of the chain.

## Fees and miner rewards

- The transaction fee box uses Ergo's standard fee script (720-block lock), so SDKs that hard-code that script (for example appkit) build valid fee outputs unchanged.
- The coinbase miner reward is locked for 2,880 blocks, about one day. Wallet code that recognises mining rewards by Ergo's 720-block reward script will not see YOLO rewards until it also knows the 2,880 version.
- Each block's emission transaction has four outputs: the emission box, treasury (4%), liquidity fund (3%) and miner (93%). The 4% and 3% go to two accumulation contracts, which later forward them to the treasury and liquidity fund boxes. The last emission transaction has three outputs (treasury, liquidity fund, miner) and burns the emission NFT. Code that assumes Ergo's two-output emission transaction needs updating.

## Storage rent

- A box that has not moved for 1,051,200 blocks (one year) can be charged rent, as on Ergo after four years.
- A rent claim can go into a block only if that block's miner attests it in the block (validation rule 308). Rule 308 can be switched off later by a miner soft-fork vote.
- By default a YOLO node refuses rent-claim transactions sent over the network (the `rejectStorageRentTxs` node setting), and miners collect rent into their own blocks (`storageRentCollection`).
- Long-lived contract boxes (pools, vaults, oracles) need enough value to pay rent, or must be touched at least once a year. On Ergo the same box would have four years.
- Two validation rules that Ergo activates at height 1,885,000 are active on YOLO from block 1: a rent claim's outputs must be distinct, and a transaction's data inputs must be unique.

## Difficulty, limits and voting

- Difficulty uses EIP-37 from the first block. Each change is at most 1.5 times up or half down, and difficulty never goes below 2.
- **Difficulty period: 64 blocks, about 32 minutes.** Block heights start at 1, not 0, so the first change needs two full periods to measure. It comes at block 129, and blocks 1 to 128 keep the starting difficulty. Tested on a live chain on 23 and 24 Nov 2026 (see the test results below).
- **A block's timestamp may be at most 2 minutes (4 block intervals) ahead of a node's clock.** Ergo allows 10 block intervals. Keep node clocks correct. The live test chain ran with this limit and refused no block. The refusal of a block that is too far ahead is unit tested only.
- **Live test results at these settings:** a private test chain ran for 3,370 blocks on 23 and 24 Nov 2026, mined by one GPU through the stratum bridge. No block was rejected. The first difficulty change came at block 129, and difficulty changed at every period boundary after it, 51 changes in all through block 3,329. Block time averaged 31 seconds over the first 3,344 blocks. An earlier GPU test chain ran at a 128-block period and a 5 minute timestamp limit.
- Block limits: cost and size. The launch cost limit is 1,500,000 and the size limit 524,288 bytes. Miners can vote to change both. The live test chain ran at the 1,500,000 cost limit.
- Relay limits, which are node settings and not consensus rules: the YOLO testnet settings file has a node relay a transaction only if its cost is at most 1,000,000 and its size at most 98,304 bytes. A YOLO node never uses a cost limit above the room a full block leaves for one pool transaction: 1,049,998 at the launch block cost limit, and lower if a vote lowers the block cost limit. On the live test chain a forward of 189 accumulation boxes was accepted and mined for each fund, and one of 190 boxes was refused at cost 1,004,788.
- A voting epoch is 128 blocks (about 64 minutes). A soft-fork vote runs 32 epochs, and activation takes another 32 epochs. Each 32-epoch window is 4,096 blocks, about 34 hours.
- A node keeps 200 blocks of state versions by default, so the deepest rollback is about 100 minutes.

## Relay and block building policy

These are policies of a YOLO node, not consensus rules. A block that breaks them is still valid. They are in the node and unit tested. On the live test chain only the relay cost limit was exercised.

- **Fee outputs count toward the relay cost.** A node relays a transaction only if its own cost plus what its fee outputs add to the miner's fee transaction is within the relay limit. Each fee output adds 2,100 at the launch parameters. Use one fee output.
- **No tokens in fee outputs.** A transaction with a fee output (a box at the standard fee script) that carries any token is not relayed. Ergo allows this. The node then remembers the transaction id, so build a new transaction rather than sending the same one again.
- **Refused boxes.** When a node building a block finds that a pool transaction costs more than the node measured when it accepted it, plus a tenth and 5,000, or that it now fails, the node removes it and refuses its input boxes until they are spent in a block. Only that node refuses them. A transaction that spends a refused box is accepted only if it also spends a plain key-signed box of its own (a box at a P2PK address, with its signature). A contract box or a multisig box does not count, so a transaction with only contract or multisig inputs cannot spend a refused box. If a bot spends shared contract boxes with no signature, for example pool or accumulation boxes, it must be able to add a P2PK input while a box it spends is refused. The node's error message says when this is the case.

## Node API

Same routes as Ergo v6.0.8. Some return different or extra fields:

- `/info` reports `network: "testnet"`. The node accepts the YOLO settings only with network type `testnet`, so this holds on every YOLO network unless that check changes.
- `/mining/candidate` returns `h` as block height / 4. `msg` still commits to the real height.
- `/emission/at/{height}` reports the YOLO schedule and adds `treasuryReward`, `lpReward` and `blockReward`. `minerReward` is the miner's 93% share. `reemitted` is always 0.
- `/emission/scripts` returns five addresses: `emission`, `treasuryAccumulation`, `lpAccumulation`, `treasuryFund` and `lpFund`. Ergo's two re-emission addresses are not returned.
- `/mining/rewardAddress` also returns `coinbaseRewardAddress`, the 2,880-block address that block rewards are paid to. `rewardAddress` is still the 720-block address used for fees.
- `/mining/candidateWithTxsAndPk` is available (the endpoint Lithos uses). It needs the API key.
- The API listens on the local interface only by default. The default API key hash is the upstream Ergo default; set your own `scorex.restApi.apiKeyHash` before you open the API to other machines.

## Supply and units

- 1 YOLO = 1,000,000,000 nanoYOLO, the same nine decimals as ERG.
- Total supply on the current devnet settings is 177,291,451 YOLO: 177,291,250 in the emission box, 100 in each of the two fund boxes, and 1 in the genesis no-premine proof box. There is no founders' allocation. These genesis values are settings and are not yet fixed for the public testnet.
- The emission contract releases the 177,291,250 over about 20 years: 93% to miners, 4% to the treasury and 3% to the liquidity fund.
- The block reward starts at 75 YOLO and halves every 1,051,200 blocks (one year) for six steps: 75, 37.5, 18.75, 9.375, 4.6875 and 2.34375 YOLO. From height 6,307,200 it is 1.5 YOLO, until the emission box runs out at height 21,023,916.
- The no-premine proof box has a script that no key can spend. Like any box, it can be charged storage rent after a year.

## Tools

| Tool | Expected to work | Notes |
|---|---|---|
| ErgoScript compilers | Yes | Same language and tree format |
| appkit, sigma-rust, Fleet | Mostly | Point at a YOLO node. Stock sigma-rust refuses address prefix 32; the YOLO fork of sigma-rust adds it. Prefix 32 has not been tested with appkit or Fleet |
| Ergo node wallet API | Yes | Built into the YOLO node |
| Kadia Explorer (arkadianet) | Yes, with changes | Adapted for YOLO and run against a private test chain. Not hosted yet |
| Ergo explorer backend (Postgres) | Yes, with changes | Forked for YOLO with its GraphQL server, to serve the web wallet. Run for 6 hours at 30 s blocks with forced forks. Not hosted yet |
| Minotaur wallet | Fork only | Stock Minotaur does not work with YOLO. A fork, Yolo Mobile Wallet, is built with a signing library that writes prefix 32. Tested in a desktop browser against a live test node (create, receive, send). Not yet run on a phone, and not published yet |
| Nautilus wallet | Fork only | Stock Nautilus does not work with YOLO. A fork, Yolo Web Wallet, is built and was hand tested in Firefox on a test chain (send and token mint). Mainnet prefix 32 is covered by unit tests only |
| Fund signing (2-of-3) | Yes | `09-fund-signing/fundsign.py`, run on devnets with test keys and with keys held in node wallets |
| GPU miners for Ergo | Yes, one tested | SOAT Miner 0.2.18 mined at about 150 MH/s on one RTX 3080 through a stock Ergo stratum bridge, on a private test chain. Not every Ergo miner runs well on YOLO; see `solo-mining-guide.md` |

## Getting a test node

There is no public testnet, download, seed list or faucet yet. The YOLO testnet settings file in the node lists no seed nodes and leaves the genesis unset, so a node started on it alone refuses to start. No YOLO mainnet settings file exists yet. Until the public testnet exists, the only way to test is a private devnet built from the fork source.

In the devnet settings file, the two fund contracts hold public test signer keys, and the file sets `ergo.chain.yolo.emission.allowTestSignerKeys = true`. Without that setting the node refuses to start on those keys. It is false by default and must stay false on any public network. This check is in the node and unit tested, but not yet run on a live chain.

A YOLO node uses its own network magic bytes ("YOLO" in ASCII) and its own data folder, `.yolo`, so it does not open an Ergo database.
