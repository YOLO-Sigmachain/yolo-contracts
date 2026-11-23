# Solo mining YOLO: node, stratum bridge, GPU miner

**Status: 22 Nov 2026.** Written from runs on private test chains, including a GPU run. There is no public network, binary or seed list yet.

## What you run

Three programs, all on your own machines:

1. **The YOLO node.** It follows the chain, builds the block you are trying to mine, and pays the reward to your key.
2. **A stratum bridge.** GPU miners speak the stratum protocol, the node does not. The bridge sits between them.
3. **A GPU miner.** A miner that supports Ergo's `autolykos2` algorithm. See the tested list in step 3.

The node and the bridge can share one machine. The miner can be on that machine or on another one on the same network.

YOLO uses the same proof-of-work as Ergo (Autolykos v2), so Ergo GPU miners can mine YOLO. Not all of them run well on it; see the tested list in step 3. One detail differs: the node hands the miner a height value equal to the block height divided by 4. The miner needs no setting for this; it uses the value the bridge sends.

## What you need

- Java 11 or newer, for the node.
- Node.js, for the bridge.
- An NVIDIA or AMD GPU that can mine Ergo. On the devnet the table size is the starting size, 2^26 entries, the same as Ergo's at launch.
- About 2.5 GB of free memory for the node.

## 1. Node

The node jar and the network settings file are not published yet. Until they are, the node can only be built from its source.

Your config file must include the network settings file, for example with a first line `include "<network-config>.conf"`. Then add a mining section:

```hocon
ergo {
  node {
    mining = true
    useExternalMiner = true
    internalMinersCount = 0
  }
}
scorex {
  restApi {
    bindAddress = "127.0.0.1:9153"
  }
}
```

Keep the API on `127.0.0.1`. Nothing outside your machine needs to reach it.

Start the node:

```
java -Xmx1500m -jar <node-jar> --testnet -c your.conf
```

`<node-jar>` is the node jar file; its release name is not fixed yet.

The node wallet routes need the API key. The default API key hash is the upstream Ergo default, so set your own `scorex.restApi.apiKeyHash` in your config.

Leave `ergo.node.maxTransactionCost` at the value in the network settings file, 1,000,000. If you set it above the room a full block leaves for one pool transaction (1,049,998 at the launch block cost limit), the node logs a warning at start, checks again whenever a vote changes the block cost limit, and uses the lower value anyway.

