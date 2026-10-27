**Verdict: candidate 3 should be the base (41.5 of 50).** Candidate 1 is second (35.5) and candidate 2 is far behind (22.5). The AWS reference scores 42.5 but cannot be picked. No Cloudflare fact that a score rests on was refuted, so no criterion is capped. I ran all three cost scripts and `check_cloudflare.py` (all three report OK). I fetched the Cloudflare doc pages as `index.md` into `/tmp/cfj`, plus git's `connect.c` and `transport.c`. Labels: [measured] means I ran or read it, [inferred] is my reasoning, [guess] is an unbacked number.

## Scores

| Criterion (weight) | C1: DO per aggregate + Paired | C2: Artifacts + Repo DO | C3: three-site Raft | AWS `DESIGN.md` |
| --- | --- | --- | --- | --- |
| Downtime (3) | 3 | 2 | 5 | 5 |
| Scale (2) | 4 | 2 | 4 | 4 |
| Correctness (2) | 4 | 3 | 5 | 5 |
| Build and operate risk (2) | 3 | 2 | 2 | 3 |
| Install fit (0.5) | 4 | 2 | 4 | 5 |
| Cost (0.5) | 5 | 3 | 5 | 2 |
| **Weighted total (of 50)** | **35.5** | **22.5** | **41.5** | **42.5** |

**Downtime**
- C3 meets every part of the rubric's 5.
  - An acknowledged write sits on two sites in two regions and two accounts, and losing any one region costs about 15 s of writes [guess].
  - Releases roll one site per day, and followers on release N refuse entries they cannot decode.
  - The 22% DO error class works out to 74% success per attempt and 98% in three [measured: I redid the arithmetic].
  - A Cloudflare-wide outage is stated as exposed. It names what still works: stale reads, and a read-only lifeboat off Cloudflare.
- C1 Paired gives RPO 0 and about 20 s of write recovery when the primary's region is lost. It has one gap, covered under flaws: losing the standby's side has no stated path.
- C2 recovers a lost region only through an operator-signed workflow after a 30-minute wait. Region RTO is "hours" and metadata RPO is seconds. During the 6-hour WEUR DO incident class it waits 30 minutes, then needs a manual recovery.

**Scale**
- C1 and C3 are bounded by about 1,000 requests a second per object and by the 20 GB container disk. That caps a network at 12 GB (C1) or about 15 GB (C3).
  - C1 spreads load with up to 16 `RepoReader`s and 16 `AuthzReplica`s.
  - C3 spreads load with follower reads and 8 replicas of each hot clone-cache key.
- C2 caps a repository at Artifacts' 1 GB (graduating at 768 MB) and a Large network at 8 GB. Managed scale needs Artifacts' 1 TB account cap raised by orders of magnitude, which C2 says itself.

**Correctness**
- C3 commits a merge in one Raft entry. Reads go through a leader lease, and the clock-drift assumption is stated. Writes are bound by revocation immediately, because `decide` reads `Authz` fresh.
- C1's merges are atomic and its reads are exact. Its stamp bound is misstated: it says "one commit's worth of time", but stamps are valid for 30 s.
- C2's recovery has a weakening it does not state (see flaws).

**Build and operate risk**
- C3 owns Raft, leases, joint consensus, and cross-account RPC. It has the most novel code.
- C2 adds the least protocol code but rests on Artifacts (beta, closed source, its cap cut from 10 GB to 1 GB on 2026-09-29), two git engines, and an unverified stage-then-apply trick on Artifacts refs.
- C1 has a smaller home-grown pairing protocol. All three Milestone 0 plans have numeric pass and fail values.

**Install fit**
- C3's self-hosted mode is a quorum across three workerd hosts.
- C1's is a single workerd host.
- C2 has no on-premises mode at all, only `tributary export`.

**Cost** (from the scripts at 50, 300, and 2,000 developers; none includes an Enterprise contract)

