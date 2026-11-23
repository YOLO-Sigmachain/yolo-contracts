#!/usr/bin/env python3
"""fundsign: build, check and co-sign YOLO treasury and liquidity fund transactions.

Two of the three fund signers take part. Each signer runs this tool against their OWN node,
whose wallet holds their signer key. Files are passed between the two signers by any channel.

  proposer:  build  ->  commit                      (sends proposal + commit file)
  cosigner:  show   ->  cosign                      (sends partial file back)
  proposer:  finish [--send]

Python 3.8+, standard library only. See README.md.
"""
import argparse, hashlib, ipaddress, json, os, re, sys, time, urllib.error, urllib.parse, urllib.request
from decimal import Decimal, InvalidOperation

NANO = 10**9
MINER_FEE_TREE = ("1005040004000e36100204a00b08cd0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
                  "ea02d192a39a8cc7a701730073011001020402d19683030193a38cc7b2a57300000193c2b2a57301007473027303830108"
                  "cdeeac93b1a57304")
GENERATOR_PK = "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
TIMELOCK = 8640          # timelockBlocks in both contracts
WINDOW = 20160           # windowBlocks
HEIGHT_SLACK = 10        # heightSlack: a recorded height must be within 10 blocks of inclusion
SIGN_WINDOW = 720        # signWindow: O0 creation height must be within 720 blocks of inclusion
FUND_FLOOR = 100 * NANO  # fundFloor
FEE_CAP = 10_000_000     # feeCap / spendFeeCap in the contracts; this tool applies it to every transaction it builds or signs
MAX_WHITELIST = 64
MINER_LOCK = 2880        # miner reward lock
MIN_BOX = 1_000_000      # smallest change output this tool will create
DEFAULT_FEE = 2_000_000
FUNDS = {"treasury": 1, "lp": 2}   # index of the fund box in /utxo/genesis
ACC_REG = {"treasury": "R4", "lp": "R5"}  # emission box register holding the accumulation script hash
REG_KEYS = {"treasury": ("R4", "R5", "R6", "R7", "R8"), "lp": ("R4", "R5", "R6", "R7")}  # exactly these on a fund box

ID_RE = re.compile(r"[0-9a-f]{64}")
HEX_RE = re.compile(r"(?:[0-9a-f]{2})+")
B58_RE = re.compile(r"[1-9A-HJ-NP-Za-km-z]+")
P2PK_RE = re.compile(r"0008cd[0-9a-f]{66}")


class Fail(Exception):
    pass


def check_id(s, what="box id"):
    if not isinstance(s, str) or not ID_RE.fullmatch(s):
        raise Fail(f"not a valid {what}: {str(s)[:80]!r}")
    return s


def check_hex(s, what):
    if not isinstance(s, str) or not HEX_RE.fullmatch(s):
        raise Fail(f"{what} is not lowercase hex of even length")
    return s


def check_address(s):
    if not isinstance(s, str) or not B58_RE.fullmatch(s):
        raise Fail(f"not an address (base58 characters only): {str(s)[:80]!r}")
    return s


def is_int(v):
    return isinstance(v, int) and not isinstance(v, bool)


def is_p2pk(tree):
    return isinstance(tree, str) and P2PK_RE.fullmatch(tree) is not None


# ---------------------------------------------------------------- encoding

def vlq(n):
    if n < 0:
        raise ValueError("negative vlq")
    out = bytearray()
    while True:
        b = n & 0x7F
        n >>= 7
        if n:
            out.append(b | 0x80)
        else:
            out.append(b)
            return bytes(out)


def read_vlq(b, i):
    n = shift = 0
    while True:
        if i >= len(b):
            raise Fail("truncated register value")
        c = b[i]
        i += 1
        n |= (c & 0x7F) << shift
        if not c & 0x80:
            return n, i
        shift += 7


def enc_long(n):
    return "05" + vlq((n << 1) ^ (n >> 63)).hex()


def dec_long(hexv):
    b = bytes.fromhex(hexv)
    if not b or b[0] != 0x05:
        raise Fail(f"register is not a Long: {hexv[:16]}")
    z, i = read_vlq(b, 1)
    if i != len(b):
        raise Fail("trailing bytes after Long")
    return (z >> 1) ^ -(z & 1)


def enc_bytes(b):
    return "0e" + vlq(len(b)).hex() + b.hex()


def dec_bytes(hexv):
    b = bytes.fromhex(hexv)
    if not b or b[0] != 0x0E:
        raise Fail(f"register is not Coll[Byte]: {hexv[:16]}")
    n, i = read_vlq(b, 1)
    if i + n != len(b):
        raise Fail("bad Coll[Byte] length")
    return b[i:]


WL_PREFIX = "0c4d0e"  # Coll[(Coll[Byte], Long)]


def enc_whitelist(entries):
    out = bytes.fromhex(WL_PREFIX) + vlq(len(entries))
    for h, height in entries:
        out += vlq(len(h)) + h + vlq((height << 1) ^ (height >> 63))
    return out.hex()


def dec_whitelist(hexv):
    if not hexv.startswith(WL_PREFIX):
        raise Fail(f"register is not a whitelist: {hexv[:16]}")
    b = bytes.fromhex(hexv)
    n, i = read_vlq(b, 3)
    out = []
    for _ in range(n):
        ln, i = read_vlq(b, i)
        h = b[i:i + ln]
        if len(h) != ln:
            raise Fail("truncated whitelist")
        i += ln
        z, i = read_vlq(b, i)
        out.append((h, (z >> 1) ^ -(z & 1)))
    if i != len(b):
        raise Fail("trailing bytes after whitelist")
    return out


def b2b(b):
    return hashlib.blake2b(b, digest_size=32).digest()


def to_nano(text):
    try:
        d = Decimal(text)
    except InvalidOperation:
        raise Fail(f"not an amount: {text}")
    n = d * NANO
    if n != n.to_integral_value() or n <= 0:
        raise Fail(f"amount must be positive with at most 9 decimals: {text}")
    return int(n)


def coins(n):
    sign, n = ("-", -n) if n < 0 else ("", n)
    s = f"{n // NANO:,}.{n % NANO:09d}".rstrip("0").rstrip(".")
    return sign + s + " YOLO"


def tx_hash(tx):
    return hashlib.sha256(json.dumps(tx, sort_keys=True, separators=(",", ":")).encode()).hexdigest()


