# LP Fund Whitelist: Initial Entries

## Overview

The LP Fund governance contract (v2.2; 2026-11 review fix F-005, entry cap from the 2026-11 retest) stores the whitelist in R4 as `Coll[(Coll[Byte], Long)]`: one pair per allowed destination, holding the 32-byte blake2b256 hash of the destination ErgoTree and the height at which the entry was added. Funds can only be sent to contracts whose `blake2b256(propositionBytes)` matches an entry, and only once `HEIGHT > height added + 8,640` (72 h at 30 s) for that entry. One output at the standard miner fee script with value at most 10,000,000 nano may ride along (F-017).

New entries can be added via the Whitelist Update path (2-of-3 multisig, append-only: the old list must be an exact prefix of the new one, each new entry carries a height in `[HEIGHT - 10, HEIGHT]`, and the list may hold at most 64 entries). Entries cannot be removed or re-dated; migration to a new contract is required to shrink the whitelist. The per-entry delay gives notice of a new destination, not a veto, since freeze needs the same 2-of-3 quorum.

## Initial Whitelist (at launch)

The initial whitelist should be populated once the target DeFi contracts are deployed on YoloChain. Until then, the entries below are **placeholders** — the actual blake2b256 hashes will be computed from the deployed ErgoTree bytes.

| # | Destination | Rationale | Hash |
|---|-------------|-----------|------|
| 1 | Spectrum-style AMM Pool | Primary DEX liquidity for the native token. Seeding LP here enables price discovery and trading from day one. | TBD — compute from deployed pool contract ErgoTree |
| 2 | AEther Bridge Lock | Cross-chain liquidity for bridged assets. Enables the native token to be traded on Ergo mainnet and other chains. | TBD — compute from deployed bridge lock contract ErgoTree |
| 3 | Staging/Hold Contract | Simple P2S contract for temporarily holding funds before deployment to a specific pool. Useful when a pool isn't ready yet but funds need to leave the governance box. | TBD — deploy a simple hold contract and compute hash |

## How to Compute a Hash

Given a deployed contract's ErgoTree hex (from `/script/addressToTree/{addr}`):

```bash
python3 -c "
import hashlib
tree_hex = 'ERGOTREE_HEX_HERE'
tree_bytes = bytes.fromhex(tree_hex)
h = hashlib.blake2b(tree_bytes, digest_size=32)
print(h.hexdigest())
"
```

Or in Rust (used in the test suite):

```rust
use blake2::{digest::consts::U32, Blake2b, Digest};
type Blake2b256 = Blake2b<U32>;

fn proposition_hash(tree: &ErgoTree) -> Vec<u8> {
    let bytes = tree.sigma_serialize_bytes().expect("serialize tree");
    let mut h = Blake2b256::new();
    h.update(&bytes);
    h.finalize().to_vec()
}
```

## Adding Entries Post-Launch

To add a new destination via the Whitelist Update path:

1. Deploy the new destination contract on-chain
2. Compute its `blake2b256(propositionBytes)` hash
3. Construct a transaction with:
   - Input: current LP Fund governance box
   - Output: same box with R4 = the old list followed by `(new hash, h)`, where `h` is between the current height minus 10 and the current height
   - R5, R6, R7 preserved, value preserved, NFT at tokens(0) preserved, creation height at least the current height minus 10, at most 64 entries in all
4. Sign with 2-of-3 multisig
5. The new entry can be paid from height `h + 8,641`; older entries stay usable throughout.

## Deployment Notes

The LP Fund governance box must be initialized with:
- `R4`: the initial whitelist, `Coll[(Coll[Byte], Long)]`. An entry with height 0 is usable from block 8,641; the script does not check genesis heights, so a negative height would make an entry usable at once.
- `R5`: `0L` (normal state)
- `R6`: `0L` (no migration pending)
- `R7`: 32 zero bytes, `Coll[Byte]` (no migration target). This reserves the room a migration approval needs, so a full list can still be migrated. It is never a live target: migration execute needs `R5 = 2`.
- value: at least the 100 YOLO fund floor (`fundFloor`), so a storage rent spender can never take the box, NFT included; the NFT id must never be put in a rent builder token whitelist
- `tokens(0)`: the LP fund NFT, minted in one unit, whose id is compiled into both `lp_fund.es` and `lp_accumulation.es` (placeholder `LP_NFT_ID_PLACEHOLDER`)

The contract caps the list at 64 entries. Each entry costs 34 to 37 bytes (1 length byte, the 32-byte hash, 1 to 4 bytes of height). Measured with the v2.2 tree (1,031 bytes), a value of 1e16 nano and entry heights near 2,000,000: a 64-entry box is 3,520 bytes and its migration-approval successor 3,523 bytes. About 80 entries would fit physically under the 4,096-byte limit (79 to 86, depending on entry heights; about 83 with the 942-byte v2.0 tree), so the cap leaves room for value and height growth.

The accumulation contract no longer embeds the LP Fund script hash. Since v2.0 it pays whichever box holds the LP fund NFT (F-001, F-008), so changing `lp_fund.es` no longer changes the accumulation tree or genesis R5. (The v1 constant was `cfc1ed1d72ef1cb680a08f4b3b28bb2f9c585f26b11a785b0e16b165fe020569`.)

Review NV-4 asked for each launch entry to be reviewed. Listing `lp_accumulation.es` itself is pointless under v2.0: a box there can only be merged or forwarded back into the NFT holder.