| Design | 50 | 300 | 2,000 | Cheapest reliable at 50 |
| --- | --- | --- | --- | --- |
| C1, Paired | $212 | $930 | $5,110 | $80 (Regional only) |
| C2 | $438 | $1,647 | $14,911 | $218 |
| C3 | $180 | $947 | $5,835 | $168, keeping the cross-region quorum |
| AWS, one Region | $1,287 | $2,573 | $12,432 | $226 (`scrappy`) |
| AWS, Paired | $2,292 | $4,466 | $20,636 | $388 (`scrappy-paired`) |

C1 prices 4 `standard-4` git workers for 4 TB of git data. That is probably too few, because every cold network has to be pulled back from R2 [inferred].

## Journeys traced

**Journey 1: atomic three-branch push to a busy monorepo, then a 5 GB clone and a 2 GB push**
- **C1**
  - Path: Worker token cache, then `AuthzReplica` (weur), then the `Repo` primary (enam) for discovery.
  - The pack goes to an R2 staging bucket via Local Uploads, then to a git-worker container for `index-pack`.
  - The pack is placed in the enam and wnam buckets, then `Network` commits `PackRegistered` and replicates to its standby.
  - `Repo` then commits one entry with three updates and replicates it to the wnam standby.
  - Durable at "ok": the entry confirmed in two hints and the pack in two buckets. That is two Paired commits in series per push [inferred].
  - CI clones are served from the R2 clone cache.
  - The 5 GB clone hydrates a `standard-4` (Milestone 0 gate: 60 s). A runtime update can cut it after 20 minutes, so a client below about 4 MB/s is at risk.
  - The 2 GB push goes over HTTPS on an Enterprise zone, or otherwise over SSH into staging. The SSH gateway reaches Durable Objects through the container's outbound handler, and I confirmed that doc.
- **C2**
  - Path: D1 token re-read on the primary, `Owner` DO strong check, then `Repo` DO `admit`.
  - The body streams to the public Artifacts remote as hidden staging refs and is copied to an R2 pack log in wnam. Then the DO commits, and an alarm later runs `apply` to move the real refs in Artifacts.
  - Durable at "ok": objects in Artifacts and the wnam pack log. Refs exist only in the enam DO, and their cross-region copy is the pack-log header.
  - The journey assumes a 600 MB monorepo. Anything past 768 MB graduates to the Large tier, which stops at 8 GB.
  - The 5 GB clone and 2 GB push run on a separate Large-tier host, and the push needs Enterprise.
- **C3**
  - Path: `Tokens` leader read with `fresh`, then `Authz` read with `fresh`.
  - The body streams to the `GitCache` container on the leader's site and is copied to three site buckets (two must finish). After `index-pack`, the leader appends the entry and the first follower ack makes the quorum.
  - Durable at "ok": the entry on two continents and two accounts, the pack in two buckets or inline if it is 1 MiB or less.
  - Copying a 2 GB push into three cross-continent buckets from the edge Worker is gated at Milestone 0 (Worker memory under 100 MB).

**Journey 4: member removed and token revoked, then the member pushes over HTTPS and SSH**
- **C1**
  - The 204 for the removal waits until every replica applies it or its 10 s lease expires.
  - A revoked token can still push for up to 5 s, because the Worker uses a cached token for up to 5 s before re-checking. A stamp minted before the 204 can commit for up to 30 s.
  - The SSH gateway re-checks the key's `Claim` and the stamp the same way.
- **C2**
  - The HTTPS push re-reads the token on the D1 primary, so a revoked token is refused as soon as the 204 is sent.
  - Another token reaches `admit`, whose `Owner` DO check refuses it.
  - A push admitted before the 204 still commits, because `commit` re-checks only the old ref values. That bound (the length of the push) is not stated [inferred].
  - SSH goes Spectrum, then `cloudflared`, then a D1 fingerprint lookup, then the same `admit`. Reads lag up to 40 s.
- **C3**
  - The token is read on its shard leader. `decide` reads `Authz` fresh as a dependency inside the commit, so every write committed after the 204 is refused, over HTTPS and SSH alike.
  - Reads lag up to 10 s.