def signer_keys(tree_hex):
    """The three signer public keys embedded in a fund script, in script order."""
    keys, i = [], 0
    while True:
        i = tree_hex.find("08cd", i)
        if i < 0:
            break
        if i % 2:          # only at a byte boundary
            i += 1
            continue
        k = tree_hex[i + 4:i + 70]
        if len(k) == 66 and k[:2] in ("02", "03") and k != GENERATOR_PK and k not in keys:
            keys.append(k)
        i += 4
    if len(keys) != 3:
        raise Fail(f"expected 3 signer keys in the fund script, found {len(keys)}")
    return keys


# ---------------------------------------------------------------- node

class NoRedirect(urllib.request.HTTPRedirectHandler):
    """The API key and signing requests go only to the address given; a redirect is refused."""

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise Fail(f"the node answered with a redirect (HTTP {code}); refusing to follow it")


OPENER = urllib.request.build_opener(NoRedirect)


def is_loopback(url):
    host = urllib.parse.urlsplit(url).hostname or ""
    if host == "localhost":
        return True
    try:
        return ipaddress.ip_address(host).is_loopback
    except ValueError:
        return False


class Node:
    def __init__(self, url, key_file):
        self.url = url.rstrip("/")
        if not is_loopback(self.url):
            print(f"fundsign: WARNING: --node {self.url} is not a loopback address; the API key and "
                  "signing requests go over the network", file=sys.stderr)
        key = os.environ.get("YOLO_NODE_API_KEY", "")
        if key_file:
            with open(key_file) as f:
                key = f.read().strip()
        self.key = key

    def call(self, path, data=None):
        sensitive = isinstance(data, dict) and (
            "secrets" in data or "secretHints" in (data.get("hints") or {}))
        req = urllib.request.Request(
            self.url + path, data=None if data is None else json.dumps(data).encode(),
            headers={"api_key": self.key, "Content-Type": "application/json"})
        try:
            with OPENER.open(req, timeout=90) as r:
                body = r.read()
                return json.loads(body) if body else None
        except urllib.error.HTTPError as e:
            if sensitive:   # the reply can quote a request that carried key material
                raise Fail(f"node refused {path}: HTTP {e.code}")
            raise Fail(f"node refused {path}: HTTP {e.code}: {node_message(e.read())}")
        except (urllib.error.URLError, OSError) as e:
            raise Fail(f"cannot reach node at {self.url}: {e}")

    def height(self):
        return self.call("/info")["fullHeight"]

    def box(self, box_id):
        return self.call(f"/utxo/byId/{check_id(box_id)}")

    def raw(self, box_id):
        return self.call(f"/utxo/byIdBinary/{check_id(box_id)}")["bytes"]

    def tree_of(self, address):
        return self.call(f"/script/addressToTree/{check_address(address)}")["tree"]

    def address_of(self, tree, strict=False):
        try:
            check_hex(tree, "ergoTree")   # never put anything else in the URL path
            return self.call(f"/utils/ergoTreeToAddress/{tree}")["address"]
        except Fail:
            if strict:
                raise Fail(f"this node cannot turn script {str(tree)[:24]}... into an address")
            return "script " + str(tree)[:24] + "..."


def node_message(body):
    """The node's error text. Its 400 replies quote the whole request first, and a request can
    carry key material in test mode, so only the text after the quoted request is kept."""
    try:
        d = json.loads(body)
        d = d.get("detail") or d.get("reason") or ""
    except (ValueError, AttributeError):
        d = body.decode(errors="replace") if isinstance(body, bytes) else str(body)
    if not isinstance(d, str):
        d = json.dumps(d)
    i = d.rfind("). ")
    if i >= 0:
        d = d[i + 3:]
    elif "Request(" in d:
        d = "(message withheld: it quotes the request)"
    return d[-300:]


# ---------------------------------------------------------------- chain state

class Chain:
    """What the tool needs to know about the two funds, read from one node."""

    def __init__(self, node):
        self.node = node
        g = node.call("/utxo/genesis")
        if len(g) < 3 or not g[0]["assets"]:
            raise Fail("this node's genesis does not look like a YOLO genesis")
        self.emission = g[0]
        self.genesis = {name: g[i] for name, i in FUNDS.items()}
        self.nft = {name: check_id(b["assets"][0]["tokenId"], "token id") for name, b in self.genesis.items()}
        self.tree = {name: b["ergoTree"] for name, b in self.genesis.items()}
        self.acc_hash = {name: dec_bytes(self.emission["additionalRegisters"][ACC_REG[name]])
                         for name in FUNDS}

    def fund_of_box(self, box):
        for name, nft in self.nft.items():
            if box["assets"] and box["assets"][0] == {"tokenId": nft, "amount": 1}:
                return name
        return None

    def fund_box(self, name):
        try:
            r = self.node.call(f"/blockchain/box/unspent/byTokenId/{self.nft[name]}?offset=0&limit=5")
            items = r["items"] if isinstance(r, dict) else r
        except Fail:
            items = []
        if len(items) > 1:
            raise Fail(f"more than one unspent box holds the {name} NFT")
        box_id = items[0]["boxId"] if items else self.genesis[name]["boxId"]
        try:
            box = self.node.box(box_id)
        except Fail:
            raise Fail(f"cannot find the unspent {name} fund box (does this node run with extraIndex = true?)")
        if self.fund_of_box(box) != name:
            raise Fail("the box found does not hold the fund NFT at tokens(0)")
        return box

    def is_accumulation(self, box, name):
        return b2b(bytes.fromhex(box["ergoTree"])) == self.acc_hash[name]

    def accumulation_tree(self, name):
        tip = self.node.height()
        for h in range(tip, max(tip - 50, 0), -1):
            hid = check_id(self.node.call(f"/blocks/at/{int(h)}")[0], "block id")
            for o in self.node.call(f"/blocks/{hid}/transactions")["transactions"][0]["outputs"]:
                if self.is_accumulation(o, name):
                    return o["ergoTree"]
        raise Fail("no accumulation output found in the last 50 blocks")

    def accumulation_boxes(self, name, limit, min_value=0, confirm=True):
        """Up to `limit` accumulation boxes, skipping boxes holding tokens or worth less than min_value.
        With confirm (the default) each box is read again from the unspent set; `status` skips that
        and only counts what the index lists."""
        tree = self.accumulation_tree(name)
        out, offset, page = [], 0, 100 if confirm else 1000
        while len(out) < limit:
            r = self.node.call(f"/blockchain/box/unspent/byErgoTree?offset={offset}&limit={page}&sortDirection=asc", tree)
            items = r["items"] if isinstance(r, dict) else r
            if not items:
                break
            for b in items:
                if not b.get("assets") and b.get("value", 0) >= min_value and len(out) < limit:
                    if not confirm:
                        out.append(b)
                        continue
                    try:
                        out.append(self.node.box(b["boxId"]))
                    except Fail:
                        pass
            if len(items) < page:
                break
            offset += page
        return out


