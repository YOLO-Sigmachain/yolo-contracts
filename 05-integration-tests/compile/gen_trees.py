#!/usr/bin/env python3
"""Compile the five pre-genesis contracts offline into the ErgoTree hex the tests load.

  python3 gen_trees.py                 write <crate>/trees/<name>.hex for all five
  python3 gen_trees.py --check         compile and compare with those files, exit 1 on any difference
  python3 gen_trees.py --print NAME    also dump NAME's root expression to stderr (hoisting check)
  python3 gen_trees.py --ids EMISSION=<64 hex> TREASURY=<64 hex> LP=<64 hex>
                                       compile with real NFT ids instead of the test ids and print
                                       the trees and hashes only (writes nothing); an id not given
                                       keeps its test value

Placeholders substituted before compiling (the .es sources do not compile without them):
  EMISSION_NFT_ID_PLACEHOLDER  emission NFT id  (test value cc..cc, 32 bytes)
  TREASURY_NFT_ID_PLACEHOLDER  treasury NFT id  (test value aa..aa, 32 bytes)
  LP_NFT_ID_PLACEHOLDER        LP fund NFT id   (test value bb..bb, 32 bytes)
The test values must match emission_nft_id() / make_nft_token() / TREASURY_NFT / LP_NFT in the
tests. The three ids must be distinct (checked here) and must not equal the id of any box that
can ever be spent (not checkable here: consensus mints a token whose id equals INPUTS(0).id).

Network prefix: the two multisig scripts hold PK("9...") literals, which are mainnet-encoded
addresses; they are compiled with --network=mainnet. The prefix only governs address decoding
and does not change the tree bytes. When production keys are encoded with YOLO's own address
prefix, set NETWORK below to that prefix byte.

No script hash is embedded in any of the five any more: the accumulation scripts name their
destination by NFT id. Genesis R4 / R5 of the emission box are blake2b256 of the treasury and
LP accumulation trees, printed at the end.

Needs scala-cli and a JVM (sigma-state 6.0.7, cached). Talks to no node. Writes only the five
trees/*.hex files.
"""
import hashlib
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent

TEST_IDS = {
    "EMISSION_NFT_ID_PLACEHOLDER": "cc" * 32,
    "TREASURY_NFT_ID_PLACEHOLDER": "aa" * 32,
    "LP_NFT_ID_PLACEHOLDER": "bb" * 32,
}

# name -> (source, output hex file, network)
CONTRACTS = {
    "emission": ("01-emission-tests/emission.es", "01-emission-tests/trees/emission.hex", "testnet"),
    "treasury_accumulation": ("02-treasury-tests/treasury_accumulation.es",
                              "02-treasury-tests/trees/treasury_accumulation.hex", "testnet"),
    "treasury_governance": ("02-treasury-tests/treasury_governance.es",
                            "02-treasury-tests/trees/treasury_governance.hex", "mainnet"),
    "lp_accumulation": ("03-lp-fund-tests/lp_accumulation.es",
                        "03-lp-fund-tests/trees/lp_accumulation.hex", "testnet"),
    "lp_fund": ("03-lp-fund-tests/lp_fund.es", "03-lp-fund-tests/trees/lp_fund.hex", "mainnet"),
}


def compile_es(name, subs, show=False):
    src, _, network = CONTRACTS[name]
    args = ["scala-cli", "--power", "run", "--server=false", "--offline", "Compile.scala", "--",
            f"--network={network}"] + (["--print"] if show else []) + [str(ROOT / src)] + \
           [f"{k}={v}" for k, v in subs.items()]
    out = subprocess.run(args, cwd=HERE, capture_output=True, text=True)
    if out.returncode != 0:
        sys.exit(f"compile failed for {src}:\n{out.stderr}")
    if show:
        for line in out.stderr.splitlines():
            if line.startswith("BlockValue") or line.startswith("SigmaAnd") or line.startswith("BoolToSigmaProp"):
                print(line, file=sys.stderr)
    # A placeholder left in the source fails to compile (fromBase16 of a non-hex string).
    return out.stdout.strip().splitlines()[-1]


def blake2b256(hex_str):
    return hashlib.blake2b(bytes.fromhex(hex_str), digest_size=32).hexdigest()


def main():
    argv = sys.argv[1:]
    check = "--check" in argv
    show = argv[argv.index("--print") + 1] if "--print" in argv else None
    subs = dict(TEST_IDS)
    real = "--ids" in argv
    if real:
        for kv in argv[argv.index("--ids") + 1:]:
            if kv.startswith("--"):
                break
            k, v = kv.split("=", 1)
            key = {"EMISSION": "EMISSION_NFT_ID_PLACEHOLDER", "TREASURY": "TREASURY_NFT_ID_PLACEHOLDER",
                   "LP": "LP_NFT_ID_PLACEHOLDER"}[k]
            if len(v) != 64:
                sys.exit(f"{k} id must be 64 hex characters")
            subs[key] = v.lower()
    # Distinctness across all three ids (test ids included).
    if len(set(subs.values())) != len(subs):
        sys.exit("emission, treasury and LP NFT ids must all differ")

    trees = {n: compile_es(n, subs, show == n) for n in CONTRACTS}
    bad = 0
    for n, t in trees.items():
        path = ROOT / CONTRACTS[n][1]
        if real:
            print(f"{n}: {t}")
        elif check:
            have = path.read_text().strip() if path.exists() else ""
            same = have == t
            bad += not same
            print(f"{'OK  ' if same else 'DIFF'} {n} ({len(t) // 2} bytes) {path.relative_to(ROOT)}")
        else:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(t + "\n")
            print(f"wrote {path.relative_to(ROOT)} ({len(t) // 2} bytes)")
    print()
    for n, t in trees.items():
        print(f"blake2b256({n}) = {blake2b256(t)}")
    print(f"genesis emission R4 = {blake2b256(trees['treasury_accumulation'])}")
    print(f"genesis emission R5 = {blake2b256(trees['lp_accumulation'])}")
    if trees["treasury_accumulation"] == trees["lp_accumulation"]:
        print("ERROR: the two accumulation trees are identical (NFT ids equal?)")
        bad += 1
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
