#!/usr/bin/env python3
"""Offline tests for fundsign.py. Standard library only; no network: every node call goes to MockNode.

Run from this directory:
  python3 -I test_fundsign.py -v
  python3 -I -m unittest discover -v -p test_fundsign.py
(`python3 -I -m unittest test_fundsign.py` cannot import the file, since -I leaves the current
directory off sys.path.)
"""
import argparse, contextlib, copy, hashlib, importlib.util, io, json, os, re, stat, sys, tempfile, unittest
import urllib.error

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
_spec = importlib.util.spec_from_file_location("fundsign", os.path.join(HERE, "fundsign.py"))
fs = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(fs)


def read_tree(rel):
    with open(os.path.join(ROOT, rel)) as f:
        return f.read().strip()


# Compiled launch contracts (NFT placeholders substituted with aa..aa / bb..bb by the test suites).
TT = read_tree("02-treasury-tests/trees/treasury_governance.hex")
LT = read_tree("03-lp-fund-tests/trees/lp_fund.hex")
TACC = read_tree("02-treasury-tests/trees/treasury_accumulation.hex")
LACC = read_tree("03-lp-fund-tests/trees/lp_accumulation.hex")
TNFT, LNFT = "aa" * 32, "bb" * 32
K = fs.signer_keys(TT)

PROP = "0008cd" + K[0]          # proposer's wallet address
COS = "0008cd" + K[1]           # cosigner's wallet address
RCPT = "0008cd02" + "dd" * 32
EVIL = "0008cd02" + "ee" * 32
DEST2 = "0008cd03" + "cc" * 32
UNKNOWN = "0008cd02" + "99" * 32   # a script the mock node cannot name
P2S = "100104c801d17300"           # not a P2PK script
ADDR = {"9hPRPSR": PROP, "9hCSGNR": COS, "9hRCPT": RCPT, "9hEVXX": EVIL, "9hDEST2": DEST2}
REV = {v: k for k, v in ADDR.items()}

N = fs.NANO
FEE = 2_000_000
D, W = fs.TIMELOCK, fs.WINDOW


def bid(name):
    return hashlib.sha256(name.encode()).hexdigest()


def bx(name, value, tree, assets=None, regs=None, ch=100):
    return {"boxId": bid(name), "value": value, "ergoTree": tree, "assets": assets or [],
            "additionalRegisters": regs or {}, "creationHeight": ch}


def treg(comm=b"\x00" * 32, ah=0, rec=b"", amt=0, flag=0):
    return {"R4": fs.enc_bytes(comm), "R5": fs.enc_long(ah), "R6": fs.enc_bytes(rec),
            "R7": fs.enc_long(amt), "R8": fs.enc_long(flag)}


def lreg(wl=(), flag=0, mh=0, mt=b"\x00" * 32):
    return {"R4": fs.enc_whitelist(list(wl)), "R5": fs.enc_long(flag), "R6": fs.enc_long(mh), "R7": fs.enc_bytes(mt)}


def tfund(regs, value=1000 * N, extra_assets=()):
    return bx("T", value, TT, [{"tokenId": TNFT, "amount": 1}] + list(extra_assets), regs)


def lfund(regs, value=1000 * N):
    return bx("L", value, LT, [{"tokenId": LNFT, "amount": 1}], regs)


def nft_of(fund_box):
    return [dict(fund_box["assets"][0])]


def out(value, tree, ch, assets=None, regs=None):
    return fs.out_box(value, tree, ch, assets, regs)


def fee_out(ch, value=FEE):
    return out(value, fs.MINER_FEE_TREE, ch)


def mktx(inputs, outputs):
    return {"inputs": [{"boxId": b["boxId"], "extension": {}} for b in inputs], "dataInputs": [], "outputs": outputs}


FB = bx("FB", 5 * N, PROP)          # proposer's fee box
COMM = fs.b2b(bytes.fromhex(RCPT) + (50 * N).to_bytes(8, "big"))
HR, HE, HD = (fs.b2b(bytes.fromhex(t)) for t in (RCPT, EVIL, DEST2))
MIG = fs.b2b(b"some target script")


def kept(fund_box, new_regs, tip, acc=(), fee_box=FB, ch=None):
    """A kept-path transaction: the fund box keeps its value (plus accumulation), the proposer pays the fee."""
    ch = tip if ch is None else ch
    o0 = out(fund_box["value"] + sum(b["value"] for b in acc), fund_box["ergoTree"], ch, nft_of(fund_box), new_regs)
    outs = [o0] + ([out(fee_box["value"] - FEE, fee_box["ergoTree"], ch)] if fee_box["value"] > FEE else []) + [fee_out(ch)]
    return mktx([fund_box, fee_box] + list(acc), outs), [fund_box, fee_box] + list(acc)


class MockNode(fs.Node):
    """Answers the node API paths fundsign.py uses, from an in-memory set of unspent boxes."""

    def __init__(self, boxes, tip, wallet_key=None, wallet_boxes=None, fail_sign=False, on_sign=None):
        self.url, self.key = "http://127.0.0.1:9", ""
        self.boxes = {b["boxId"]: copy.deepcopy(b) for b in boxes}
        self.tip, self.wallet_key, self.fail_sign, self.on_sign = tip, wallet_key, fail_sign, on_sign
        self.wallet_boxes = [b["boxId"] for b in (wallet_boxes if wallet_boxes is not None else [FB])]
        self.calls = []
        emission = bx("emission", 1, "0008cd" + "02" * 33, [{"tokenId": "ee" * 32, "amount": 1}],
                      {"R4": fs.enc_bytes(fs.b2b(bytes.fromhex(TACC))), "R5": fs.enc_bytes(fs.b2b(bytes.fromhex(LACC)))})
        self.genesis = [emission, bx("gT", 1000 * N, TT, [{"tokenId": TNFT, "amount": 1}], treg()),
                        bx("gL", 1000 * N, LT, [{"tokenId": LNFT, "amount": 1}], lreg())]

    def keys_of(self, tree):
        if tree in (TT, LT):
            return fs.signer_keys(tree)
        return [tree[6:]] if fs.is_p2pk(tree) else []

    def call(self, path, data=None):
        self.calls.append(path)
        tail = path.rsplit("/", 1)[-1]
        if path == "/utxo/genesis":
            return copy.deepcopy(self.genesis)
        if path == "/info":
            return {"fullHeight": self.tip}
        if path.startswith("/utxo/byId/"):
            if tail in self.boxes:
                return copy.deepcopy(self.boxes[tail])
            raise fs.Fail("mock: box not found")
        if path.startswith("/utxo/byIdBinary/"):
            if tail in self.boxes:
                return {"bytes": "ff" + tail}
            raise fs.Fail("mock: box not found")
        if path.startswith("/script/addressToTree/"):
            if tail in ADDR:
                return {"tree": ADDR[tail]}
            raise fs.Fail("mock: bad address")
        if path.startswith("/utils/ergoTreeToAddress/"):
            if tail in REV:
                return {"address": REV[tail]}
            raise fs.Fail("mock: cannot convert")
        if path.startswith("/blockchain/box/unspent/byTokenId/"):
            nft = tail.split("?")[0]
            return {"items": [{"boxId": b["boxId"]} for b in self.boxes.values()
                              if b["assets"] and b["assets"][0]["tokenId"] == nft]}
        if path.startswith("/blocks/at/"):
            return [bid("block")]
        if path == f"/blocks/{bid('block')}/transactions":
            return {"transactions": [{"outputs": [{"ergoTree": TACC}, {"ergoTree": LACC}]}]}
        if path.startswith("/blockchain/box/unspent/byErgoTree"):
            offset = int(re.search(r"offset=(\d+)", path).group(1))
            items = [copy.deepcopy(b) for b in self.boxes.values() if b["ergoTree"] == data]
            return {"items": items[offset:offset + 100]}
        if path.startswith("/wallet/boxes/unspent"):
            return [{"box": copy.deepcopy(self.boxes[i])} for i in self.wallet_boxes if i in self.boxes]
        if path == "/wallet/generateCommitments":
            pub, sec = {}, {}
            for n, i in enumerate(data["tx"]["inputs"]):
                if self.wallet_key in self.keys_of(self.boxes[i["boxId"]]["ergoTree"]):
                    pub[str(n)] = [{"hint": "cmtReal", "pubkey": {"op": -51, "h": self.wallet_key}, "a": "02" + "aa" * 32}]
                    sec[str(n)] = [{"hint": "cmtWithSecret", "pubkey": {"op": -51, "h": self.wallet_key}, "secret": "00"}]
            return {"publicHints": pub, "secretHints": sec}
        if path == "/wallet/extractHints":
            return {"publicHints": {"0": [{"hint": "proofReal", "pubkey": {"op": -51, "h": K[1]}}]}}
        if path == "/wallet/transaction/sign":
            if self.on_sign:
                self.on_sign(data)
            if self.fail_sign:
                raise fs.Fail("node refused /wallet/transaction/sign: HTTP 400")
            return dict(copy.deepcopy(data["tx"]), id=bid("signed"))
        if path in ("/transactions/check", "/transactions"):
            return bid("signed")
        raise fs.Fail("mock: no route " + path)