def registers(box, fund):
    r = box["additionalRegisters"]
    try:
        if fund == "treasury":
            return {"commitment": dec_bytes(r["R4"]), "approval_height": dec_long(r["R5"]),
                    "recipient": dec_bytes(r["R6"]), "amount": dec_long(r["R7"]), "flag": dec_long(r["R8"])}
        return {"whitelist": dec_whitelist(r["R4"]), "flag": dec_long(r["R5"]),
                "migration_height": dec_long(r["R6"]), "migration_target": dec_bytes(r["R7"])}
    except KeyError as e:
        raise Fail(f"fund box is missing register {e}")


# ---------------------------------------------------------------- building

def out_box(value, tree, height, assets=None, regs=None):
    return {"value": value, "ergoTree": tree, "creationHeight": height,
            "assets": assets or [], "additionalRegisters": regs or {}}


def pick_fee_box(node, need, tip, wanted=None):
    """A wallet box of the proposer that pays the fee. It must be at a plain public key address
    (P2PK) and hold no tokens."""
    if wanted:
        b = node.box(wanted)
        if not is_p2pk(b["ergoTree"]):
            raise Fail("the fee box is not at a plain public key address (P2PK); choose a box that is")
        if b["assets"]:
            raise Fail("the fee box holds tokens; choose a box with none")
        return b
    best = None
    # A mining wallet holds thousands of reward boxes, so read every page, not only the first.
    offset, page = 0, 500
    while True:
        items = node.call(f"/wallet/boxes/unspent?minConfirmations=1&minInclusionHeight=0&limit={page}&offset={offset}")
        for item in items:
            b = item["box"]
            if b["assets"] or b["value"] < need + MIN_BOX or not is_p2pk(b["ergoTree"]):
                continue
            if best is None or b["value"] < best["value"]:
                best = b
        if len(items) < page:
            break
        offset += page
    if best is None:
        raise Fail(f"the node wallet has no P2PK box of at least {coins(need + MIN_BOX)} without tokens")
    return best


def build(chain, a):
    node, fund = chain.node, a.fund
    f = chain.fund_box(fund)
    reg = registers(f, fund)
    tip = node.height()
    h = tip              # recorded heights: the node signs as of its current height, so not tip + 1
    if not 0 < a.fee_nano <= FEE_CAP:
        raise Fail(f"fee must be above 0 and at most {coins(FEE_CAP)}")
    fee = a.fee_nano
    regs = {k: f["additionalRegisters"][k] for k in REG_KEYS[fund]}
    o0_value = f["value"]
    extra_inputs, payments, from_fund = [], [], False
    act = a.action
    frozen = reg["flag"] == 1

    def need(cond, msg):
        if not cond:
            raise Fail(f"cannot build {fund} {act}: {msg}")

    if act == "forward":
        need(not frozen, "the fund is frozen")
        acc = chain.accumulation_boxes(fund, a.max_boxes, a.min_box_nano)
        need(acc, f"there are no accumulation boxes of at least {coins(a.min_box_nano)} to forward")
        extra_inputs = acc
        o0_value += sum(b["value"] for b in acc)
    elif act == "freeze":
        need(not frozen, "the fund is already frozen")
        regs["R8" if fund == "treasury" else "R5"] = enc_long(1)
    elif act == "unfreeze":
        need(frozen, "the fund is not frozen")
        regs["R8" if fund == "treasury" else "R5"] = enc_long(0)
    elif fund == "treasury" and act == "approve":
        need(reg["flag"] == 0 and reg["approval_height"] == 0, "a proposal is already recorded, or the fund is not in the normal state")
        amount = to_nano(a.amount)
        rec = bytes.fromhex(node.tree_of(a.to))
        need(f["value"] - amount - FEE_CAP >= FUND_FLOOR, f"the payment would take the fund below its floor of {coins(FUND_FLOOR)}")
        regs.update(R4=enc_bytes(b2b(rec + amount.to_bytes(8, "big"))), R5=enc_long(h),
                    R6=enc_bytes(rec), R7=enc_long(amount), R8=enc_long(0))
    elif fund == "treasury" and act == "cancel":
        need(reg["approval_height"] > 0, "nothing is recorded")
        regs["R5"] = enc_long(0)
        if reg["flag"] == 2:
            regs["R8"] = enc_long(0)
    elif fund == "treasury" and act == "execute":
        need(reg["flag"] == 0 and reg["approval_height"] > 0, "no payment proposal is recorded in the normal state")
        first, last = reg["approval_height"] + TIMELOCK + 1, reg["approval_height"] + TIMELOCK + WINDOW
        need(first <= tip, f"the wait is not over; possible from height {first} (now {tip})")
        need(tip + 1 <= last, f"the window closes at height {last} (now {tip}); cancel and approve again")
        need(reg["commitment"] == b2b(reg["recipient"] + reg["amount"].to_bytes(8, "big")), "the recorded commitment does not match")
        o0_value = f["value"] - reg["amount"] - fee
        need(o0_value >= FUND_FLOOR, "the payment would take the fund below its floor")
        regs.update(R5=enc_long(0), R8=enc_long(0))
        payments = [out_box(reg["amount"], reg["recipient"].hex(), tip)]
        from_fund = True
    elif fund == "lp" and act == "whitelist-add":
        need(not frozen, "the fund is frozen")
        wl = list(reg["whitelist"])
        for addr in a.dest:
            hsh = b2b(bytes.fromhex(node.tree_of(addr)))
            need(all(hsh != e[0] for e in wl), f"{addr} is already on the whitelist")
            wl.append((hsh, h))
        need(len(wl) <= MAX_WHITELIST, f"the whitelist holds at most {MAX_WHITELIST} entries")
        regs["R4"] = enc_whitelist(wl)
    elif fund == "lp" and act == "spend":
        need(not frozen, "the fund is frozen")
        amount = to_nano(a.amount)
        tree = node.tree_of(a.to)
        hsh = b2b(bytes.fromhex(tree))
        matured = [e for e in reg["whitelist"] if e[0] == hsh and h > e[1] + TIMELOCK]
        if not matured:
            waiting = [e[1] + TIMELOCK + 1 for e in reg["whitelist"] if e[0] == hsh]
            need(False, f"the destination is on the whitelist but usable only from height {min(waiting)}" if waiting
                 else "the destination is not on the whitelist")
        o0_value = f["value"] - amount - fee
        need(o0_value >= FUND_FLOOR, f"the payment would take the fund below its floor of {coins(FUND_FLOOR)}")
        payments = [out_box(amount, tree, tip)]
        from_fund = True
    elif fund == "lp" and act == "cancel":
        need(reg["flag"] == 2, "no migration approval is recorded")
        regs.update(R5=enc_long(0), R6=enc_long(0))
    else:
        raise Fail(f"unknown action for {fund}: {act}")

    o0 = out_box(o0_value, f["ergoTree"], tip, [{"tokenId": chain.nft[fund], "amount": 1}], regs)
    if from_fund:
        inputs = [f]
        outputs = [o0] + payments + [out_box(fee, MINER_FEE_TREE, tip)]
    else:
        fb = pick_fee_box(node, fee, tip, a.fee_box)
        if not (fb["value"] == fee or fb["value"] >= fee + MIN_BOX):
            raise Fail(f"the fee box must hold exactly the fee or at least {coins(fee + MIN_BOX)}; choose another box")
        change = fb["value"] - fee
        inputs = [f, fb] + extra_inputs
        outputs = [o0] + ([out_box(change, fb["ergoTree"], tip)] if change else []) + [out_box(fee, MINER_FEE_TREE, tip)]
    tx = {"inputs": [{"boxId": b["boxId"], "extension": {}} for b in inputs], "dataInputs": [], "outputs": outputs}
    return {"version": 1, "fund": fund, "action": act, "built_at_height": tip, "tx": tx}