**Journey 5: the data center, then the region, holding the newest writes is lost mid-push**
- **C1**
  - Data center: the platform fences and restarts the object; restart time is undocumented.
  - Region: Workers report to the wnam standby, which promotes after its lease plus margin and a compare-and-swap on the register in weur. RPO 0, about 20 s [guess].
  - What I found: the reverse case is undefined.
    - The primary may acknowledge and serve reads only while it holds a standby lease (lines 166-167), and only the standby can promote (line 175).
    - So losing the standby's region stops reads and writes on every Paired object paired there, with no automatic path to `Alone` [inferred].
    - The pack-bucket failure row also contradicts the rule that a durable pack must be in every bucket of its copy set.
- **C2**
  - Data center: confirmed DO writes survive; the in-flight intent is abandoned and git's retry succeeds.
  - Region: 30-minute wait, then `RepoRecover` rebuilds into a new generation from the R2 journal and the pack log.
  - Replay applies pack-log records whose old values still match. That can bring back pushes that were staged but never acknowledged, or that `commit` refused, such as a non-fast-forward to a protected ref [inferred, not stated as a weakening].
- **C3**
  - Data center: the leader restarts or a follower wins the election after 10 s.
  - Region: B or C leads in 10 to 15 s, then `ReplaceMember` adds a member in wnam.
  - One slip: it says every acknowledged pack is "in B's bucket". In fact it is in whichever two buckets finished first.

**Journey 6: global outage (November 18, 2025 class) and control-plane outage (November 2, 2023 class)**
- **C1**
  - Global: HTTPS and SSH are down, nothing is lost, and no failover storm starts.
  - Control plane: everything serves. It states openly that new container starts are unknown and sizes warm pools for a day.
- **C2**
  - Global: everything is down, SSH included, because the SSH front calls the Worker through the proxy.
  - Control plane: everything serves, and containers are kept warm.
  - It also states that Artifacts' KV token dependency exposes git to the June 2025 class.
- **C3**
  - Global: the same outage, except installs running a lifeboat serve read-only git and `gh` about 30 s behind. The lifeboat's DNS-only hostname assumes Cloudflare DNS survives [inferred].
  - Control plane: it calls this "removed", which slightly overclaims given that container cold starts are unknown.

## Fact checks