def chain_for(boxes, tip, **kw):
    return fs.Chain(MockNode(boxes, tip, **kw))


def describe(boxes, tip, tx, expect=None, **kw):
    return fs.describe(chain_for(boxes, tip, **kw), {"tx": tx}, expect)


def build_args(fund, action, **kw):
    a = argparse.Namespace(fund=fund, action=action, fee_nano=FEE, fee_box=None, max_boxes=50, min_box_nano=N,
                           to=None, amount=None, dest=[])
    for k, v in kw.items():
        setattr(a, k, v)
    return a


B58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"


def b58decode(s):
    n = 0
    for c in s:
        n = n * 58 + B58.index(c)
    raw = n.to_bytes((n.bit_length() + 7) // 8, "big")
    return b"\x00" * (len(s) - len(s.lstrip("1"))) + raw


# ---------------------------------------------------------------- encoders and helpers

class Encoding(unittest.TestCase):
    def test_long_round_trip(self):
        for n in [0, 1, -1, 63, 64, -64, -65, D, 2**31, -2**31, 10**18, 2**63 - 1, -2**63]:
            self.assertEqual(fs.dec_long(fs.enc_long(n)), n, n)
        self.assertEqual(fs.enc_long(0), "0500")
        self.assertEqual(fs.enc_long(1), "0502")
        self.assertEqual(fs.enc_long(-1), "0501")
        with self.assertRaises(fs.Fail):
            fs.dec_long("0e00")
        with self.assertRaises(fs.Fail):
            fs.dec_long("050200")

    def test_bytes_round_trip(self):
        for b in [b"", b"\x01", bytes(range(256)) * 2]:
            self.assertEqual(fs.dec_bytes(fs.enc_bytes(b)), b)
        self.assertEqual(fs.enc_bytes(b""), "0e00")
        with self.assertRaises(fs.Fail):
            fs.dec_bytes("0e0201")
        with self.assertRaises(fs.Fail):
            fs.dec_bytes("0500")

    def test_whitelist_round_trip(self):
        w = [(b"\x01" * 32, 0), (b"\x02" * 32, 123456), (b"\x03" * 32, -5), (b"\x04" * 32, 2**63 - 1)]
        self.assertEqual(fs.dec_whitelist(fs.enc_whitelist(w)), w)
        self.assertEqual(fs.enc_whitelist([]), "0c4d0e00")
        self.assertEqual(fs.dec_whitelist("0c4d0e00"), [])
        with self.assertRaises(fs.Fail):
            fs.dec_whitelist(fs.enc_whitelist(w) + "00")

    def test_to_nano(self):
        self.assertEqual(fs.to_nano("0.002"), 2_000_000)
        self.assertEqual(fs.to_nano("1"), N)
        self.assertEqual(fs.to_nano("1e3"), 1000 * N)
        self.assertEqual(fs.to_nano("0.000000001"), 1)
        for bad in ["-1", "0", "0.0000000001", "nan", "abc", ""]:
            with self.assertRaises(fs.Fail, msg=bad):
                fs.to_nano(bad)

    def test_coins(self):
        self.assertEqual(fs.coins(0), "0 YOLO")
        self.assertEqual(fs.coins(N), "1 YOLO")
        self.assertEqual(fs.coins(1), "0.000000001 YOLO")
        self.assertEqual(fs.coins(1234 * N + 567890123), "1,234.567890123 YOLO")
        self.assertEqual(fs.coins(-1), "-0.000000001 YOLO")          # F8
        self.assertEqual(fs.coins(-(N + 5)), "-1.000000005 YOLO")

    def test_signer_keys_on_compiled_trees(self):
        kt, kl = fs.signer_keys(TT), fs.signer_keys(LT)
        self.assertEqual(kt, kl)
        self.assertEqual(len(set(kt)), 3)
        with open(os.path.join(ROOT, "02-treasury-tests/treasury_governance.es")) as f:
            addrs = re.findall(r'PK\("(\w+)"\)', f.read())
        from_source = [b58decode(a)[1:34].hex() for a in addrs]
        self.assertEqual(kt, from_source)
        for tree in (TT, LT):
            self.assertTrue(all(tree.find("08cd" + k) % 2 == 0 for k in kt))

    def test_signer_keys_ignores_odd_offsets(self):          # F15
        fake = "1003" + "".join("08cd" + k for k in K) + "0e22" + "0" + "08cd" + "03" + "ab" * 32 + "0"
        self.assertEqual(fs.signer_keys(fake), K)

    def test_checks(self):
        self.assertEqual(fs.check_id("ab" * 32), "ab" * 32)
        for bad in ["AB" * 32, "ab" * 31, "../../wallet/lock", None, "ab" * 32 + "\n"]:
            with self.assertRaises(fs.Fail):
                fs.check_id(bad)
        for bad in ["", "abc", "0A", "zz", 5]:
            with self.assertRaises(fs.Fail):
                fs.check_hex(bad, "x")
        fs.check_address("9hRCPT")
        for bad in ["9h/../x", "9h0O", "9hIl", "", "9h?x=1"]:
            with self.assertRaises(fs.Fail):
                fs.check_address(bad)
        self.assertTrue(fs.is_p2pk(PROP))
        self.assertFalse(fs.is_p2pk(P2S))
        self.assertFalse(fs.is_p2pk(PROP + "00"))


# ---------------------------------------------------------------- describe: accepted kinds

class DescribeAccepts(unittest.TestCase):
    def ok(self, boxes, tip, tx, kind, expect=None):
        lines, info = describe(boxes, tip, tx, expect)
        self.assertEqual(info["kind"], kind, "\n".join(lines))
        return lines, info

    # treasury
    def test_treasury_forward(self):
        T = tfund(treg())
        acc = [bx("A1", 3 * N, TACC), bx("A2", 4 * N, TACC)]
        tx, boxes = kept(T, treg(), 5000, acc)
        _, info = self.ok(boxes, 5000, tx, "forward")
        self.assertEqual(info["n_signed"], 2)

    def test_treasury_approve(self):
        T = tfund(treg())
        tx, boxes = kept(T, treg(COMM, 5000, bytes.fromhex(RCPT), 50 * N, 0), 5000)
        lines, info = self.ok(boxes, 5000, tx, "approve")
        self.assertEqual(info["deadline"], 5010)
        self.assertIn("9hRCPT", "\n".join(lines))

    def test_treasury_cancel(self):
        T = tfund(treg(COMM, 1000, bytes.fromhex(RCPT), 50 * N, 0))
        tx, boxes = kept(T, treg(COMM, 0, bytes.fromhex(RCPT), 50 * N, 0), 5000)
        lines, _ = self.ok(boxes, 5000, tx, "cancel")
        self.assertIn("cancels the recorded payment of 50 YOLO to 9hRCPT", "\n".join(lines))

    def test_treasury_execute(self):
        tip = 1000 + D + 1
        T = tfund(treg(COMM, 1000, bytes.fromhex(RCPT), 50 * N, 0))
        o0 = out(T["value"] - 50 * N - FEE, TT, tip, nft_of(T), treg(COMM, 0, bytes.fromhex(RCPT), 50 * N, 0))
        tx = mktx([T], [o0, out(50 * N, RCPT, tip), fee_out(tip)])
        lines, info = self.ok([T], tip, tx, "execute")
        self.assertEqual(info["n_signed"], 1)
        self.assertIn("PAYS OUT          50 YOLO to 9hRCPT", "\n".join(lines))

    def test_treasury_freeze_and_unfreeze(self):
        T = tfund(treg())
        tx, boxes = kept(T, treg(flag=1), 5000)
        self.ok(boxes, 5000, tx, "freeze")
        T = tfund(treg(flag=1))
        tx, boxes = kept(T, treg(flag=0), 5000)
        lines, _ = self.ok(boxes, 5000, tx, "unfreeze")
        self.assertNotIn("recorded item remains", "\n".join(lines))

    # liquidity fund
    def test_lp_forward(self):
        L = lfund(lreg([(HR, 1000)]))
        acc = [bx("B1", 3 * N, LACC), bx("B2", 4 * N, LACC)]
        tx, boxes = kept(L, lreg([(HR, 1000)]), 5000, acc)
        self.ok(boxes, 5000, tx, "forward")

    def test_lp_whitelist_add(self):
        L = lfund(lreg([(HR, 1000)]))
        tx, boxes = kept(L, lreg([(HR, 1000), (HD, 5000)]), 5000)
        lines, info = self.ok(boxes, 5000, tx, "whitelist-add")
        self.assertFalse(info["dest_ok"])           # no --expect-dest given
        self.assertEqual(info["deadline"], 5010)
        lines, info = self.ok(boxes, 5000, tx, "whitelist-add", expect=["9hDEST2"])
        self.assertTrue(info["dest_ok"])
        self.assertIn(f"adds destination 9hDEST2 (hash {HD.hex()})", "\n".join(lines))

    def test_lp_spend(self):
        tip = 1000 + D + 1
        L = lfund(lreg([(HR, 1000)]))
        o0 = out(L["value"] - 100 * N - FEE, LT, tip, nft_of(L), lreg([(HR, 1000)]))
        tx = mktx([L], [o0, out(100 * N, RCPT, tip), fee_out(tip)])
        self.ok([L], tip, tx, "spend")

    def test_lp_freeze_and_unfreeze(self):
        L = lfund(lreg([(HR, 1000)]))
        tx, boxes = kept(L, lreg([(HR, 1000)], flag=1), 5000)
        self.ok(boxes, 5000, tx, "freeze")
        L = lfund(lreg([(HR, 1000)], flag=1))
        tx, boxes = kept(L, lreg([(HR, 1000)], flag=0), 5000)
        self.ok(boxes, 5000, tx, "unfreeze")

    def test_lp_freeze_during_migration_approval(self):
        L = lfund(lreg([(HR, 1000)], flag=2, mh=5000, mt=MIG))
        tx, boxes = kept(L, lreg([(HR, 1000)], flag=1, mh=5000, mt=MIG), 5005)
        self.ok(boxes, 5005, tx, "freeze")
        tx, boxes = kept(L, lreg([(HR, 1000)], flag=1, mh=0, mt=MIG), 5005)
        with self.assertRaisesRegex(fs.Fail, "does not sign migrations"):
            describe(boxes, 5005, tx)

    def test_default_min_box_is_below_every_share(self):
        a = fs.parser().parse_args(["build", "treasury", "forward"])
        self.assertLessEqual(fs.to_nano(a.min_box), 30_000_000)   # terminal LP share, the smallest

    def test_lp_cancel_migration(self):           # F13
        L = lfund(lreg([(HR, 1000)], flag=2, mh=5000, mt=MIG))
        tx, boxes = kept(L, lreg([(HR, 1000)], flag=0, mh=0, mt=MIG), 5005)
        lines, _ = self.ok(boxes, 5005, tx, "cancel-migration")
        self.assertIn("cancels a MIGRATION approval recorded at height 5000", "\n".join(lines))

    def test_output_creation_height_tip_plus_one_kept(self):   # F4: this check is unchanged
        T = tfund(treg())
        tx, boxes = kept(T, treg(flag=1), 5000, ch=5001)
        self.ok(boxes, 5000, tx, "freeze")
        tx, boxes = kept(T, treg(flag=1), 5000, ch=5002)
        with self.assertRaisesRegex(fs.Fail, "this node is behind"):
            describe(boxes, 5000, tx)


# ---------------------------------------------------------------- describe: refusals

class DescribeRefuses(unittest.TestCase):
    def refused(self, boxes, tip, tx, pattern, expect=None):
        with self.assertRaisesRegex(fs.Fail, pattern):
            describe(boxes, tip, tx, expect)

    def execute_tx(self, tip, pay_regs=None, o0_regs_extra=None):
        T = tfund(treg(COMM, 1000, bytes.fromhex(RCPT), 50 * N, 0))
        regs = dict(treg(COMM, 0, bytes.fromhex(RCPT), 50 * N, 0), **(o0_regs_extra or {}))
        o0 = out(T["value"] - 50 * N - FEE, TT, tip, nft_of(T), regs)
        return mktx([T], [o0, out(50 * N, RCPT, tip, None, pay_regs), fee_out(tip)]), [T]

    def test_f1_registers_on_payment_or_change(self):
        tip = 1000 + D + 1
        tx, boxes = self.execute_tx(tip, pay_regs={"R4": fs.enc_bytes(bytes.fromhex(EVIL))})
        self.refused(boxes, tip, tx, "output 1 carries registers")
        T = tfund(treg())
        tx, boxes = kept(T, treg(flag=1), 5000)
        tx["outputs"][1]["additionalRegisters"] = {"R4": "0e0102"}
        self.refused(boxes, 5000, tx, "output 1 carries registers")

    def test_f2_extra_registers(self):
        tip = 1000 + D + 1
        tx, boxes = self.execute_tx(tip, o0_regs_extra={"R9": fs.enc_bytes(b"\x99" * 20)})
        self.refused(boxes, tip, tx, "output 0 must carry exactly the registers R4, R5, R6, R7, R8")
        L = lfund(lreg([(HR, 1000)]))
        tx, boxes = kept(L, dict(lreg([(HR, 1000), (HE, 5000)]), R8=fs.enc_long(7)), 5000)
        self.refused(boxes, 5000, tx, "output 0 must carry exactly the registers R4, R5, R6, R7$")
        T = tfund(dict(treg(), R9="0500"))
        tx, boxes = kept(T, treg(flag=1), 5000)
        self.refused(boxes, 5000, tx, "the fund box must carry exactly")
        T = tfund({k: v for k, v in treg().items() if k != "R8"})
        tx, boxes = kept(T, treg(flag=1), 5000)
        self.refused(boxes, 5000, tx, "the fund box must carry exactly")

    def test_f3_execute_window(self):
        tx, boxes = self.execute_tx(1000 + D)
        self.refused(boxes, 1000 + D, tx, "the wait is not over")
        last = 1000 + D + W
        tx, boxes = self.execute_tx(last - 1)
        _, info = describe(boxes, last - 1, tx)
        self.assertEqual(info["deadline"], last)
        tx, boxes = self.execute_tx(last)
        self.refused(boxes, last, tx, "expired")

    def test_f4_lp_spend_maturity_edge(self):
        tip = 1000 + D
        L = lfund(lreg([(HR, 1000)]))
        o0 = out(L["value"] - 100 * N - FEE, LT, tip, nft_of(L), lreg([(HR, 1000)]))
        tx = mktx([L], [o0, out(100 * N, RCPT, tip), fee_out(tip)])
        self.refused([L], tip, tx, "not to a matured whitelist entry")

    def test_f4_recorded_heights_not_after_tip(self):
        T = tfund(treg())
        tx, boxes = kept(T, treg(COMM, 5001, bytes.fromhex(RCPT), 50 * N, 0), 5000)
        self.refused(boxes, 5000, tx, "approval height is in the future")
        L = lfund(lreg([(HR, 1000)]))
        tx, boxes = kept(L, lreg([(HR, 1000), (HD, 5001)]), 5000)
        self.refused(boxes, 5000, tx, "new whitelist entry is malformed")

    def test_f5_fee_input_must_be_p2pk(self):
        T = tfund(treg())
        tx, boxes = kept(T, treg(flag=1), 5000, fee_box=bx("FP2S", 5 * N, P2S))
        self.refused(boxes, 5000, tx, "not at a plain public key address")

    def test_f5_one_change_output(self):
        T = tfund(treg())
        tx, boxes = kept(T, treg(flag=1), 5000)
        ch = tx["outputs"][1]
        tx["outputs"][1:2] = [dict(ch, value=ch["value"] - N), dict(ch, value=N)]
        self.refused(boxes, 5000, tx, "more than one change output")

    def test_f8_output_value_typing(self):
        T = tfund(treg())
        for field, bad in [("value", True), ("value", 0), ("value", -1), ("value", "5"), ("value", 1.0),
                           ("creationHeight", -1), ("creationHeight", True), ("creationHeight", "5000")]:
            tx, boxes = kept(T, treg(flag=1), 5000)
            tx["outputs"][1][field] = bad
            with self.assertRaises(fs.Fail, msg=f"{field}={bad!r}"):
                describe(boxes, 5000, tx)

    def test_f8_negative_change_balanced(self):
        # a change of -1 balanced by one more nanocoin elsewhere is refused on its value
        T = tfund(treg())
        fbx = bx("FS", FEE - 1, PROP)
        o0 = out(T["value"], TT, 5000, nft_of(T), treg(flag=1))
        tx = mktx([T, fbx], [o0, out(-1, PROP, 5000), fee_out(5000)])
        self.refused([T, fbx], 5000, tx, "output 1 value is not a whole number above 0")

    def test_f9_extra_tokens_on_fund_box(self):
        T = tfund(treg(), extra_assets=[{"tokenId": "cd" * 32, "amount": 5}])
        tx, boxes = kept(T, treg(flag=1), 5000)
        self.refused(boxes, 5000, tx, "tokens other than the fund NFT")

    def test_f10_ids_and_hex_checked_before_node_calls(self):
        T = tfund(treg())
        cases = []
        tx, boxes = kept(T, treg(flag=1), 5000)
        tx["inputs"][1]["boxId"] = "../../wallet/lock"
        cases.append(tx)
        tx, _ = kept(T, treg(flag=1), 5000)
        tx["inputs"][1]["boxId"] = FB["boxId"].upper()
        cases.append(tx)
        tx, _ = kept(T, treg(flag=1), 5000)
        tx["outputs"][1]["ergoTree"] = PROP.upper()
        cases.append(tx)
        tx, _ = kept(T, treg(flag=1), 5000)
        tx["outputs"][0]["additionalRegisters"]["R8"] = "050"
        cases.append(tx)
        tx, _ = kept(T, treg(flag=1), 5000)
        tx["dataInputs"] = [{"boxId": "x/../y"}]
        cases.append(tx)
        for tx in cases:
            node = MockNode(boxes, 5000)
            chain = fs.Chain(node)
            node.calls.clear()
            with self.assertRaises(fs.Fail):
                fs.describe(chain, {"tx": tx})
            self.assertEqual(node.calls, [], tx)

    def test_f10_node_methods_validate_path_parts(self):
        node = MockNode([], 5000)
        for f, arg in [(node.box, "../x"), (node.raw, "AB" * 32), (node.tree_of, "9h/../wallet")]:
            with self.assertRaises(fs.Fail):
                f(arg)
        with self.assertRaises(fs.Fail):
            node.address_of("zz/..", strict=True)
        self.assertEqual(node.address_of("zz/.."), "script zz/.....")
        self.assertEqual(node.calls, [])

    def test_f13_treasury_cancel_flags(self):
        # cancel of a migration approval: flag 2 -> 0, labelled as such
        T = tfund(treg(MIG, 900, b"", 0, 2))
        tx, boxes = kept(T, treg(MIG, 0, b"", 0, 0), 905)
        lines, info = describe(boxes, 905, tx)
        self.assertEqual(info["kind"], "cancel")
        self.assertIn("cancels a MIGRATION approval", "\n".join(lines))
        tx, boxes = kept(T, treg(MIG, 0, b"", 0, 2), 905)
        self.refused(boxes, 905, tx, "a cancel must set the flag")
        # cancel while frozen keeps the flag
        T = tfund(treg(COMM, 1000, bytes.fromhex(RCPT), 50 * N, 1))
        tx, boxes = kept(T, treg(COMM, 0, bytes.fromhex(RCPT), 50 * N, 1), 5000)
        self.assertEqual(describe(boxes, 5000, tx)[1]["kind"], "cancel")
        tx, boxes = kept(T, treg(COMM, 0, bytes.fromhex(RCPT), 50 * N, 0), 5000)
        self.refused(boxes, 5000, tx, "a cancel must set the flag")
        T = tfund(treg(COMM, 1000, bytes.fromhex(RCPT), 50 * N, 0))
        tx, boxes = kept(T, treg(COMM, 0, bytes.fromhex(RCPT), 50 * N, 1), 5000)
        self.refused(boxes, 5000, tx, "a cancel must set the flag")

    def test_f13_frozen_paths(self):
        T = tfund(treg(flag=1))
        tx, boxes = kept(T, treg(flag=1), 5000, [bx("A1", 3 * N, TACC)])
        self.refused(boxes, 5000, tx, "treasury is frozen")
        L = lfund(lreg([(HR, 1000)], flag=1))
        tx, boxes = kept(L, lreg([(HR, 1000)], flag=1), 5000, [bx("B1", 3 * N, LACC)])
        self.refused(boxes, 5000, tx, "liquidity fund is frozen")
        tip = 1000 + D + 1
        o0 = out(L["value"] - 100 * N - FEE, LT, tip, nft_of(L), lreg([(HR, 1000)], flag=1))
        self.refused([L], tip, mktx([L], [o0, out(100 * N, RCPT, tip), fee_out(tip)]), "frozen; a spend")
        tx, boxes = kept(L, lreg([(HR, 1000), (HD, 5000)], flag=1), 5000)
        self.refused(boxes, 5000, tx, "match no liquidity fund path")

    def test_f13_unfreeze_note_and_approve_address(self):
        T = tfund(treg(MIG, 900, b"", 0, 1))
        tx, boxes = kept(T, treg(MIG, 900, b"", 0, 0), 905)
        lines, info = describe(boxes, 905, tx)
        self.assertEqual(info["kind"], "unfreeze")
        self.assertIn("a recorded item remains", "\n".join(lines))
        comm = fs.b2b(bytes.fromhex(UNKNOWN) + (50 * N).to_bytes(8, "big"))
        T = tfund(treg())
        tx, boxes = kept(T, treg(comm, 5000, bytes.fromhex(UNKNOWN), 50 * N, 0), 5000)
        self.refused(boxes, 5000, tx, "cannot turn script")

    def test_f13_lp_cancel_migration_must_keep_the_rest(self):
        L = lfund(lreg([(HR, 1000)], flag=2, mh=5000, mt=MIG))
        tx, boxes = kept(L, lreg([(HR, 1000)], flag=0, mh=0, mt=b"\x01" * 32), 5005)
        self.refused(boxes, 5005, tx, "migration fields")
        tx, boxes = kept(L, lreg([(HR, 1000), (HD, 5005)], flag=0, mh=0, mt=MIG), 5005)
        self.refused(boxes, 5005, tx, "migration fields")
        tx, boxes = kept(L, lreg([(HR, 1000)], flag=0, mh=5000, mt=MIG), 5005)
        self.refused(boxes, 5005, tx, "migration fields")

    def test_migration_approval(self):
        T = tfund(treg())
        tx, boxes = kept(T, treg(MIG, 5000, b"", 0, 2), 5000)
        self.refused(boxes, 5000, tx, "migration approval")
        L = lfund(lreg([(HR, 1000)]))
        tx, boxes = kept(L, lreg([(HR, 1000)], flag=2, mh=5000, mt=MIG), 5000)
        self.refused(boxes, 5000, tx, "migration")
        T = tfund(treg())
        tx, boxes = kept(T, treg(), 5000)
        tx["outputs"][0]["ergoTree"] = LT
        self.refused(boxes, 5000, tx, "not at the fund script")

    def test_payment_on_non_payment_path(self):
        T = tfund(treg())
        o0 = out(T["value"] - 10 * N - FEE, TT, 5000, nft_of(T), treg(flag=1))
        self.refused([T], 5000, mktx([T], [o0, out(10 * N, EVIL, 5000), fee_out(5000)]), "does not allow one")
        L = lfund(lreg([(HR, 1000)]))
        o0 = out(L["value"] - 10 * N - FEE, LT, 5000, nft_of(L), lreg([(HR, 1000)], flag=1))
        self.refused([L], 5000, mktx([L], [o0, out(10 * N, RCPT, 5000), fee_out(5000)]), "does not allow one")

    def test_value_leaving_fund_on_kept_path(self):
        T = tfund(treg())
        o0 = out(T["value"] - FEE, TT, 5000, nft_of(T), treg(flag=1))
        self.refused([T], 5000, mktx([T], [o0, fee_out(5000)]), "value leaves the fund")

    def test_second_fee_output(self):
        T = tfund(treg())
        o0 = out(T["value"], TT, 5000, nft_of(T), treg(flag=1))
        tx = mktx([T, FB], [o0, out(FB["value"] - 2 * FEE, PROP, 5000), fee_out(5000), fee_out(5000)])
        self.refused([T, FB], 5000, tx, "exactly one miner fee output")

    def test_fee_above_cap(self):
        T = tfund(treg())
        o0 = out(T["value"], TT, 5000, nft_of(T), treg(flag=1))
        cap = fs.FEE_CAP + 1
        tx = mktx([T, FB], [o0, out(FB["value"] - cap, PROP, 5000), fee_out(5000, cap)])
        self.refused([T, FB], 5000, tx, "above")

    def test_tokens_on_other_inputs_and_outputs(self):
        T = tfund(treg())
        ftok = bx("FT", 5 * N, PROP, [{"tokenId": "cd" * 32, "amount": 1}])
        tx, boxes = kept(T, treg(flag=1), 5000, fee_box=ftok)
        self.refused(boxes, 5000, tx, "holds tokens")
        tx, boxes = kept(T, treg(flag=1), 5000)
        tx["outputs"][1]["assets"] = [{"tokenId": TNFT, "amount": 1}]
        self.refused(boxes, 5000, tx, "output 1 holds tokens")

    def test_accumulation_boxes_not_last(self):
        T = tfund(treg())
        A1 = bx("A1", 3 * N, TACC)
        o0 = out(T["value"] + A1["value"], TT, 5000, nft_of(T), treg())
        tx = mktx([T, A1, FB], [o0, out(FB["value"] - FEE, PROP, 5000), fee_out(5000)])
        self.refused([T, A1, FB], 5000, tx, "must come after")

    def test_data_inputs_and_extension(self):
        T = tfund(treg())
        tx, boxes = kept(T, treg(flag=1), 5000)
        tx["dataInputs"] = [{"boxId": FB["boxId"]}]
        self.refused(boxes, 5000, tx, "data inputs")
        tx, boxes = kept(T, treg(flag=1), 5000)
        tx["inputs"][0]["extension"] = {"0": "0500"}
        self.refused(boxes, 5000, tx, "context variables")


# ---------------------------------------------------------------- build

class Build(unittest.TestCase):
    def build_ok(self, boxes, tip, a, kind, wallet_boxes=None):
        chain = chain_for(boxes, tip, wallet_boxes=wallet_boxes)
        p = fs.build(chain, a)
        lines, info = fs.describe(chain, p, a.dest or None)
        self.assertEqual(info["kind"], kind, "\n".join(lines))
        return p, info

    def build_fails(self, boxes, tip, a, pattern, wallet_boxes=None):
        with self.assertRaisesRegex(fs.Fail, pattern):
            fs.build(chain_for(boxes, tip, wallet_boxes=wallet_boxes), a)

    def test_treasury_actions(self):
        T = tfund(treg())
        acc = [bx("A1", 3 * N, TACC), bx("A2", 4 * N, TACC)]
        p, _ = self.build_ok([T, FB] + acc, 5000, build_args("treasury", "forward"), "forward")
        self.assertEqual([i["boxId"] for i in p["tx"]["inputs"]], [T["boxId"], FB["boxId"]] + [b["boxId"] for b in acc])
        self.build_ok([T, FB], 5000, build_args("treasury", "approve", to="9hRCPT", amount="50"), "approve")
        self.build_ok([T, FB], 5000, build_args("treasury", "freeze"), "freeze")
        T1 = tfund(treg(flag=1))
        self.build_ok([T1, FB], 5000, build_args("treasury", "unfreeze"), "unfreeze")
        Tp = tfund(treg(COMM, 1000, bytes.fromhex(RCPT), 50 * N, 0))
        self.build_ok([Tp, FB], 5000, build_args("treasury", "cancel"), "cancel")
        Tm = tfund(treg(MIG, 900, b"", 0, 2))
        p, _ = self.build_ok([Tm, FB], 905, build_args("treasury", "cancel"), "cancel")
        self.assertEqual(fs.registers(p["tx"]["outputs"][0], "treasury")["flag"], 0)
        p, _ = self.build_ok([Tp], 1000 + D + 1, build_args("treasury", "execute"), "execute")
        self.assertEqual(len(p["tx"]["inputs"]), 1)

    def test_lp_actions(self):
        L = lfund(lreg([(HR, 1000)]))
        acc = [bx("B1", 3 * N, LACC)]
        self.build_ok([L, FB] + acc, 5000, build_args("lp", "forward"), "forward")
        _, info = self.build_ok([L, FB], 5000, build_args("lp", "whitelist-add", dest=["9hDEST2"]), "whitelist-add")
        self.assertTrue(info["dest_ok"])
        self.build_ok([L], 1000 + D + 1, build_args("lp", "spend", to="9hRCPT", amount="100"), "spend")
        self.build_ok([L, FB], 5000, build_args("lp", "freeze"), "freeze")
        L1 = lfund(lreg([(HR, 1000)], flag=1))
        self.build_ok([L1, FB], 5000, build_args("lp", "unfreeze"), "unfreeze")
        Lm = lfund(lreg([(HR, 1000)], flag=2, mh=5000, mt=MIG))
        self.build_ok([Lm, FB], 5005, build_args("lp", "freeze"), "freeze")
        p, _ = self.build_ok([Lm, FB], 5005, build_args("lp", "cancel"), "cancel-migration")
        r = fs.registers(p["tx"]["outputs"][0], "lp")
        self.assertEqual((r["flag"], r["migration_height"], r["migration_target"]), (0, 0, MIG))

    def test_lp_cancel_needs_migration(self):
        L = lfund(lreg([(HR, 1000)]))
        self.build_fails([L, FB], 5000, build_args("lp", "cancel"), "no migration approval")

    def test_build_copies_only_fund_registers(self):      # F2
        T = tfund(dict(treg(), R9="0500"))
        p = fs.build(chain_for([T, FB], 5000), build_args("treasury", "freeze"))
        self.assertEqual(set(p["tx"]["outputs"][0]["additionalRegisters"]), set(fs.REG_KEYS["treasury"]))

    def test_below_floor(self):
        T = tfund(treg(), value=150 * N)
        self.build_fails([T, FB], 5000, build_args("treasury", "approve", to="9hRCPT", amount="60"), "below its floor")
        L = lfund(lreg([(HR, 1000)]), value=150 * N)
        self.build_fails([L], 1000 + D + 1, build_args("lp", "spend", to="9hRCPT", amount="60"), "below its floor")

    def test_execute_window(self):             # F3
        Tp = tfund(treg(COMM, 1000, bytes.fromhex(RCPT), 50 * N, 0))
        self.build_fails([Tp], 1000 + D, build_args("treasury", "execute"), "wait is not over")
        last = 1000 + D + W
        self.build_fails([Tp], last, build_args("treasury", "execute"), "window closes")
        self.build_ok([Tp], last - 1, build_args("treasury", "execute"), "execute")

    def test_lp_destination(self):
        L = lfund(lreg([(HR, 1000)]))
        self.build_fails([L], 1000 + D + 1, build_args("lp", "spend", to="9hEVXX", amount="1"), "not on the whitelist")
        self.build_fails([L], 1000 + D, build_args("lp", "spend", to="9hRCPT", amount="1"), "usable only from height")

    def test_frozen_states(self):
        T1 = tfund(treg(flag=1))
        acc = [bx("A1", 3 * N, TACC)]
        self.build_fails([T1, FB] + acc, 5000, build_args("treasury", "forward"), "frozen")
        self.build_fails([T1, FB], 5000, build_args("treasury", "freeze"), "already frozen")
        self.build_fails([T1, FB], 5000, build_args("treasury", "approve", to="9hRCPT", amount="1"), "normal state")
        T = tfund(treg())
        self.build_fails([T, FB], 5000, build_args("treasury", "unfreeze"), "not frozen")
        L1 = lfund(lreg([(HR, 1000)], flag=1))
        self.build_fails([L1, FB], 5000, build_args("lp", "whitelist-add", dest=["9hDEST2"]), "frozen")
        self.build_fails([L1], 1000 + D + 1, build_args("lp", "spend", to="9hRCPT", amount="1"), "frozen")

    def test_fee_box_rules(self):              # F5, F8
        T = tfund(treg())
        small = bx("SM", FEE + fs.MIN_BOX - 1, PROP)
        self.build_fails([T, small], 5000, build_args("treasury", "freeze", fee_box=small["boxId"]), "exactly the fee or at least")
        exact = bx("EX", FEE, PROP)
        p, _ = self.build_ok([T, exact], 5000, build_args("treasury", "freeze", fee_box=exact["boxId"]), "freeze")
        self.assertEqual(len(p["tx"]["outputs"]), 2)
        p2s = bx("PS", 50 * N, P2S)
        self.build_fails([T, p2s], 5000, build_args("treasury", "freeze", fee_box=p2s["boxId"]), "not at a plain public key")
        # automatic choice skips non-P2PK wallet boxes, even the smallest
        mid = bx("MID", 3 * N, PROP)
        p, _ = self.build_ok([T, p2s, mid], 5000, build_args("treasury", "freeze"), "freeze", wallet_boxes=[p2s, mid])
        self.assertEqual(p["tx"]["inputs"][1]["boxId"], mid["boxId"])
        self.build_fails([T, p2s], 5000, build_args("treasury", "freeze"), "no P2PK box", wallet_boxes=[p2s])

    def test_min_box_skips_dust(self):          # F14
        T = tfund(treg())
        dust = [bx(f"D{i}", N // 2, TACC) for i in range(3)]
        big = bx("BIG", 3 * N, TACC)
        p, info = self.build_ok([T, FB] + dust + [big], 5000, build_args("treasury", "forward", max_boxes=1), "forward")
        self.assertEqual([b["boxId"] for b in info["acc"]], [big["boxId"]])
        self.build_fails([T, FB] + dust, 5000, build_args("treasury", "forward"), "no accumulation boxes of at least")
        p, info = self.build_ok([T, FB] + dust, 5000, build_args("treasury", "forward", min_box_nano=1), "forward")
        self.assertEqual(len(info["acc"]), 3)


# ---------------------------------------------------------------- stand-in and signing flow

class StandIn(unittest.TestCase):
    def test_forward_stand_in(self):
        L = lfund(lreg([(HR, 1000)]))
        acc = [bx("B1", 3 * N, LACC), bx("B2", 4 * N, LACC)]
        tx, boxes = kept(L, lreg([(HR, 1000)]), 5005, acc)
        _, info = describe(boxes, 5005, tx)
        stx, sboxes = fs.stand_in(tx, info)
        self.assertEqual([i["boxId"] for i in stx["inputs"]], [L["boxId"], FB["boxId"]])
        self.assertEqual([b["boxId"] for b in sboxes], [L["boxId"], FB["boxId"]])
        self.assertEqual(stx["outputs"][0]["value"], L["value"])
        self.assertEqual(stx["outputs"][1:], tx["outputs"][1:])
        self.assertEqual(tx["outputs"][0]["value"], L["value"] + 7 * N)   # the original is untouched

    def test_no_accumulation_is_unchanged(self):
        T = tfund(treg())
        tx, boxes = kept(T, treg(flag=1), 5000)
        _, info = describe(boxes, 5000, tx)
        self.assertIs(fs.stand_in(tx, info)[0], tx)


def ns(**kw):
    base = dict(yes=True, devnet_secret_file=None, expect_dest=[], secret=None, send=False)
    base.update(kw)
    return argparse.Namespace(**base)


class Signing(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.addCleanup(self.dir.cleanup)
        self.out = io.StringIO()

    def write_proposal(self, tx, name="p"):
        path = os.path.join(self.dir.name, name + ".proposal.json")
        with open(path, "w") as f:
            json.dump({"version": 1, "tx": tx}, f)
        return path, path[:-len(".proposal.json")]

    def run_cmd(self, fn, node, a):
        with contextlib.redirect_stdout(self.out):
            fn(fs.Chain(node), a)

    def freeze_round(self):
        T = tfund(treg())
        tx, boxes = kept(T, treg(flag=1), 5000)
        path, base = self.write_proposal(tx)
        return boxes, path, base

    def test_secret_file_private_and_not_overwritten(self):     # F6, F7
        boxes, path, base = self.freeze_round()
        self.run_cmd(fs.cmd_commit, MockNode(boxes, 5000, wallet_key=K[0]), ns(proposal=path))
        spath = base + ".secret.json"
        self.assertEqual(stat.S_IMODE(os.stat(spath).st_mode), 0o600)
        self.assertTrue(os.path.exists(base + ".commit.json"))
        with open(spath) as f:
            before = f.read()
        node = MockNode(boxes, 5000, wallet_key=K[0])
        with self.assertRaisesRegex(fs.Fail, "already exists"):
            self.run_cmd(fs.cmd_commit, node, ns(proposal=path))
        self.assertNotIn("/wallet/generateCommitments", node.calls)
        with open(spath) as f:
            self.assertEqual(f.read(), before)

    def test_private_write_refuses_symlink_and_existing(self):   # F7
        target = os.path.join(self.dir.name, "target")
        link = os.path.join(self.dir.name, "x.secret.json")
        os.symlink(target, link)
        with self.assertRaises(fs.Fail):
            fs.write_json(link, {"a": 1}, private=True)
        self.assertFalse(os.path.exists(target))
        p = os.path.join(self.dir.name, "y.secret.json")
        fs.write_json(p, {"a": 1}, private=True)
        with self.assertRaises(fs.Fail):
            fs.write_json(p, {"a": 2}, private=True)

    def full_round_until_partial(self, boxes, tip, path, expect=()):
        self.run_cmd(fs.cmd_commit, MockNode(boxes, tip, wallet_key=K[0]), ns(proposal=path))
        base = path[:-len(".proposal.json")]
        self.run_cmd(fs.cmd_cosign, MockNode(boxes, tip, wallet_key=K[1]),
                     ns(proposal=path, commit=base + ".commit.json", expect_dest=list(expect)))
        return base

    def test_failed_finish_deletes_secret_before_signing(self):  # F6
        boxes, path, base = self.freeze_round()
        self.full_round_until_partial(boxes, 5000, path)
        spath = base + ".secret.json"
        seen = []
        node = MockNode(boxes, 5000, wallet_key=K[0], fail_sign=True,
                        on_sign=lambda data: seen.append(os.path.exists(spath)))
        with self.assertRaisesRegex(fs.Fail, "commit again"):
            self.run_cmd(fs.cmd_finish, node, ns(proposal=path, partial=base + ".partial.json"))
        self.assertEqual(seen, [False])           # the secret was already gone when the sign call was made
        self.assertFalse(os.path.exists(spath))
        self.assertFalse(os.path.exists(base + ".signed.json"))

    def test_finish_success(self):
        boxes, path, base = self.freeze_round()
        self.full_round_until_partial(boxes, 5000, path)
        self.run_cmd(fs.cmd_finish, MockNode(boxes, 5000, wallet_key=K[0]), ns(proposal=path, partial=base + ".partial.json"))
        self.assertTrue(os.path.exists(base + ".signed.json"))
        self.assertFalse(os.path.exists(base + ".secret.json"))

    def test_cosign_refuses_when_cosigner_pays_fee(self):        # F5
        T = tfund(treg())
        cos_box = bx("CB", 5 * N, COS)
        tx, boxes = kept(T, treg(flag=1), 5000, fee_box=cos_box)
        path, base = self.write_proposal(tx)
        self.run_cmd(fs.cmd_commit, MockNode(boxes, 5000, wallet_key=K[0]), ns(proposal=path))
        node = MockNode(boxes, 5000, wallet_key=K[1])
        with self.assertRaisesRegex(fs.Fail, "can sign the fee input"):
            self.run_cmd(fs.cmd_cosign, node, ns(proposal=path, commit=base + ".commit.json"))
        self.assertNotIn("/wallet/transaction/sign", node.calls)

    def test_whitelist_add_needs_expect_dest(self):              # F12
        L = lfund(lreg([(HR, 1000)]))
        tx, boxes = kept(L, lreg([(HR, 1000), (HD, 5000)]), 5000)
        path, base = self.write_proposal(tx)
        self.run_cmd(fs.cmd_commit, MockNode(boxes, 5000, wallet_key=K[0]), ns(proposal=path))
        for expect in ([], ["9hEVXX"], ["9hDEST2", "9hEVXX"]):
            node = MockNode(boxes, 5000, wallet_key=K[1])
            with self.assertRaisesRegex(fs.Fail, "--expect-dest", msg=str(expect)):
                self.run_cmd(fs.cmd_cosign, node, ns(proposal=path, commit=base + ".commit.json", expect_dest=expect))
            self.assertNotIn("/wallet/transaction/sign", node.calls)
        self.run_cmd(fs.cmd_cosign, MockNode(boxes, 5000, wallet_key=K[1]),
                     ns(proposal=path, commit=base + ".commit.json", expect_dest=["9hDEST2"]))
        self.assertIn("adds destination 9hDEST2 (hash", self.out.getvalue())
        # finish refuses without a match too, and keeps the secret since nothing was signed
        node = MockNode(boxes, 5000, wallet_key=K[0])
        with self.assertRaisesRegex(fs.Fail, "--expect-dest"):
            self.run_cmd(fs.cmd_finish, node, ns(proposal=path, partial=base + ".partial.json"))
        self.assertTrue(os.path.exists(base + ".secret.json"))
        self.run_cmd(fs.cmd_finish, MockNode(boxes, 5000, wallet_key=K[0]),
                     ns(proposal=path, partial=base + ".partial.json", expect_dest=["9hDEST2"]))
        self.assertTrue(os.path.exists(base + ".signed.json"))

    def test_show_prints_match(self):
        L = lfund(lreg([(HR, 1000)]))
        tx, boxes = kept(L, lreg([(HR, 1000), (HD, 5000)]), 5000)
        path, _ = self.write_proposal(tx)
        self.run_cmd(fs.cmd_show, MockNode(boxes, 5000), ns(proposal=path, expect_dest=["9hDEST2"]))
        self.assertIn("--expect-dest: every added destination is one you named", self.out.getvalue())
        self.run_cmd(fs.cmd_show, MockNode(boxes, 5000), ns(proposal=path, expect_dest=["9hEVXX"]))
        self.assertIn("DOES NOT MATCH", self.out.getvalue())
        self.assertIn("NOT among --expect-dest", self.out.getvalue())


# ---------------------------------------------------------------- node transport and the devnet option

class FakeOpener:
    def __init__(self, body):
        self.body = body

    def open(self, req, timeout=None):
        raise urllib.error.HTTPError(req.full_url, 400, "Bad Request", {}, io.BytesIO(self.body))


class Transport(unittest.TestCase):
    def setUp(self):
        self.saved = fs.OPENER
        self.addCleanup(setattr, fs, "OPENER", self.saved)

    def node(self):
        with contextlib.redirect_stderr(io.StringIO()):
            return fs.Node("http://127.0.0.1:9", None)

    def test_f17_sensitive_error_has_no_body(self):
        fs.OPENER = FakeOpener(b'{"detail": "MARKERBODY Request(...). something failed"}')
        n = self.node()
        for data in ({"tx": {}, "hints": {"secretHints": {"0": []}, "publicHints": {}}},
                     {"tx": {}, "secrets": {"dlog": ["00"]}}):
            with self.assertRaises(fs.Fail) as cm:
                n.call("/wallet/transaction/sign", data)
            self.assertEqual(str(cm.exception), "node refused /wallet/transaction/sign: HTTP 400")
        with self.assertRaises(fs.Fail) as cm:
            n.call("/transactions/check", {"inputs": []})
        self.assertIn("something failed", str(cm.exception))

    def test_f18_redirect_refused(self):
        with self.assertRaisesRegex(fs.Fail, "redirect"):
            fs.NoRedirect().redirect_request(None, None, 302, "Found", {}, "http://elsewhere/")
        self.assertTrue(any(isinstance(h, fs.NoRedirect) for h in self.saved.handlers))

    def test_f18_loopback_warning(self):
        for url, local in [("http://127.0.0.1:9153", True), ("http://localhost:9153", True), ("http://[::1]:9153", True),
                           ("http://10.0.0.5:9053", False), ("http://node.example:9053", False)]:
            self.assertEqual(fs.is_loopback(url), local, url)
            err = io.StringIO()
            with contextlib.redirect_stderr(err):
                fs.Node(url, None)
            self.assertEqual(err.getvalue().count("WARNING"), 0 if local else 1, url)

    def test_f17_devnet_secret_file_needs_env(self):
        calls = []
        saved_chain = fs.Chain

        def no_chain(node):
            calls.append(node)
            raise fs.Fail("stop before any node call")
        fs.Chain = no_chain
        self.addCleanup(setattr, fs, "Chain", saved_chain)
        saved_env = os.environ.pop("YOLO_DEVNET", None)
        self.addCleanup(lambda: os.environ.__setitem__("YOLO_DEVNET", saved_env) if saved_env is not None
                        else os.environ.pop("YOLO_DEVNET", None))
        argv = ["--node", "http://127.0.0.1:9", "commit", "x.proposal.json", "--devnet-secret-file", "nofile"]
        err = io.StringIO()
        with contextlib.redirect_stderr(err):
            self.assertEqual(fs.main(argv), 1)
        self.assertIn("YOLO_DEVNET=1", err.getvalue())
        self.assertEqual(calls, [])
        os.environ["YOLO_DEVNET"] = "1"
        err = io.StringIO()
        with contextlib.redirect_stderr(err):
            self.assertEqual(fs.main(argv), 1)
        self.assertEqual(err.getvalue().count("WARNING"), 1)
        self.assertEqual(len(calls), 1)


if __name__ == "__main__":
    unittest.main()