# ---------------------------------------------------------------- checking

def validate_proposal(proposal):
    """Shape and character checks on everything the proposal carries, made before any node call:
    box ids are 64 lowercase hex, scripts and register values are lowercase hex of even length,
    output values are whole numbers above 0 and creation heights whole numbers >= 0."""
    if not isinstance(proposal, dict) or not isinstance(proposal.get("tx"), dict):
        raise Fail("not a proposal file")
    tx = proposal["tx"]
    ins, dins, outs = tx.get("inputs"), tx.get("dataInputs", []), tx.get("outputs")
    if not isinstance(ins, list) or not ins or not isinstance(outs, list) or not outs or not isinstance(dins, list):
        raise Fail("the transaction has no inputs or no outputs")
    for i in ins + dins:
        if not isinstance(i, dict):
            raise Fail("an input is malformed")
        check_id(i.get("boxId"))
    for k, o in enumerate(outs):
        if not isinstance(o, dict):
            raise Fail(f"output {k} is malformed")
        check_hex(o.get("ergoTree"), f"output {k} ergoTree")
        v, ch = o.get("value"), o.get("creationHeight")
        if not is_int(v) or v <= 0:
            raise Fail(f"output {k} value is not a whole number above 0")
        if not is_int(ch) or ch < 0:
            raise Fail(f"output {k} creation height is not a whole number >= 0")
        if not isinstance(o.get("assets"), list) or not isinstance(o.get("additionalRegisters"), dict):
            raise Fail(f"output {k} is malformed")
        for r, val in o["additionalRegisters"].items():
            check_hex(val, f"output {k} register {r}")