1. DO soft limit of 1,000 requests a second, 10 GB per object, 2 MB row, overloaded errors returned to the caller. **Confirmed.** https://developers.cloudflare.com/durable-objects/platform/limits/index.md
2. DOs do not move after creation, hints are best effort, and the jurisdictions are `eu`, `us`, `fedramp`. **Confirmed.** https://developers.cloudflare.com/durable-objects/reference/data-location/index.md
3. The output gate holds replies until writes are confirmed, and PITR covers 30 days. **Confirmed.** https://developers.cloudflare.com/durable-objects/api/sqlite-storage-api/index.md
4. A write is confirmed after 3 of 5 followers in different data centers. **Unverified by me**: the blog returned 429 and Wayback returned 503. `research/cloudflare-uptime.md` says its archived quote was checked word for word. No page says the followers span regions. https://blog.cloudflare.com/sqlite-in-durable-objects/
5. A DO may be replaced on a partition or update, and code updates roll out eventually consistently. **Confirmed.** https://developers.cloudflare.com/durable-objects/platform/known-issues/index.md
6. Gradual deployments pin each DO to one version and reset it only when it is reassigned. C3's "every deploy exercises election" rests on this. **Confirmed.** https://developers.cloudflare.com/workers/versions-and-deployments/gradual-deployments/with-durable-objects/index.md
7. Alarms run at least once. **Confirmed, with a caveat**: only 6 retries when the handler throws. C1's finishers and C2's `apply` must reschedule themselves. https://developers.cloudflare.com/durable-objects/api/alarms/index.md
8. Workers request body limit of 100/100/200 MB, up to 5 GB on Enterprise; no response limit; 20-minute grace on runtime updates; 128 MB per isolate. **Confirmed.** C1's "at most 6 calls in flight" is only partly right: the 6 counts connections still waiting for headers, and extra calls queue rather than fail. https://developers.cloudflare.com/workers/platform/limits/index.md
9. Containers GA since 2026-04-13; at most 4 vCPU, 12 GiB, 20 GB per instance; 1,500 vCPU per account; ephemeral disk; not guaranteed to run beside their DO. **Confirmed.** https://developers.cloudflare.com/changelog/post/2026-04-13-containers-sandbox-ga/index.md ; https://developers.cloudflare.com/containers/platform/limits/index.md ; https://developers.cloudflare.com/containers/concepts/architecture/index.md
10. Container outbound handlers reach DO and R2 bindings (C1's SSH write path). **Confirmed.** https://developers.cloudflare.com/containers/configuration/workers-connections/index.md
11. Container `jurisdiction` and `regions` constraints. **Confirmed.** https://developers.cloudflare.com/containers/concepts/placement/index.md
12. R2 is strongly consistent and evaluates `If-None-Match`/`If-Match` atomically, with losers getting 412. **Confirmed.** https://developers.cloudflare.com/r2/reference/consistency/index.md
13. An R2 bucket lives in one region, writes are synchronous within it, and "durability is not a guarantee of availability". **Confirmed.** https://developers.cloudflare.com/r2/reference/durability/index.md
14. R2 Local Uploads stay strongly consistent, with an asynchronous copy to the bucket (C1). **Confirmed.** https://developers.cloudflare.com/r2/buckets/local-uploads/index.md
15. The R2 get-bucket API returns `location` (C3). **Confirmed** that the field exists. Whether it reports actual placement or just the hint is unverified. https://developers.cloudflare.com/api/resources/r2/subresources/buckets/methods/get/index.md
16. Queues deliver at least once. **Confirmed.** https://developers.cloudflare.com/queues/reference/delivery-guarantees/index.md
17. D1 replicas "may be arbitrarily out of date", and the Sessions API gives sequential consistency. **Confirmed.** The beta status comes from the research and I did not re-check it. https://developers.cloudflare.com/d1/best-practices/read-replication/index.md
18. Artifacts: 1 GB for new repositories (10 GB for older ones), 2,000 git requests per 10 s, 1 TB per account, no v2 receive-pack, no `filter` or `include-tag`, synchronous multi-data-center replication, "a single logical instance". **Confirmed.** https://developers.cloudflare.com/artifacts/platform/limits/index.md ; https://developers.cloudflare.com/artifacts/api/git-protocol/index.md ; https://developers.cloudflare.com/artifacts/concepts/how-artifacts-works/index.md
19. Artifacts imports only public HTTPS remotes, and the binding mints tokens and forks repositories (C2). **Confirmed.** https://developers.cloudflare.com/artifacts/guides/import-repositories/index.md ; https://developers.cloudflare.com/artifacts/api/workers-binding/index.md
20. Artifacts keeps its auth tokens in KV (C2). **Unverified**: the source is a blog post via a secondary mirror, per the research.
21. Unmodified git never pushes over protocol v2 (C2). **Confirmed** in git's source: `connect.c` line 1459 and `transport.c` line 981 (https://github.com/git/git).
22. Spectrum SSH is one app on Pro and Business; generic TCP needs Enterprise. **Confirmed.** https://developers.cloudflare.com/spectrum/protocols-per-plan/index.md
23. A Tunnel virtual-network origin needs a TCP/UDP app with exactly one address. **Confirmed.** Whether a Pro SSH app may use it, and whether `cloudflared` inside a container can be the connector, is **unverified**. All three SSH paths depend on this. https://developers.cloudflare.com/spectrum/reference/configuration-options/index.md
24. `/cdn-cgi/trace` names the serving data center. **Confirmed.** That a DO's subrequest reports the DO's own colo (C3's placement check) is **unverified**. https://developers.cloudflare.com/fundamentals/reference/cdn-cgi-endpoint/index.md
25. DO bindings do not cross accounts (C3). **Unverified**; C3 labels it an inference.

## Most serious flaw in each design
- **C3 (winner):** Its region-loss and continent-loss guarantees rest entirely on a Raft, lease, and joint-consensus implementation the team must write.
  - Its members talk across accounts over public HTTPS through the site Workers, so a partial proxy failure like December 2025 would also hit replication [inferred].
  - Its region-separation check depends on fact 24, which is unverified.
  - All three sites still share Cloudflare's DO storage layer, so the June 2025 class is only mitigated.
- **C1:** Losing the standby's region leaves no path forward. The primary can neither renew its lease nor go `Alone`, so every Paired object paired there stops reads and writes, and the design does not say so.
- **C2:** The design depends on a closed beta product that cut its per-repository cap tenfold a month ago. Region recovery is manual and takes hours, and its pack-log replay can bring back pushes that were never acknowledged or that the commit check rejected.
- **AWS:** It builds a fold engine, Rust receive-pack, cells, and an in-account operator role, at 2.5 to 10 times the list cost of C3.

## What each loser does better than C3
- **C1**
  - It is one account, with members reached through DO bindings rather than public HTTPS.
  - Its pairing protocol is smaller than Raft.
  - Its default Paired topology is enam and wnam, two regions on one continent, so writes are faster than Tri's intercontinental round trip.
  - It offers a Regional durability dial: $80 cheapest, $5,110 at 2,000 developers.
  - Up to 16 `AuthzReplica`s per huge org.
  - Its SSH writes reach Durable Objects through the container outbound handler, which I confirmed in the docs.
  - It states more honestly that container starts during a control-plane outage are unknown.
- **C2**
  - It writes no consensus code.
  - When R2 is down it still commits pushes of any size (C3 only up to 1 MiB).
  - The pack log gives a ready exit path for every repository.
  - It checks against git's source that Artifacts' v1-only push is invisible to clients.
  - The merge-queue `parent_tip` invariant.
  - Its SLOs are the most conservative of the three.

## Recommendation
Use **candidate 3** as the base, and take two things from candidate 1: its container outbound-handler SSH path and its `AuthzReplica` fan-out. Also restate C3's control-plane row as "contained" until Milestone 0 shows containers can start without the control plane.

**The single condition that flips it:** if C3's Milestone 0 consensus gate fails (any linearizability violation in the 48-hour Porcupine run, or Tri commit p99 above 400 ms), take candidate 1 instead. Fix its standby-loss stall first, by letting the primary compare-and-swap itself to `Alone` through the register.

## Where Cloudflare beats AWS, and where it loses
Cloudflare wins on cost and on operations.
- C3 lists at $180 against AWS's $1,287 one-Region and $2,292 Paired at 50 developers, and $5,835 against $20,636 Paired at 2,000 [measured: scripts and `research/costs.md`].
- There are no nodes, NAT gateways, or load balancers to run. Objects per account are unlimited, R2 egress is free, and the anycast edge is included.
- Placement across continents is a location hint away.

Cloudflare loses on blast radius and on guarantees.
- Its large outages are global: November 18, 2025 (about 3 h), December 5, 2025 (28% of HTTP), and June 12, 2025, when DO, D1, and Queues failed together (`research/cloudflare-uptime.md` #9, #13, #14). AWS's worst stayed in one Region.
- Durable Objects, D1, and KV carry no SLA, against DynamoDB global tables' 99.999%, and no Cloudflare page states a cross-region RPO of zero. So Tributary must build the consensus that MRSC sells.
- The 20 GB container disk caps repositories at about 12 to 15 GB, against NVMe nodes on AWS.
- HTTPS pushes are capped at 100 or 200 MB below Enterprise.
- SSH rests on an unverified Spectrum, Tunnel, and container chain.
- Self-hosting means experimental workerd storage.
- Residency stops at `eu`, `us`, and `fedramp`, against AWS's 34 Regions including Zurich.