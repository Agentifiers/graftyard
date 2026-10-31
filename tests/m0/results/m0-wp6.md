# M0-WP6: git on Containers and R2 (2026-10-29)

`src/rehydrate.ts` runs as its own script, `graft-m0-rehydrate`, with its own bucket `graft-m0-rehydrate` (created with the enam hint) and one container class, `Shard`, under the `durable_object` scheduling policy. The container image (`b0-22/Dockerfile`) is Alpine with git 2.49 and a small Go server, `b0-22/sink/main.go`, that writes and serves files on the container's disk and runs `git upload-pack`. No other bed shares the script or the bucket, so nothing here creates a site-script version.

## B0-22: rehydrating a 5 GB network from R2

**Seed.** One `Shard` container generated a synthetic network with `git fast-import`: 47,765 random blobs of 20 to 400 KB in 11 packs of at most 512 MB, 5.38 GB with indexes, one ref. Random blobs do not compress or delta, so the bytes on disk equal the bytes copied. The object streamed each `.pack` and `.idx` into the bucket under `net/n5g/` at 85 MB/s (`results/raw/b0-22-seed-5g.json`).

**Run.** Each run uses a new object name, so it starts a new `standard-4` container (4 vCPU, 12 GiB, 20 GB disk) with `enableInternet: false`, as the design requires: bytes reach the container only through its object. The object lists `net/n5g/`, streams every `.pack` and `.idx` from the bucket into the container's disk over `getTcpPort`, `parallel` objects at a time, then runs `git init --bare`, moves the packs in, writes the refs, and runs `git fsck --connectivity-only` and `count-objects`. The time is from the request reaching the object to the end of fsck.

| Run | Hint, container location | Parallel | Container start | Copy | Copy rate | Git setup and fsck | Total |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `enam-p4-1` | enam, mia09 | 4 | 1.7 s | 62.2 s | 86 MB/s | 0.6 s | 64.8 s |
| `enam-p8-1` | enam, mia09 | 8 | 0.4 s | 90.2 s | 60 MB/s | 0.6 s | 91.4 s |
| `rh-enam-p2-1` | enam, mia09 | 2 | 0.4 s | 69.7 s | 77 MB/s | 0.6 s | 70.9 s |
| `rh-enam-p4-2` | enam, ewr12 | 4 | 0.4 s | 91.3 s | 59 MB/s | 0.8 s | 92.6 s |
| `rh-enam-p4-3` | enam, atl13 | 4 | 0.4 s | 60.3 s | 89 MB/s | 0.6 s | 61.4 s |
| `rh-enam-p6-1` | enam, atl13 | 6 | 0.5 s | 63.4 s | 85 MB/s | 0.7 s | 64.7 s |
| `rh-weur-p4-1` | weur, ams19 | 4 | 4.3 s | 46.5 s | 116 MB/s | 0.6 s | 52.0 s |
| `rh-weur-p8-1` | weur, mad05 | 8 | 1.1 s | 116.0 s | 46 MB/s | 0.4 s | 118.1 s |

Every run ended with fsck exit 0, 47,765 objects in 11 packs, and `refs/heads/main` at the seeded tip (`results/raw/b0-22-*.json`).

| Row | Result | Gate | Verdict |
| --- | --- | --- | --- |
| B0-22 | 8 of 8 rehydrations of 5.38 GB at most 118.1 s, median 67.9 s, min 52.0 s | At most 120 s; fail over 600 s | **Pass**, with a thin margin at the tail |