def describe(chain, proposal, expect=None):
    """Work out what a proposal does from the transaction itself and this node's view of its
    inputs. Nothing a proposal file says about itself is trusted. `expect` is the list of
    --expect-dest addresses, matched against the entries a whitelist-add adds.
    Returns (lines, info)."""
    validate_proposal(proposal)
    node, tx = chain.node, proposal["tx"]
    if tx.get("dataInputs"):
        raise Fail("the transaction has data inputs; this tool does not sign those")
    ids = [i["boxId"] for i in tx["inputs"]]
    if len(set(ids)) != len(ids):
        raise Fail("an input is listed twice")
    if any(i.get("extension") for i in tx["inputs"]):
        raise Fail("an input carries context variables; this tool does not sign those")
    boxes = []
    for i in ids:
        try:
            boxes.append(node.box(i))
        except Fail:
            raise Fail(f"input {i} is not unspent on this node (already spent, or this node is behind)")
    f, outs = boxes[0], tx["outputs"]
    fund = chain.fund_of_box(f)
    if fund is None or f["ergoTree"] != chain.tree[fund]:
        raise Fail("input 0 is not a fund box")
    if f["assets"] != [{"tokenId": chain.nft[fund], "amount": 1}]:
        raise Fail("the fund box holds tokens other than the fund NFT; refusing, since they would be lost")
    if any(chain.fund_of_box(b) for b in boxes[1:]):
        raise Fail("a second fund box is among the inputs; this tool signs for one fund at a time")
    tip = node.height()
    o0 = outs[0]
    if o0["ergoTree"] != f["ergoTree"]:
        raise Fail("output 0 is not at the fund script (a migration?); this tool does not sign migrations")
    if o0["assets"] != [{"tokenId": chain.nft[fund], "amount": 1}]:
        raise Fail("output 0 does not hold exactly the fund NFT")
    for what, b in (("the fund box", f), ("output 0", o0)):
        if set(b["additionalRegisters"]) != set(REG_KEYS[fund]):
            raise Fail(f"{what} must carry exactly the registers {', '.join(REG_KEYS[fund])}")
    # NEVER burn or move tokens: the only token allowed anywhere is the fund NFT, in O0
    for b in boxes[1:]:
        if b["assets"]:
            raise Fail(f"input {b['boxId'][:12]} holds tokens; refusing, since they could be lost")
    for k, o in enumerate(outs[1:], 1):
        if o["assets"]:
            raise Fail(f"output {k} holds tokens")
        if o["additionalRegisters"]:
            raise Fail(f"output {k} carries registers; this tool signs only outputs after output 0 without registers")
    total_in, total_out = sum(b["value"] for b in boxes), sum(o["value"] for o in outs)
    if total_in != total_out:
        raise Fail(f"inputs {total_in} and outputs {total_out} do not balance")
    newest = max(o["creationHeight"] for o in outs)
    if newest > tip + 1:
        raise Fail(f"this node is at height {tip} but the proposal was built at height {newest}: "
                   "this node is behind, wait for it to catch up and run the command again")

    acc = [b for b in boxes[1:] if chain.is_accumulation(b, fund)]
    acc_ids = {b["boxId"] for b in acc}
    other = [b for b in boxes[1:] if b["boxId"] not in acc_ids]
    if [b["boxId"] for b in boxes[1:]] != [b["boxId"] for b in other + acc]:
        raise Fail("accumulation boxes must come after every other input")
    if len(other) > 1:
        raise Fail("more than one fee input")
    if other and not is_p2pk(other[0]["ergoTree"]):
        raise Fail("the fee input is not at a plain public key address (P2PK)")
    acc_sum = sum(b["value"] for b in acc)
    fee_outs = [o for o in outs[1:] if o["ergoTree"] == MINER_FEE_TREE]
    if len(fee_outs) != 1:
        raise Fail("expected exactly one miner fee output")
    fee = fee_outs[0]["value"]
    if fee > FEE_CAP:
        raise Fail(f"miner fee {coins(fee)} is above {coins(FEE_CAP)}")
    own_trees = {b["ergoTree"] for b in other}
    change = [o for o in outs[1:] if o["ergoTree"] in own_trees]
    pays = [o for o in outs[1:] if o["ergoTree"] != MINER_FEE_TREE and o["ergoTree"] not in own_trees]
    if len(change) > 1:
        raise Fail("more than one change output")
    if other and sum(o["value"] for o in change) + fee != other[0]["value"]:
        raise Fail("the fee input's value does not equal its change plus the fee")
    left_fund = f["value"] + acc_sum - o0["value"]
    if left_fund < 0:
        raise Fail("output 0 holds more than the fund and its accumulation boxes")
    paid = sum(o["value"] for o in pays)
    if left_fund != paid + (0 if other else fee):
        raise Fail("value leaving the fund does not equal the payments plus the fee")
    if o0["value"] < min(f["value"], FUND_FLOOR):
        raise Fail("output 0 is below the fund floor")

    old, new = registers(f, fund), registers(o0, fund)
    deadline = o0["creationHeight"] + SIGN_WINDOW
    lines, kind, dest_ok = [], None, True
    same = old == new
    if fund == "treasury":
        prop = ("commitment", "recipient", "amount")
        same_prop = all(old[k] == new[k] for k in prop)
        if same:
            if pays:
                raise Fail("a payment with unchanged registers is not a valid treasury path")
            if old["flag"] == 1:
                raise Fail("the treasury is frozen; with registers unchanged this is not a valid path")
            kind = "forward" if acc else "consolidation"
        elif same_prop and old["approval_height"] > 0 and new["approval_height"] == 0 and pays:
            kind = "execute"
            if (len(pays) != 1 or outs[1] is not pays[0] or pays[0]["ergoTree"] != old["recipient"].hex()
                    or pays[0]["value"] != old["amount"] or new["flag"] != 0 or old["flag"] != 0):
                raise Fail("the payment does not match the recorded proposal")
            a0 = old["approval_height"]
            if tip < a0 + TIMELOCK + 1:
                raise Fail(f"the wait is not over; the payment is possible from height {a0 + TIMELOCK + 1} (now {tip})")
            deadline = min(deadline, a0 + TIMELOCK + WINDOW)
        elif pays:
            raise Fail("a payment on a path that does not allow one")
        elif same_prop and old["approval_height"] > 0 and new["approval_height"] == 0:
            kind = "cancel"
            if new["flag"] != (0 if old["flag"] == 2 else old["flag"]):
                raise Fail("a cancel must set the flag to 0 after a migration approval, and otherwise keep it")
        elif same_prop and old["approval_height"] == new["approval_height"] and new["flag"] == 1 and old["flag"] != 1:
            kind = "freeze"
        elif same_prop and old["approval_height"] == new["approval_height"] and new["flag"] == 0 and old["flag"] == 1:
            kind = "unfreeze"
        elif old["approval_height"] == 0 and old["flag"] == 0 and new["flag"] == 0 and new["approval_height"] > 0:
            kind = "approve"
            if new["commitment"] != b2b(new["recipient"] + new["amount"].to_bytes(8, "big")):
                raise Fail("the recorded commitment does not match the recipient and amount")
            if new["amount"] <= 0:
                raise Fail("the recorded amount is not positive")
            deadline = min(deadline, new["approval_height"] + HEIGHT_SLACK)
            if new["approval_height"] > tip:
                raise Fail("the recorded approval height is in the future")
            node.address_of(new["recipient"].hex(), strict=True)
        elif new["flag"] == 2:
            raise Fail("this is a migration approval; this tool does not sign migrations")
        else:
            raise Fail("the register changes match no treasury path this tool signs")
    else:
        ow, nw = old["whitelist"], new["whitelist"]
        rest_same = all(old[k] == new[k] for k in ("migration_height", "migration_target"))
        if (old["flag"] == 2 and new["flag"] == 0 and ow == nw and new["migration_height"] == 0
                and new["migration_target"] == old["migration_target"]):
            kind = "cancel-migration"
            if pays:
                raise Fail("a payment on a path that does not allow one")
        elif old["flag"] == 2 and new["flag"] == 1 and ow == nw and rest_same:
            # The contract allows a freeze while a migration approval is recorded. The approval cannot
            # be executed after it: an unfreeze returns the fund to the normal state.
            kind = "freeze"
            if pays:
                raise Fail("a payment on a path that does not allow one")
        elif not rest_same or new["flag"] == 2 or old["flag"] == 2:
            raise Fail("this touches the migration fields; this tool does not sign migrations")
        elif same and pays:
            kind = "spend"
            if old["flag"] == 1:
                raise Fail("the liquidity fund is frozen; a spend is not possible")
            for o in pays:
                hsh = b2b(bytes.fromhex(o["ergoTree"]))
                if not any(e[0] == hsh and tip > e[1] + TIMELOCK for e in ow):
                    raise Fail(f"payment to {node.address_of(o['ergoTree'])} is not to a matured whitelist entry")
            if acc or other:
                raise Fail("a spend takes the fund box as its only input")
        elif pays:
            raise Fail("a payment on a path that does not allow one")
        elif same:
            if old["flag"] == 1:
                raise Fail("the liquidity fund is frozen; with registers unchanged this is not a valid path")
            kind = "forward" if acc else "consolidation"
        elif ow == nw and new["flag"] == 1 and old["flag"] != 1:
            kind = "freeze"
        elif ow == nw and new["flag"] == 0 and old["flag"] == 1:
            kind = "unfreeze"
        elif old["flag"] == new["flag"] and old["flag"] != 1 and len(nw) > len(ow) and nw[:len(ow)] == ow:
            kind = "whitelist-add"
            added = nw[len(ow):]
            if any(len(e[0]) != 32 or e[1] <= 0 or e[1] > tip for e in added) or len(nw) > MAX_WHITELIST:
                raise Fail("a new whitelist entry is malformed")
            deadline = min(deadline, min(e[1] for e in added) + HEIGHT_SLACK)
        else:
            raise Fail("the register changes match no liquidity fund path this tool signs")
    if kind in ("consolidation", "freeze", "unfreeze", "cancel", "approve", "whitelist-add", "forward",
                "cancel-migration") and left_fund != 0:
        raise Fail("value leaves the fund on a path that should keep it")
    if kind != "forward" and acc:
        raise Fail("accumulation boxes on a path other than a forward")

    name = "Treasury" if fund == "treasury" else "Liquidity fund"
    lines.append(f"{name}: {kind.upper()}")
    lines.append(f"  fund box now      {coins(f['value'])}")
    lines.append(f"  fund box after    {coins(o0['value'])}")
    if acc:
        lines.append(f"  forwarded in      {len(acc)} accumulation boxes, {coins(acc_sum)}")
    for o in pays:
        lines.append(f"  PAYS OUT          {coins(o['value'])} to {node.address_of(o['ergoTree'])}")
    if kind == "approve":
        lines.append(f"  records a payment of {coins(new['amount'])} to {node.address_of(new['recipient'].hex())}")
        lines.append(f"  it can be paid from height {new['approval_height'] + TIMELOCK + 1} to {new['approval_height'] + TIMELOCK + WINDOW}")
    if kind == "cancel":
        if old["flag"] == 2:
            lines.append(f"  cancels a MIGRATION approval (target script hash {old['commitment'].hex()})")
        else:
            lines.append(f"  cancels the recorded payment of {coins(old['amount'])} to {node.address_of(old['recipient'].hex())}")
    if kind == "cancel-migration":
        lines.append(f"  cancels a MIGRATION approval recorded at height {old['migration_height']} "
                     f"(target script hash {old['migration_target'].hex()})")
    if kind == "unfreeze" and fund == "treasury" and new["approval_height"] > 0:
        lines.append(f"  note: a recorded item remains (recorded at height {new['approval_height']}). "
                     "If it was a migration approval, it can only be cancelled.")
    if kind == "whitelist-add":
        added = [hsh for hsh, _ in new["whitelist"][len(old["whitelist"]):]]
        expected = {b2b(bytes.fromhex(node.tree_of(x))): x for x in (expect or [])}
        for hsh, hh in new["whitelist"][len(old["whitelist"]):]:
            who = expected.get(hsh)
            label = f"{who} (hash {hsh.hex()})" if who else f"hash {hsh.hex()}" + (" NOT among --expect-dest" if expect else "")
            lines.append(f"  adds destination {label} usable from height {hh + TIMELOCK + 1}")
        dest_ok = bool(expected) and all(h in expected for h in added) and all(h in added for h in expected)
        if expect:
            missing = [x for h, x in expected.items() if h not in added]
            lines.append("  --expect-dest: every added destination is one you named, and every one you named is added"
                         if dest_ok else "  --expect-dest: DOES NOT MATCH the added destinations"
                         + (f"; not added: {', '.join(missing)}" if missing else ""))
        else:
            lines.append("  (a hash, not an address: name the agreed addresses with --expect-dest ADDRESS, or compare with "
                         "`fundsign.py hash ADDRESS`. Entries can never be removed.)")
    lines.append(f"  miner fee         {coins(fee)}, paid by {'the fund' if not other else node.address_of(other[0]['ergoTree'])}")
    for o in change:
        lines.append(f"  change            {coins(o['value'])} back to the fee payer")
    left = deadline - tip
    lines.append(f"  must be in a block by height {deadline} (chain is at {tip}; {left} blocks left)")
    if left <= 0:
        raise Fail("\n".join(lines) + "\nthis proposal has expired; build it again")
    return lines, {"fund": fund, "kind": kind, "boxes": boxes, "n_signed": 1 + len(other), "acc": acc,
                   "deadline": deadline, "keys": signer_keys(f["ergoTree"]), "dest_ok": dest_ok}


