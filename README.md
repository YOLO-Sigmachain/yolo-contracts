# YOLO SigmaChain contracts

YOLO SigmaChain is a planned proof-of-work blockchain forked from the Ergo node at v6.0.8. It uses Autolykos v2, the same proof-of-work as Ergo. Contracts are written in ErgoScript, with Sigma 6.0 active from the first block.

This repository holds the five contracts that exist from the first block, their tests, and the tool the fund signers use.

**Status, 24 November 2026: no public network exists.** The contracts and the node run on private test chains only. Nothing here is deployed.

[Parameters](#parameters) · [Launch contracts](#the-launch-contracts) · [Documents](#documents) · [Related repositories](#related-repositories)

## Parameters

| Parameter | Value |
|---|---|
| Proof-of-work | Autolykos v2 |
| Block time | 30 seconds |
| Block reward | 75 YOLO for the first 1,051,200 blocks (one year), then halved each year for five more years: 37.5, 18.75, 9.375, 4.6875, 2.34375. From block 6,307,200 it is 1.5 YOLO per block |
| Last emission payout | Height 21,023,916. After that, miners are paid by transaction fees and storage rent |
| Total supply | 177,291,451 YOLO. The genesis emission box holds 177,291,250. The treasury and liquidity fund boxes hold 100 each. The no-premine proof box holds 1, which no one can spend |
| Reward split | 93% miner, 4% treasury, 3% liquidity fund, enforced by the emission contract |
| Miner reward lock | 2,880 blocks, about one day |
| Premine | None. No founders allocation |
| Storage rent | Boxes unmoved for 1,051,200 blocks (one year) can be charged. Fee factor 300,000 nano per byte per year at launch |
| Rent collection | A rent claim is valid in a block only if that block's miner attests it, from the first block. Nodes do not relay rent claims; a mining node builds its own |
| Treasury and liquidity fund custody | 2-of-3 multisig contracts. Every payment waits 72 hours on chain before it can execute |

## The launch contracts

| Contract | File | What it guards |
|---|---|---|
| Emission | `01-emission-tests/emission.es` | The box that pays each block's reward and enforces the split |
| Treasury accumulation | `02-treasury-tests/treasury_accumulation.es` | Each block's 4%, until it is forwarded to the treasury |
| Treasury | `02-treasury-tests/treasury_governance.es` | The treasury: 2 of 3 signers, 72 hour wait on every payment |
| Liquidity accumulation | `03-lp-fund-tests/lp_accumulation.es` | Each block's 3%, until it is forwarded to the liquidity fund |
| Liquidity fund | `03-lp-fund-tests/lp_fund.es` | The liquidity fund: 2 of 3 signers, payments only to destinations listed at least 72 hours earlier |

How each transaction works, with a diagram per transaction: [docs/launch-contracts.md](docs/launch-contracts.md).

Three things to know before reading them:

- **The signer keys in the two fund contracts are public test keys.** Their secrets are in the test suites. They are replaced before genesis. The node refuses to start on a fund contract that holds them, unless the setting `allowTestSignerKeys` is true. Only the local devnet settings file turns it on.
- **The three NFT ids are placeholders in the source.** They are fixed when the genesis state is built.
- **The liquidity fund whitelist gives public notice, not control of use.** The same signers who spend can add destinations. Once funds reach an exchange or a bridge, no contract on YOLO controls them.

## What has been tested

- Each contract has its own test suite, and `05-integration-tests` runs them together.
- On private test chains the node paid the 93 / 4 / 3 split on every block, and the 2,880 block miner lock held.
- Forwards from the accumulation boxes into both funds were made at 30 second block times, up to 189 boxes in one transaction.
- A GPU mined 3,370 blocks through a stock Ergo stratum bridge at the launch settings: a 64-block difficulty period, a 2 minute limit on how far a block's timestamp may be ahead of a node's clock, a block cost limit of 1,500,000 and a relay limit of 1,000,000. Block time averaged 31 seconds and no block was rejected.

## What does not exist yet

- A public testnet, published node builds, seed nodes and a hosted explorer.
- A starting difficulty. It is set just before the public testnet.
- Final signer keys for the treasury and the liquidity fund.
- An operator for the keeper that forwards treasury and liquidity boxes.
- Published wallet builds.

## Repository layout

| Directory | Contents |
|---|---|
| `01-emission-tests` | Emission contract, a reference model of the schedule, and tests |
| `02-treasury-tests` | Treasury accumulation and treasury contracts, with tests |
| `03-lp-fund-tests` | Liquidity accumulation and liquidity fund contracts, with tests |
| `05-integration-tests` | Tests across all five contracts, and the script that compiles them |
| `09-fund-signing` | The signing tool for the two funds, with its own instructions |
| `docs` | The guides listed below |

## Running the tests

| What | Where | Command |
|---|---|---|
| Emission model tests | `01-emission-tests` | `python3 test_emission_exhaustive.py` |
| Contract tests | `01-emission-tests`, `02-treasury-tests`, `03-lp-fund-tests`, `05-integration-tests` | `cargo test --release` |
| Signing tool tests | `09-fund-signing` | `python3 -I test_fundsign.py` |
| Check the compiled contracts match the source | `05-integration-tests/compile` | `python3 gen_trees.py --check` (needs scala-cli and a JVM) |

## Documents

- [docs/launch-contracts.md](docs/launch-contracts.md): what each contract does, one transaction at a time.
- [docs/developer-guide.md](docs/developer-guide.md): what differs from Ergo for people building on YOLO.
- [docs/solo-mining-guide.md](docs/solo-mining-guide.md): node, stratum bridge and GPU miner.
- [docs/node-changes.md](docs/node-changes.md): what the node changes from Ergo.

## Related repositories

All are forks, published so that every change from the original project can be seen.

| Repository | Fork of |
|---|---|
| `yolo-node` | ergoplatform/ergo |
| `yolo-wallet-web` | nautls/nautilus-wallet |
| `yolo-wallet-mobile` | minotaur-ergo/minotaur-wallet |
| `yolo-sigma-rust` | ergoplatform/sigma-rust |
| `yolo-explorer-backend` | ergoplatform/explorer-backend |
| `yolo-graphql` | nautls/ergo-graphql |
| `yolo-explorer` | arkadianet/kadia-explorer |

## Licence

MIT. See [LICENSE](LICENSE).
