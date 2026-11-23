# fundsign: signing tool for the treasury and liquidity fund

**Status: draft, 2026-11-15. Reviewed once and fixed. Run on two-node devnets, with test keys and with keys held in the node wallets.**

The treasury and the liquidity (LP) fund are each one box guarded by a 2-of-3 contract. Any change to either box needs two of the three signers. This tool builds the transaction, shows each signer what it does, and carries out the two-person signing through each signer's own node.

It needs Python 3.8 or later and nothing else.

## What each signer needs

- Their own YOLO node, synced, with `extraIndex = true`.
- A node wallet that holds their signer key and is unlocked.
- The node API key in a file, passed with `--api-key-file`, or in the `YOLO_NODE_API_KEY` variable.
- For the signer who builds a transaction (the proposer): a small box in the wallet to pay the miner fee, at a plain public key address (P2PK), holding no tokens, and holding exactly the fee or at least the fee plus 0.001 YOLO. Payments out of a fund (treasury `execute`, LP `spend`) pay the fee from the fund and need no such box. `cosign` refuses if the second signer's wallet could sign the fee box.

## One signing round

Two signers take part. Files go between them by any channel. None of the shared files holds a key.

| Step | Who | Command | File produced |
|---|---|---|---|
| 1 | Proposer | `fundsign.py build treasury approve --to ADDRESS --amount 500` | `NAME.proposal.json` |
| 2 | Proposer | `fundsign.py commit NAME.proposal.json` | `NAME.commit.json` (share), `NAME.secret.json` (keep) |
| 3 | Second signer | `fundsign.py show NAME.proposal.json` | none |
| 4 | Second signer | `fundsign.py cosign NAME.proposal.json --commit NAME.commit.json` | `NAME.partial.json` (send back) |
| 5 | Proposer | `fundsign.py finish NAME.proposal.json --partial NAME.partial.json --send` | `NAME.signed.json` |

Every signing command prints a summary first and asks for `YES`. The summary is worked out from the transaction and from your own node. It does not rely on anything the proposal file says about itself. Read it. Check the amount and the address against what you agreed to by another channel.

For a `whitelist-add`, give `show`, `cosign` and `finish` one `--expect-dest ADDRESS` for each destination agreed by another channel. The summary then shows each address next to its hash. `cosign` and `finish` refuse unless every added hash is one of those addresses and every one of those addresses is added.

`NAME.secret.json` is for one proposal only. Never share it and never reuse it. `commit` refuses to run while it exists. `finish` deletes it before it signs, so if `finish` fails for any reason, start again from step 2 (`commit`, then a new `cosign`).

## Actions

| Fund | Action | What it does | Fee paid by |
|---|---|---|---|
| both | `forward [--min-box N]` | Moves accumulation boxes (each block's share) into the fund box. Boxes below N YOLO (default 0.01) are skipped. The smallest block share in the schedule is 0.03 YOLO | proposer |
| both | `freeze`, `unfreeze` | Stops or restarts the fund | proposer |
| treasury | `approve --to A --amount N` | Records a payment. It can be paid 8,641 blocks later (72 hours) and for 20,160 blocks after that | proposer |
| treasury | `cancel` | Clears the recorded payment, or a recorded migration approval | proposer |
| treasury | `execute` | Pays the recorded payment | fund |
| lp | `whitelist-add --dest A` | Adds a destination. Usable 8,641 blocks later. It can never be removed | proposer |
| lp | `spend --to A --amount N` | Pays a destination that has been on the list long enough | fund |
| lp | `cancel` | Clears a recorded migration approval | proposer |

`fundsign.py status` shows both funds, any recorded payment, the whitelist and what is waiting to be forwarded. `fundsign.py hash ADDRESS` gives the whitelist hash of an address, to compare with a `whitelist-add` summary (or pass `--expect-dest`, above).

Migration (moving a fund to a new contract) is not supported. The tool refuses to sign a migration approval or a migration. It does sign the cancel of a migration approval.

## Time limits

Every summary ends with the height by which the transaction must be in a block.

- Most actions: 720 blocks, about 6 hours.
- `execute`: 720 blocks, or the end of the payment window if that comes first. It cannot be built or signed before the wait is over.
- `approve` and `whitelist-add`: **10 blocks, about 5 minutes at 30 second blocks.** The contract only accepts a recorded height within 10 blocks of the block that includes it. Both signers must be online together and work through steps 1 to 5 at once. If it expires, build it again.

## What was tested

On two-node devnets with the launch contracts, 2026-11-15, after the review fixes:

| Action | Test keys from a file | Keys held in the node wallets |
|---|---|---|
| Treasury forward, approve, cancel, freeze, unfreeze | Confirmed on chain | Confirmed on chain |
| Treasury execute (payout after the 8,640-block wait) | Confirmed, 40 YOLO received | Not run |
| LP forward, whitelist-add | Confirmed on chain | Confirmed on chain |
| LP freeze, unfreeze | Confirmed on chain | Not run |
| LP spend (after the wait) | Confirmed, 120.5 YOLO received | Not run |

Refused, as intended: an early payout, a spend to an unlisted or not yet usable destination, the same key signing twice, a proposal swapped after commit, a freeze altered to also pay out, a node with no signer key, a whitelist addition without `--expect-dest`, and a second signer whose node is behind.

Offline tests: `python3 -I test_fundsign.py` (66 tests).

Not yet tested: real 30 second block timing, signers on separate machines, and LP migration cancel on a node.

## Known limit in the node

The node's own multisig calls fail when a transaction has an input that needs no signature, which is every forward. The tool works around this by making two of the calls on a reduced copy of the transaction. Following the Ergo multisig instructions by hand will not work for a forward.