Then set up the node wallet (`/wallet/init` or `/wallet/restore` through the node's API, as on Ergo) and write down the mnemonic. Block rewards go to the first key of this wallet. Check which key that is:

```
curl http://127.0.0.1:9153/mining/rewardPublicKey
```

Wait until the node is synced. `curl http://127.0.0.1:9153/info` shows `fullHeight` equal to `headersHeight` when it is.

## 2. Stratum bridge

The bridge tested here is [mhssamadani/ErgoStratumServer](https://github.com/mhssamadani/ErgoStratumServer) at commit `fd475d6`. It ran against the YOLO node without source changes on a chain that already had blocks.

```
git clone https://github.com/mhssamadani/ErgoStratumServer
cd ErgoStratumServer && npm install
```

Create a start file next to that folder, `start-yolo.js`:

```js
var Stratum = require('./ErgoStratumServer/lib');

var pool = Stratum.createPool({
    "coin": { "name": "YOLO", "symbol": "YOLO", "algorithm": "blake", "reward": "POW" },
    "extraNonce1Size": 1,
    "address": "YOUR_YOLO_ADDRESS",
    "blockRefreshInterval": 500,
    "jobRebroadcastTimeout": 60,
    "connectionTimeout": 600,
    "banning": { "enabled": false },
    "ports": {
        "3052": { "diff": 8, "multiplyDifficulty": true }
    },
    "daemons": [ { "host": "127.0.0.1", "port": 9153, "user": "unused", "password": "unused" } ],
    "p2p": { "enabled": false }
}, function (ip, port, workerName, password, callback) {
    callback({ error: null, authorized: true, disconnect: false });
});

pool.on('share', function (isValidShare, isValidBlock, data) {
    if (isValidBlock) console.log('Block found');
});
pool.on('log', function (severity, logKey, logText) {
    console.log(severity + ': [' + logKey + '] ' + logText);
});
pool.start();
```

Notes on these settings:

- **`address`** is only checked against the node when the bridge starts. It does not decide where rewards go. Rewards follow the node wallet's key (step 1).
- **`diff: 8`** makes a share 8 times easier than a block, so the miner reports a share every few seconds and you can see it working. It does not change your chance of finding a block.
- **`blockRefreshInterval: 500`** asks the node for new work twice a second. With 30 second blocks, slow polling wastes hashrate on old work.
- **Port 3052** is the port miners connect to. The bridge listens on every network interface, so open this port only to your own network, never to the internet.

Start it:

```
node start-yolo.js
```

## 3. GPU miner

Tested on 19 and 20 Nov 2026 with one NVIDIA RTX 3080 on a private test chain, through the bridge above. That chain ran on the node's older settings: a 128-block difficulty period and a 5 minute timestamp limit. SOAT then mined 3,370 blocks on a second test chain at the launch settings on 23 and 24 Nov 2026, with no block rejected.

| Miner | Source | Result |
|---|---|---|
| [SOAT Miner](https://github.com/blindrun/soat-miner) 0.2.18 | Open, MIT, no fee | Steady at about 150 MH/s. After the first difficulty change at block 257, about 1,670 blocks mined at an average of 29.7 seconds, no rejected blocks. Recommended |
| Rigel 1.19.1 | Closed | Shows about 190 MH/s, but drops to about 30 MH/s for minutes at a time on YOLO. Not recommended until that is understood |
| Other Ergo miners | | Not tested yet |

SOAT, on Windows (the Linux build takes the same options):

```
soat-miner.exe --pool BRIDGE_IP:3052 --wallet LABEL --worker rig1
```

- `BRIDGE_IP` is the address of the machine running the bridge (`127.0.0.1` if it is the same machine).
- Use `--pool`. The `--node` and `--port` options are a different mode that talks to a node directly, and they fail against the bridge. That direct mode has not been tested on YOLO.
- `LABEL` is only a login name. The bridge pays your node's wallet whatever you put here. SOAT 0.2.18 refuses a label unless it starts with `9` and is 40 to 60 characters long, which is the shape of an Ergo address, so a YOLO address is refused. Until that is changed in SOAT, use any text of that shape. SOAT's `--lithos` option is meant to skip the check; it has not been tested here.
- SOAT was tested on an NVIDIA card. It uses a different code path on AMD cards, which has not been tested.

If the miner is on another machine, allow that machine through the firewall of the bridge machine on TCP port 3052.

## How to tell it is working

- The bridge log shows the miner authorising, then accepted shares, then "Block found".
- `curl http://127.0.0.1:9153/info` shows `fullHeight` going up.
- The node wallet shows the rewards. A mined reward cannot be spent for 2,880 blocks (about one day).
- `curl http://127.0.0.1:9153/mining/rewardAddress` shows `coinbaseRewardAddress`, the address block rewards are paid to. `rewardAddress` is the 720-block address used for fees.

Each block pays the miner 93% of the block reward. The other 7% goes to the treasury (4%) and liquidity fund (3%) inside the same transaction; the node builds this for you. In the first year the block reward is 75 YOLO, so the miner's part is 69.75 YOLO per block.

## What to expect

Your share of blocks is your hashrate divided by the network hashrate. The network targets one block every 30 seconds, 2,880 a day. Solo mining pays nothing between blocks, so with a small share of the network, days can pass without a reward.

## Known issues

- **Mining the very first block of a new chain.** Before block 1 exists the node reports no difficulty, and the bridge stops at start with `Cannot read properties of null`. This only affects whoever mines block 1. The fix is one line in the bridge's `lib/pool.js`: change `if (typeof(difficulty) == 'object')` to `if (difficulty !== null && typeof(difficulty) == 'object')`.
- **A chain that sits idle for hours.** On a small or private chain, if no block is found for more than about 200 minutes, the chain can stop advancing until the node is restarted. Restart the node before mining again after a long stop. Seen on a private test chain; a network with steady mining does not reach it.
- **The first difficulty change comes late.** Difficulty is adjusted once per period. Block heights start at 1, not 0, so the first adjustment needs two full periods to measure, and until then every block uses the starting difficulty. The node now uses a 64-block period: the first change is at block 129, so blocks 1 to 128 of a new chain all run at the starting difficulty. Tested on a GPU test chain on 23 and 24 Nov 2026: the first change came at block 129, and difficulty changed at every period boundary after it, 51 changes in all through block 3,329. With one miner the difficulty moved between 3.7 and 5.8 billion from period to period, and block time averaged 31 seconds.
- **Keep the clock of the node machine correct.** The node refuses a block whose timestamp is more than 2 minutes ahead of its own clock. Ergo allows 10 block intervals. The GPU test chain of 23 and 24 Nov 2026 ran with the 2 minute limit and refused no block.
- **Late solutions in the node log.** If your miner finds a solution just after a new block arrives, the node may refuse it and log, at info level, "Solution does not fit the current candidate and there is no previous one, rejecting it". The node keeps its current work and mining goes on.

## Other ways to mine

- **Lithos.** The node has the endpoint Lithos uses (`/mining/candidateWithTxsAndPk`), and it was checked on the devnet: the block paid the key that was asked for. The endpoint needs the API key. It has not been tested with Lithos itself.
- **CPU.** The node's built-in CPU miner (`useExternalMiner = false`) works, at roughly a thousand hashes a second per thread. It is only useful on a private test chain.