# ---------------------------------------------------------------- signing

def stand_in(tx, info):
    """The node's commitment and hint-extraction calls fail on a transaction that has an input
    needing no signature (accumulation boxes). Both calls are made on this stand-in instead:
    the same transaction without those inputs, with their value taken off output 0. It keeps the
    fund box at input 0 and the fee input at input 1, so hints carry the same input numbers, and
    it reduces to the same 2-of-3 because a forward leaves the registers unchanged."""
    if not info["acc"]:
        return tx, info["boxes"]
    n = info["n_signed"]
    o0 = dict(tx["outputs"][0])
    o0["value"] -= sum(b["value"] for b in info["acc"])
    return {"inputs": tx["inputs"][:n], "dataInputs": [], "outputs": [o0] + tx["outputs"][1:]}, info["boxes"][:n]


def secrets_arg(a):
    """Devnet only: a signer key supplied from a file instead of the node wallet. main() refuses
    the option unless YOLO_DEVNET=1 is set."""
    if not getattr(a, "devnet_secret_file", None):
        return {}
    if os.environ.get("YOLO_DEVNET") != "1":
        raise Fail("--devnet-secret-file is for a devnet only; set YOLO_DEVNET=1 to use it")
    with open(a.devnet_secret_file) as f:
        return {"secrets": {"dlog": [f.read().strip()]}}


def my_key(node, tx, boxes, keys, extra):
    """Which of the three signer keys this node can sign with, and its commitments."""
    stx, sboxes = tx, boxes
    c = node.call("/wallet/generateCommitments",
                  dict({"tx": stx, "inputsRaw": [node.raw(b["boxId"]) for b in sboxes], "dataInputsRaw": []}, **extra))
    mine = [h["pubkey"]["h"] for h in c["publicHints"].get("0", []) if h.get("hint") == "cmtReal"]
    mine = [k for k in mine if k in keys]
    if len(mine) != 1:
        raise Fail("this node's wallet does not hold exactly one of the three signer keys" if not mine
                   else "this node holds more than one signer key; two different people must sign")
    return mine[0], c


def confirm(lines, a, verb):
    print("\n".join(lines))
    if a.yes:
        return
    if not sys.stdin.isatty():
        raise Fail(f"refusing to {verb} without confirmation; rerun with --yes after reading the summary")
    if input(f"\nType YES to {verb}: ").strip() != "YES":
        raise Fail("stopped")