**What limits it.** The copy is 98% of the time. Git's part, with the packs and their indexes already built, is under 1 s, and a container start is 0.4 to 4.3 s. The copy rate ranged from 46 to 116 MB/s, and more streams did not raise it: 8 streams gave both the slowest enam run and the slowest weur run. So the limit is the object's own stream into its container, not R2 reads, which at 4 streams reached 116 MB/s across the Atlantic. Every byte passes through one object because of `enableInternet: false` [inference from the parallel sweep; the object's CPU was not profiled]. Both weur runs read a bucket in enam. A shard rehydrating from its own site's bucket, as the design has it, reads within its region, so the weur rows are a worst case.

**Margin.** The worst run is 2% under the bar. A network over about 6 GB would cross 120 s at the slowest measured rate, and the 600 s fail line holds to about 27 GB at 46 MB/s. A shard serves a cold network only after the copy, so the time is the first fetch's wait after an instance loss (`design/DESIGN.md`, "Failure modes", git caches, instance). Two ways to cut it that this run does not test: rehydrate through more than one object, or start the network from its newest packs and fetch older ones lazily.

**Not tested here.** Real repository content. A clone of torvalds/linux in the container was abandoned after 727 s, when the Worker call lost its connection. Real content would change the git time (deltas and `index-pack` are not needed when `.idx` files are stored) but not the copy, whose bytes are the same either way [inference].

## B0-14: daily working set of git cache shards

The row asks for a synthetic replay. No install exists to replay, so the stand-in is public GitHub activity, for which `run/working-set.py` keeps the method. For 19 large public orgs, the sizes of all 25,556 non-fork repositories (1,217 GB, GitHub's `size` field, read 2026-10-29) were matched against two weekdays of GH Archive events, 2026-07-14 and 07-15 (17,134 events from those orgs). A repository counts toward a day's working set if it had a push, a branch or tag create or delete, or a pull request event that day. The share is the bytes of those repositories over the org's bytes (`results/raw/b0-14.json`).

GH Archive for late October 2026 holds about 3,500 events an hour against 80,000 to 165,000 in earlier months, so it is incomplete for recent days, and the replay uses July.

| Org | Git bytes | Repos | Active a day | Working set |
| --- | --- | --- | --- | --- |
| golang | 1.8 GB | 61 | 5.5 | 67% |
| vercel | 6.1 GB | 202 | 13.5 | 76% |
| kubernetes | 6.8 GB | 80 | 13.5 | 62% |
| openai | 7.0 GB | 244 | 12.5 | 22% |
| cloudflare | 7.4 GB | 503 | 18.5 | 45% |
| nodejs | 9.6 GB | 229 | 10.5 | 20% |
| grafana | 10.4 GB | 441 | 128 | 76% |
| hashicorp | 20.2 GB | 785 | 38 | 61% |
| rust-lang | 20.4 GB | 227 | 19.5 | 14% |
| huggingface | 21.2 GB | 399 | 23.5 | 9% |
| googlecloudplatform | 25.8 GB | 1,523 | 57 | 19% |
| dotnet | 33.1 GB | 262 | 46.5 | 49% |
| aws | 60.7 GB | 556 | 58 | 30% |
| mozilla | 64.1 GB | 2,289 | 35.5 | 16% |
| elastic | 68.0 GB | 781 | 53.5 | 83% |
| azure | 98.5 GB | 2,706 | 103 | 37% |
| google | 122.2 GB | 2,888 | 83.5 | 12% |
| apache | 236.2 GB | 3,159 | 197.5 | 23% |
| microsoft | 398.0 GB | 8,221 | 247.5 | 11% |

Against each tier's git bytes in `scripts/cost_model.py`, taking orgs within a factor of three:

| Tier | Git bytes | `cost.py` guess | Orgs in band | Median | Max |
| --- | --- | --- | --- | --- | --- |
| 50 developers | 30 GB | 30% | 9 | 30% | 83% |
| 300 developers | 300 GB | 20% | 3 | 12% | 23% |
| 2,000 developers | 4,000 GB | 10% | 0 | none | none |

| Row | Result | Gate | Verdict |
| --- | --- | --- | --- |
| B0-14 | The median matches the 50-developer guess, the 300-developer guess sits between the median and the max, and the share falls with org size, which is consistent with the 2,000-developer guess (Microsoft, the largest, is 11 to 12.5%). `cost.py` keeps 30%, 20%, and 10%, so the shard counts stay 2, 5, and 25 | Recorded; shard counts updated | **Recorded**. The shard counts are unchanged |

**Limits.** Public activity is a proxy [inference]. It sees no fetches, so a repository that is only cloned that day is missed. CI fetches follow pushes and pull requests, so the miss is mostly developers' fetches of quiet repositories. Public orgs keep more dead repositories than a company might, which lowers their share. Within one tier the spread is wide: at 50 developers, the 83% org would need 4 `standard-2` shards per site rather than 2. A working set over capacity is not an outage, because the LRU evicts and the evicted network rehydrates on its next fetch, at B0-22's 52 to 118 s for 5 GB. So the design's rule stands: each install counts its shards from its own measured working set ("Deployment modes", git caches), and these numbers only set the defaults.

## B0-23: clone cache under CI load

**Bed.** The same script. One `Shard` object, `cc-mono`, with the enam hint, holds a synthetic monorepo on its container's disk: a first commit of 20,000 text files of 1 to 4 KB in 40 services, then 300 commits that each change 3 files, 43 MB packed with bitmaps (`sink mono`). Each tip is one CI checkout target. A depth-1 protocol v2 fetch of a tip is 40.8 MB. `index-pack` accepts the bytes the cache serves, and the tip's tree has all 20,000 files (checked once by hand).

**Edge path** (`cloneCache` in `src/rehydrate.ts`). The key is a SHA-256 of (network, wanted tip, depth, capabilities, run), which is `CloneKey` plus a run salt so each run starts cold. A clone reads one of 8 replicas at random and falls back to replica 0. On a miss it calls the network's shard object, which forwards the fetch request to the container. The container runs one `git upload-pack` per key into a file and streams that file to every request for the key, the first one and any that arrive while it runs (`/clone` in `b0-22/sink/main.go`). When the pack is complete, the object writes it to the bucket as replica 0, then the other 7. So a tip costs one `upload-pack` however many clones miss on it at once.

**Load.** 11 load clients, `Shard` objects created with the enam, wnam, and weur hints (they landed in 7 to 9 colos across the US and Europe). Each runs 3 clones a second from its alarm against the script's public hostname, as a CI runner would, so 33 clones a second in all. Each clone reads the first 1 MiB and closes. The tip moves every 6 s, which is 10 times a minute. A clone wants the tip as of a random delay, the time from a push to its CI job's checkout. Time to first byte is from the client's request to the first body byte (`run/clone-cache.ts`; raw rows in `results/raw/b0-23-*.json`).

Three things vary by run: the delay from a push to its clones, whether a push hook fills the new tip's entry when the tip appears (`--prefill`: client 0 asks the shard for the tip at once), and whether a miss claims the key first (`--claim`: a create-only claim object in the bucket, and only the edge that creates it calls the shard, while the others poll for replica 0). "Steady" excludes the first 90 s, when every client starts on tips not yet built, which a running install never sees. TTFB is time to first byte.

| Run | Clone delay after the push | Push hook | Claim | Steady clones | Failed | Served from R2 | No `upload-pack` of its own | TTFB p50 | TTFB p99 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `L1` | 5 to 60 s | no | no | 16,830 | 0 | 97.8% | 99.5% | 145 ms | 491 ms |
| `L2-r1` (1 replica, not 8) | 5 to 60 s | no | no | 15,300 | 0 | 97.5% | 99.5% | 100 ms | 358 ms |
| `L3-claim` | 5 to 60 s | no | yes | 11,855 | 0 | 97.4% | 99.5% | 123 ms | 1,559 ms |
| **`Z3-burst-prefill`** | **4 to 8 s** | **yes** | no | **16,830** | **0** | **100.0%** | **100.0%** | **120 ms** | **406 ms** |
| `Z1-prefill` | 0 s | yes | no | 16,830 | 34% | 45% | 66% | 255 ms | 1,852 ms |
| `Z2-wait` (over 8 streams a key wait) | 0 s | no | no | 16,830 | 44% | 42% | 56% | 221 ms | 4,049 ms |
| `Z4-claim` | 0 s | no | yes | 14,417 | 0.6% | 66% | 98.8% | 372 ms | 41,708 ms |
| `Z5-claim` (33 clients at 1/s) | 0 s | no | yes | 16,830 | 0 | 63% | 99.5% | 268 ms | 3,446 ms |

Including the first 90 s, `L1` and `L2-r1` failed 4.3% and 3.9%, all in that window, and `Z3` failed 2.3%, all before its first tip was filled. Steady-state rows are in the raw files; `.gen/b0-23-steady.py` recomputes them. The claim runs (`L3`, `Z4`) also skipped 6,055 and 2,953 load slots, which they recorded rather than sent, because a load client fell more than 5 s behind its schedule. `Z4`'s 41 s tail is one stall between 330 and 420 s, when a fill failed and the waiting clones held their claims until they went stale. One `L2-r1` client in LAX was evicted and woke 10 minutes late; its 1,800 late rows are excluded, and clients now skip missed slots instead.

**Why misses fail.** Every failure is the shard object refusing work: "Durable Object is overloaded. Requests queued for too long", or "Too much data queued". A miss goes through the one object for its network, and that object relays the 40 MB response. A tip's pack builds in about 0.4 s and reaches the bucket's first replica 1.5 to 2.4 s later. At 33 clones a second, every clone in that window is a miss on one object, which is 50 to 80 concurrent 40 MB streams, past what the object accepts. Limiting streams per key inside the object (`Z2`) does not help, because the refusal happens in the object's queue before its code runs. A claim (`Z4`, `Z5`) keeps the object to one request per key and removes the failures, but the clones that wait for the bucket take 1.5 to 3.4 s to their first byte.

**What passes.** A push hook that asks the shard to fill the new tip's entry when the push commits, with CI clones starting 4 s or more after the push (`Z3`). Each tip then costs one `upload-pack`, no clone waits, and 100% are served from R2. With clones 5 to 60 s after the push and no hook (`L1`, `L2-r1`), the tip's first clone misses and fills the entry, as the design describes, and the rest hit. A CI job starts after a webhook and a runner pickup, which is seconds [inference], so the 0 s runs are a stress case rather than the gate's load. Writing 8 replicas made no difference to read latency against 1 replica at 33 GETs a second on one key (`L2-r1`).

| Row | Result | Gate | Verdict |
| --- | --- | --- | --- |
| B0-23 | Push hook, clones 4 to 8 s after the push: 100% served from R2, TTFB p99 406 ms, no failures in the steady state. Clones 5 to 60 s after, no hook: 97.5 to 97.8%, p99 358 to 491 ms. Clones at the instant of the push, no hook: up to 44% refused by the shard object | Hit rate at least 95%, TTFB p99 at most 500 ms; fail under 80% | **Pass**, with the push hook or a few seconds of CI lag. Without either, a herd at a new tip fails, so the design adds the hook |

## B0-20 and B0-21: push size and chunked pushes

**Plans.** The Milestone 0 account has no zone, so it has no Pro, Business, or Enterprise plan. Per the build plan, rows on a plan the account lacks are recorded as not run. What can run is the same test against the script's workers.dev hostname, whose body limit is 100 MB, the Free and Pro value [inference: the cutoff measured exactly 100 MiB]. Raw notes are in `results/raw/b0-21-workersdev.txt`.

**Chunked bodies (B0-21's question, C10).** A `PUT` that streams its body into R2: 100 MiB passes and 101 MiB fails, with or without `Content-Length`. Without it (`Transfer-Encoding: chunked`), the 413 comes after about 1.6 s, once the client has sent past 100 MiB. With it, the 413 comes in 0.17 s, before the body. So the limit counts chunked bodies as they stream, and a chunked push cannot pass the plan cap.

**The edge's own cap (B0-20's Pro and Business half).** `/push/<cap>/r.git` in `src/rehydrate.ts` is a minimal smart-HTTP receive-pack. It counts the body, stops reading at its cap, and answers 200 with a side-band `report-status` that names SSH. With git 2.56:

- A 30 MB push against a 20 MB edge cap prints `remote: Push refused: over 20 MB ... Push over SSH` and `! [remote rejected] small -> main (push over HTTPS cap, use SSH)`. That is the design's behavior.
- A 150 MB push against the same 20 MB cap prints `error: RPC failed; HTTP 413 curl 22` and `send-pack: unexpected disconnect`. That is the gate's fail value, a bare 413.

The edge's answer does not help, because git keeps sending: it reads the response only after it has written the whole body, so the body passes 100 MiB first and Cloudflare replaces the Worker's response with its own 413 page. A raw client that stops at the first response byte still got the 413 once it had sent 102 MB or more. Any body at most 100 MiB got the edge's message.

| Row | Result | Gate | Verdict |
| --- | --- | --- | --- |
| B0-20, Pro and Business half | On the 100 MB workers.dev limit, a push over the plan limit shows git a bare `HTTP 413`, whatever cap the edge sets below it. Only pushes between the edge's cap and the plan limit see the message naming SSH | Prints the edge's `report-status` naming SSH; fail on a bare 413 | **Fail on workers.dev** for pushes over the plan limit. Pro and Business zones not run |
| B0-20, Enterprise half | No Enterprise zone | Completes with Worker memory under 100 MB | **Not run** |
| B0-21 | On workers.dev, chunked bodies are cut at the same 100 MiB as sized ones. Pro, Business, and Enterprise not run | Recorded | **Recorded** for the 100 MB limit |

**What it changes.** The design said a push over the plan limit gets the edge's message naming SSH. It cannot, so `design/DESIGN.md` journey 1, step 2 now says the edge's message reaches only pushes between its cap and the plan limit, and that git shows a bare 413 above that. The operator left the choice to the run, which accepted the 413 and has the install docs and the 413 runbook name SSH for large pushes (`decisions/DECISIONS.md`, "Pushes over the plan's body limit get Cloudflare's 413"). A zone on each plan, added to this account, would let the plan rows run. Whether Business's 200 MB and Enterprise's limits behave the same way is [unverified].

### B0-20 and B0-21 on a Pro zone (`graftyard.dev`, 2026-10-30)

The operator added `graftyard.dev` on the Pro plan to the Milestone 0 account. `run/zone.ts route` put a proxied `m0.graftyard.dev` and a Worker route on `graft-m0-rehydrate`, and `run/zone.ts push` ran the same cases as on workers.dev (`results/raw/zone-push-pro.json`).

| Case | Result |
| --- | --- |
| `PUT` 99 and 100 MiB, sized and chunked | 200, stored in full (5.8 to 8.3 s) |
| `PUT` 101 MiB, sized | Cloudflare 413 after 4 MB sent (94 ms) |
| `PUT` 101 MiB, chunked | Cloudflare 413 once 101 MB had streamed (1.9 s) |
| Raw receive-pack, 90 MB against an 80 MB edge cap | 200 with the edge's side-band `Push refused ... Push over SSH` |
| Raw receive-pack, 120 MB against the same cap | Cloudflare 413 page |
| `git push` of 90 MB, edge cap 80 MB | `remote: Push refused: over 80 MB ... Push over SSH`, `! [remote rejected] HEAD -> main (push over HTTPS cap, use SSH)` |
| `git push` of 120 MB | `error: RPC failed; HTTP 413 curl 22` |

| Row | Result | Gate | Verdict |
| --- | --- | --- | --- |
| B0-20, Pro | Between the edge's cap and 100 MiB, git prints the edge's `report-status` naming SSH. Over 100 MiB, git shows a bare 413, as on workers.dev | Prints the edge's `report-status` naming SSH; fail on a bare 413 | **Fail as written, for pushes over 100 MiB**, the case `decisions/DECISIONS.md`, "Pushes over the plan's body limit get Cloudflare's 413", already accepts. Its flip ("a Pro or Business zone run where the edge's response reaches git above the plan limit") did not happen |
| B0-21, Pro | Chunked bodies are cut at 100 MiB on Pro, as sized ones are | Recorded | **Recorded**: Pro's push cap is 100 MiB, chunked or not |
| B0-20 and B0-21, Business and Enterprise | No zone on those plans | | **Not run** |

## B0-19: container starts with the control plane out of reach

**Control, tokens active.** `run/container-placement.ts --per 34 --no-summary` started 102 new containers, 34 from new objects created with each of the enam, weur, and apac hints, through `graft-m0-container`. Every one started, every one in its hinted region, at start p50 175 to 279 ms and p99 3.8 to 4.8 s (`results/raw/container-placement-b0-19-control-1.json`).

**Revoked run: not yet run, because it needs the operator.** The row revokes every account API token. The account has one, the one this harness uses (`shiny-wind-2ad7`). It cannot be revoked and then restored from here: a token cannot create another token that holds Account API Tokens Write ("sub-token is not allowed to have permissions to manage other tokens"), and disabling itself leaves nothing that can enable it again. A container start through a Worker uses no API token, so the run needs only that the operator roll or disable the token in the dashboard, let the same 102 starts run, and then restore it. The starts are called with the Worker's own bearer secret, which stays valid. The watcher that would have run them was stopped when the operator skipped the row.

| Row | Result | Gate | Verdict |
| --- | --- | --- | --- |
| B0-19 | Control with tokens active: 102 of 102 started, all in their hinted regions | 100 of 100 start with every account API token revoked | **Skipped** by the operator (2026-10-29, `FRAME.md`). C8 stays [unknown] |