def write_json(path, obj, private=False):
    """A private file is created new, owner-only, and never through a symlink or over an existing file."""
    if private:
        flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0)
        try:
            fd = os.open(path, flags, 0o600)
        except FileExistsError:
            raise Fail(f"{path} already exists; refusing to overwrite it")
        if hasattr(os, "fchmod"):
            os.fchmod(fd, 0o600)
    else:
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o644)
    with os.fdopen(fd, "w") as f:
        json.dump(obj, f, indent=1)


def load(path):
    with open(path) as f:
        return json.load(f)


def base_name(path):
    return path[:-len(".proposal.json")] if path.endswith(".proposal.json") else path


def cmd_build(chain, a):
    p = build(chain, a)
    lines, _ = describe(chain, p)
    out = a.out or f"{a.fund}-{a.action}-{p['built_at_height']}.proposal.json"
    write_json(out, p)
    print("\n".join(lines))
    print(f"\nwrote {out}\nnext: fundsign.py commit {out}")


def require_dests(info):
    """A whitelist-add is co-signed or finished only when --expect-dest names exactly the added destinations."""
    if info["kind"] == "whitelist-add" and not info["dest_ok"]:
        raise Fail("refusing: for a whitelist-add, pass --expect-dest ADDRESS for every destination agreed by "
                   "another channel; every added hash must be one of them and every one of them must be added")


def cmd_show(chain, a):
    lines, info = describe(chain, load(a.proposal), a.expect_dest)
    print("\n".join(lines))
    print("  signer keys in the fund script: " + ", ".join(k[:10] + "..." for k in info["keys"]))


def cmd_commit(chain, a):
    base = base_name(a.proposal)
    spath = base + ".secret.json"
    if os.path.lexists(spath):
        raise Fail(f"{spath} already exists; a commitment is used once. Finish that round, or delete the file "
                   "and start again")
    p = load(a.proposal)
    lines, info = describe(chain, p, a.expect_dest)
    confirm(lines, a, "commit to this transaction")
    stx, sboxes = stand_in(p["tx"], info)
    key, c = my_key(chain.node, stx, sboxes, info["keys"], secrets_arg(a))
    h = tx_hash(p["tx"])
    write_json(spath, {"proposal": h, "signer": key, "secretHints": c["secretHints"],
                       "publicHints": c["publicHints"]}, private=True)
    write_json(base + ".commit.json", {"proposal": h, "signer": key, "publicHints": c["publicHints"]})
    print(f"\nwrote {base}.commit.json  (send this and the proposal to the second signer)")
    print(f"wrote {base}.secret.json  (KEEP PRIVATE. Use once, for this proposal only; `finish` deletes it)")


def cmd_cosign(chain, a):
    p, c = load(a.proposal), load(a.commit)
    if c["proposal"] != tx_hash(p["tx"]):
        raise Fail("the commit file is for a different transaction")
    lines, info = describe(chain, p, a.expect_dest)
    if c["signer"] not in info["keys"]:
        raise Fail("the commit file is not from one of the three signers")
    require_dests(info)
    extra = secrets_arg(a)
    stx, sboxes = stand_in(p["tx"], info)
    key, mine = my_key(chain.node, stx, sboxes, info["keys"], extra)
    if key == c["signer"]:
        raise Fail("this node holds the same signer key as the proposer; a second signer must co-sign")
    if any(hn.get("hint") == "cmtReal" for hn in mine["publicHints"].get("1", [])):
        raise Fail("this node's wallet can sign the fee input, so co-signing would pay the fee from your own box; "
                   "the proposer must pay the fee from their own box")
    confirm(lines, a, "co-sign this transaction")
    node = chain.node
    partial = node.call("/wallet/transaction/sign", dict(
        {"tx": p["tx"], "inputsRaw": [node.raw(b["boxId"]) for b in info["boxes"]], "dataInputsRaw": [],
         "hints": {"secretHints": {}, "publicHints": c["publicHints"]}}, **extra))
    out = base_name(a.proposal) + ".partial.json"
    write_json(out, {"proposal": c["proposal"], "cosigner": key, "tx": partial})
    print(f"\nwrote {out}  (send it back to the proposer; it is not valid until they finish it)")


def cmd_finish(chain, a):
    p, part = load(a.proposal), load(a.partial)
    base = base_name(a.proposal)
    spath = a.secret or base + ".secret.json"
    sec = load(spath)
    h = tx_hash(p["tx"])
    if part["proposal"] != h or sec["proposal"] != h:
        raise Fail("the partial or secret file is for a different transaction")
    lines, info = describe(chain, p, a.expect_dest)
    keys = info["keys"]
    if part["cosigner"] not in keys or part["cosigner"] == sec["signer"]:
        raise Fail("the co-signer is not a second fund signer")
    pt = part["tx"]
    if [i["boxId"] for i in pt["inputs"]] != [i["boxId"] for i in p["tx"]["inputs"]]:
        raise Fail("the partial transaction has different inputs")
    require_dests(info)
    confirm(lines, a, "sign" + (" and broadcast" if a.send else ""))
    extra = secrets_arg(a)
    # The commitment secret is single use: delete it before any signing call, so that whatever
    # happens next it can never sign a second, different transaction.
    os.remove(spath)
    node = chain.node
    try:
        stx, sboxes = stand_in(p["tx"], info)
        n = len(stx["inputs"])
        ex = node.call("/wallet/extractHints", {
            "tx": {"inputs": pt["inputs"][:n], "dataInputs": [], "outputs": stx["outputs"]},
            "real": [{"op": -51, "h": part["cosigner"]}],
            "simulated": [{"op": -51, "h": k} for k in keys if k not in (part["cosigner"], sec["signer"])],
            "inputsRaw": [node.raw(b["boxId"]) for b in sboxes], "dataInputsRaw": []})
        pub = {}
        for src in (sec["publicHints"], ex["publicHints"]):
            for k, v in src.items():
                pub.setdefault(k, []).extend(v)
        tx = node.call("/wallet/transaction/sign", dict(
            {"tx": p["tx"], "inputsRaw": [node.raw(b["boxId"]) for b in info["boxes"]], "dataInputsRaw": [],
             "hints": {"secretHints": sec["secretHints"], "publicHints": pub}}, **extra))
        node.call("/transactions/check", tx)   # raises if this node would not accept it
    except (Fail, ValueError, KeyError, TypeError) as e:
        raise Fail(f"{e}\n{spath} was deleted before signing and cannot be used again: start a fresh round "
                   "(commit again, then a new cosign)")
    out = base + ".signed.json"
    write_json(out, tx)
    print(f"\nsigned and accepted by this node's checks: {tx['id']}\nwrote {out}; {spath} was deleted before signing")
    if a.send:
        print("broadcast:", node.call("/transactions", tx))
    else:
        print(f"not broadcast. To send it: fundsign.py send {out}")


def cmd_send(chain, a):
    tx = load(a.signed)
    chain.node.call("/transactions/check", tx)
    print("broadcast:", chain.node.call("/transactions", tx))


def cmd_hash(chain, a):
    print(b2b(bytes.fromhex(chain.node.tree_of(a.address))).hex())


def cmd_status(chain, a):
    node, tip = chain.node, chain.node.height()
    print(f"chain height {tip}")
    for fund in (["treasury", "lp"] if a.fund == "all" else [a.fund]):
        f = chain.fund_box(fund)
        r = registers(f, fund)
        flag = {0: "normal", 1: "FROZEN", 2: "MIGRATION APPROVED"}.get(r["flag"], f"flag {r['flag']}")
        print(f"\n{'Treasury' if fund == 'treasury' else 'Liquidity fund'}: {coins(f['value'])}, {flag}, box {f['boxId']}")
        if fund == "treasury":
            if r["approval_height"] > 0 and r["flag"] != 2:
                a0 = r["approval_height"]
                print(f"  recorded payment: {coins(r['amount'])} to {node.address_of(r['recipient'].hex())}")
                print(f"  recorded at {a0}; payable from {a0 + TIMELOCK + 1} to {a0 + TIMELOCK + WINDOW}")
            else:
                print("  no payment recorded")
        else:
            print(f"  whitelist: {len(r['whitelist'])} entries")
            for hsh, hh in r["whitelist"]:
                print(f"    {hsh.hex()} usable from height {hh + TIMELOCK + 1}")
        try:
            acc = chain.accumulation_boxes(fund, 10**9, confirm=False)
            print(f"  waiting in accumulation boxes: {len(acc)} boxes, {coins(sum(b['value'] for b in acc))}")
        except Fail as e:
            print(f"  accumulation boxes: not available ({e})")


def parser():
    ap = argparse.ArgumentParser(prog="fundsign.py", description=__doc__.split("\n\n")[0])
    ap.add_argument("--node", default="http://127.0.0.1:9153", help="your own node's API address")
    ap.add_argument("--api-key-file", help="file holding the node API key (or set YOLO_NODE_API_KEY)")
    sub = ap.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("status", help="show both funds")
    s.add_argument("fund", nargs="?", default="all", choices=["all", "treasury", "lp"])
    b = sub.add_parser("build", help="build an unsigned transaction (proposer)")
    b.add_argument("fund", choices=["treasury", "lp"])
    b.add_argument("action", choices=["forward", "approve", "execute", "cancel", "whitelist-add", "spend", "freeze", "unfreeze"],
                   help="cancel: treasury clears a recorded payment or migration approval; lp clears a migration approval")
    b.add_argument("--to", help="recipient address (approve, spend)")
    b.add_argument("--amount", help="amount in YOLO (approve, spend)")
    b.add_argument("--dest", action="append", default=[], help="address to add to the whitelist (repeatable)")
    b.add_argument("--max-boxes", type=int, default=50, help="accumulation boxes per forward (default 50)")
    b.add_argument("--min-box", default="0.01",
                   help="forward: skip accumulation boxes below this many YOLO (default 0.01, below every block share)")
    b.add_argument("--fee", default="0.002", help="miner fee in YOLO (default 0.002)")
    b.add_argument("--fee-box", help="wallet box id that pays the fee (default: chosen from the wallet)")
    b.add_argument("--out", help="proposal file to write")
    for name, hlp in (("show", "describe a proposal from your own node's view"),
                      ("commit", "proposer: first signing step"),
                      ("cosign", "second signer: sign a proposal"),
                      ("finish", "proposer: complete the signature")):
        c = sub.add_parser(name, help=hlp)
        c.add_argument("proposal")
        c.add_argument("--expect-dest", action="append", default=[], metavar="ADDRESS",
                       help="whitelist-add: an address agreed by another channel (repeatable); "
                            "cosign and finish refuse unless these are exactly the added destinations")
        if name != "show":
            c.add_argument("--yes", action="store_true", help="do not ask for confirmation")
            c.add_argument("--devnet-secret-file", help="DEVNET ONLY: file with a signer key in hex")
        if name == "cosign":
            c.add_argument("--commit", required=True, help="the proposer's .commit.json")
        if name == "finish":
            c.add_argument("--partial", required=True, help="the second signer's .partial.json")
            c.add_argument("--secret", help="your .secret.json (default: next to the proposal)")
            c.add_argument("--send", action="store_true", help="broadcast when signed")
    sd = sub.add_parser("send", help="broadcast a signed transaction")
    sd.add_argument("signed")
    hs = sub.add_parser("hash", help="whitelist hash of an address")
    hs.add_argument("address")
    return ap


def main(argv=None):
    a = parser().parse_args(argv)
    try:
        if getattr(a, "devnet_secret_file", None):
            if os.environ.get("YOLO_DEVNET") != "1":
                raise Fail("--devnet-secret-file is for a devnet only; set YOLO_DEVNET=1 to use it")
            print("fundsign: WARNING: --devnet-secret-file: the signer key is read from a file, not the node wallet "
                  "(devnet only)", file=sys.stderr)
        if a.cmd == "build":
            a.fee_nano = to_nano(a.fee)
            a.min_box_nano = to_nano(a.min_box)
            if a.action in ("approve", "spend") and not (a.to and a.amount):
                raise Fail(f"{a.action} needs --to and --amount")
            if a.action == "whitelist-add" and not a.dest:
                raise Fail("whitelist-add needs at least one --dest")
        chain = Chain(Node(a.node, a.api_key_file))
        {"status": cmd_status, "build": cmd_build, "show": cmd_show, "commit": cmd_commit, "cosign": cmd_cosign,
         "finish": cmd_finish, "send": cmd_send, "hash": cmd_hash}[a.cmd](chain, a)
    except Fail as e:
        print(f"fundsign: {e}", file=sys.stderr)
        return 1
    except FileNotFoundError as e:
        print(f"fundsign: file not found: {e.filename}", file=sys.stderr)
        return 1
    except (ValueError, KeyError, TypeError) as e:
        print(f"fundsign: malformed file or node reply ({type(e).__name__}: {e})", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
