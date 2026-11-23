# G37: the composed-miss chain stops stepping after the home shard's kill (design, not built)

F112, 2026-11-23. Asked by the lead after mt20 (`tests/am1/results/b034-mt20.md`): a $0 design for G37 in
`decisions/OPEN-QUESTIONS.md`, priced against mt14's and mt20b's traces, with the invariants each option touches. No
code. Tags follow `design/SHARED.md`, "Read this first".

## Short answer

- **The chain does not stop because the home shard is rehydrating.** It stops because the restarted builder rebuilds
  the chain's base from nothing. A new container holds no chain, so the builder's first step after its catch-up is
  `/compose?start=1` with no head, which builds a 260 MB base at nice 19. The bucket chain it put before the kill is
  never read. The builder path in `ShardHost::ready_chain` (`crates/git/src/host.rs`) never installs from the buckets;
  only other shards do.
- **That base build is the whole window.** It ran 120 to 270 s after the home's catch-up ended, and every composable
  miss forwarded to the home in that time waited `CHAIN_WAIT_FORWARDED` (2 s) and answered 503 `warming`: 25 misses on
  mt20b and 45 on mt14 [measured]. mt20b's one second-phase miss over 5 s is one of them: 2,000 ms at the home, then
  2,568 ms on site b, for 5,195 ms.
- **Proposed: A, the builder installs the buckets' chain before its first step on a container that holds none.** Its
  step is then a part, not a base. Priced at about 14 s plus one part step, against 199 s (mt20b) and 341 s (mt14) from
  the step's start to the home's first tail answer. Optionally **B**, a forwarded composable miss on a chain-site shard
  whose container holds no chain answers `warming` at once. B takes the 2 s off each miss in the window A leaves.
- **Not proposed: builder failover** (mt14's proposal) **or a larger `tailMax`.** Failover adds a second writer to the
  buckets' chain, and the edge's chain safety argument rests on one writer. A larger `tailMax` addresses only site b's
  composes past a frozen chain, which A removes by keeping the chain stepping.
- **Invariants:** A and B touch none of the `AGENTS.md` invariants. Failover touches no `AGENTS.md` invariant either, but
  it breaks the single-writer ordering that `colo-gate`, `head-etags` and `chain-warm` rely on (below).

## What the traces show

Sources: `tests/fanout/raw/shardprocs-*.jsonl` (the home container's process list every 15 s),
`misslines-*.jsonl` (the shard's miss lines), `misses-*.txt` (a1's probes joined to them). mt20b's raws are on
branch `b034-mt20`; mt14's are on main.

| | mt20b (2026-11-23) | mt14 (2026-11-21) |
|---|---|---|
| Home container gone | 20:12:09 sample | killed 14:32:49 |
| Home container running again | 20:12:24 sample | 14:32:58 sample |
| Catch-up after the restart (`index-pack`, `rev-list`, midx) | to about 20:13:54 | to about 14:33:58 |
| Base build: `pack-objects --window=0 … /data/compose/…`, nice 19, RSS 2.6 to 2.7 GB | seen 20:13:54 to 20:15:54, 19.8 CPU-s | seen 14:33:58 to 14:38:28, 17.5 CPU-s |
| Home's first `stock: no chain` (its container holds no chain) | 20:14:12 | 14:34:36 |
| Home's 2 s `warming` 503s | 25, 20:14:13 to 20:16:50 | 45, 14:34:36 to 14:39:29 |
| Home's first `tail` again | 20:17:13 | 14:39:39 |
| Step start to first tail | 199 s | 341 s |

[measured, except that the base build is the builder's post-restart step, which is inference from the process
(`/data/compose/` path, no chain on the container) and the order of events]

- **Correction to mt14's reading.** `b034-mt14.md` says the home "rehydrated until about 14:39:30Z". The home was
  serving by 14:34:36: it answered that miss `stock: no chain` after a 110 ms catch-up. From 14:33:58 to 14:38:28 it
  was building the base. So mt14's "the chain froze with the builder" holds, but the freeze was the rebuild.
- **Why the build is slow.** The base is built at nice 19 (`gitLow`), and the home container is the busiest one after
  the kill: geometric repack (mt20b 20:13:54 to 20:18:39), validates, merges and catch-ups. 17.5 to 19.8 CPU-s took 2
  to 4.5 min of wall.
- **Where the clock went for over-5 s misses** in the window from the kill to the home's first tail:

| Phase | Misses | Home 2 s 503 | Of them over 5 s | Under 5 s without the 2 s | Site b composed over 5 s |
|---|---|---|---|---|---|
| mt20b kill | 43 | 3 | 0 | — | 0 |
| mt20b second | 27 | 10 | **1** (5,195 ms) | 1 | 0 |
| mt14 kill | 48 | 1 | 0 | — | 0 |
| mt14 second | 107 | 24 | **5** (5,122 to 6,136 ms) | 5 | **3** |

  mt14's other second-phase miss over 5 s was a site b `tail` at 5,048 ms [measured]. Site b's composes are the
  frozen chain's: the bucket head did not move from 14:31 to 14:39 while the tip moved every 2 minutes
  (`b034-mt14.md`, "2. Site b's shard composes").

## Options

### A. The builder installs the buckets' chain before its first step on a container that holds none (proposed)

- **Change.** In `ready_chain`'s builder branch, if the container holds no chain (`ShardHost::chain` answers `None`)
  and a warm bucket's `head` names one, install it as any other shard does (`fetch_chain_file`, `install_chain`), then
  take the step as today. The step is then a part from that head's tip, or a base only where it already would be one
  (past `composeDiffMax`, or at `foldEvery`). If the bucket has no head, or a file is missing, the step builds a base,
  as today. One file: `crates/git/src/host.rs`. The edge links it, so edgeboot runs. It ships on a site day.
- **Price.** The install is about 260 MB from R2 into the container: 13.8 s for site c's install at mt12's kill
  (`decisions/DECISIONS.md`, "`chain-install`") [measured], and the part step is a few seconds [guess]. So the home is
  ready about 15 to 30 s after its catch-up, against 199 and 341 s. On mt20b the 2 s 503 window would end about
  20:14:10 to 20:14:25, not 20:16:50. The second-phase clone at 20:15:02 meets a home with a synced chain, whose tails
  ran p50 886 ms after it recovered [inference]. On mt14 the chain would step again from about 14:34:15. Site b's
  tails would stay under `tailMax`, so its 3 over-5-s composes become tails, along with the home's 5 [inference].
- **What else it saves.** Today each post-kill base is a new base. It puts about 260 MB to each warm bucket, and its
  head deletes the old base's files (`ready_chain`, the fold branch). An edge streaming the old manifest is cut short
  (`colo-gate`'s case). A keeps the old base [inference from the code].
- **Invariants.** None of `AGENTS.md`'s. The chain keeps one writer, and `colo-gate`'s ordering holds: the builder
  still puts every file a head names before the head, and deletes an old base's files only after the head that
  replaces it.
- **Risk.** The bucket head can be behind the home's disk (pre-kill tip). That is the normal case for a part step: a
  part covers the moves since. A head from an older base past `composeDiffMax` builds a base, as today.
- **Plant (when built).** A builder that ignores the bucket and builds a base. Test: a builder whose container holds
  no chain, with a bucket head present, installs that head and steps a part, and puts only the part and the head.

### B. A forwarded composable miss answers `warming` at once while its chain-site shard holds no chain (optional, second commit)

- **Change.** In the composable path of `workers/site/src/shard.rs`, when this isolate has no `synced` chain for the
  object and the container answers no chain, a miss with `next=1` answers 503 `warming` at once instead of waiting
  `CHAIN_WAIT_FORWARDED`. The last site still waits its whole deadline, so `chain-wait`'s rule (a miss never runs out
  of shards that would wait) holds. That is `warming_now`'s rule for a rehydrate, applied to a missing chain. It costs
  one container `GET /compose-head` per composable miss, and only while `synced` is `None`.
- **Price.** 2 s off each forwarded miss in the window. Alone, it moves mt20b's miss to about 3.2 s and mt14's five
  home-path misses to 3.1 to 4.1 s. mt14's 4 site-b misses over 5 s stay, so mt14 second would be 4 of 169 over 5 s
  (97.6%), against 9 [inference]. With A, it covers only A's 15 to 30 s install window.
- **Invariants.** None of `AGENTS.md`'s. It narrows `chain-wait` for the no-chain case. `chain-wait`'s flip ("raise
  the bound") is not met by it.
- **Plant (when built).** At-once also on the last site, so a miss runs out of shards that would wait.

### C. Builder failover (mt14's proposal; not proposed)

- **Change.** While the home's first shard cannot step, the first caught-up shard on the other chain site steps the
  chain and writes the buckets.
- **Price.** On these traces, as good as A at best. Site c or b would step only after its own catch-up, and it would
  also hold no chain at first unless it installed one, so it pays A's install anyway. It also needs a rule for who is
  builder, and a handover.
- **What it breaks.** The edge's chain reads are safe because one writer orders its puts. Every file before the head,
  and an old base's files deleted only after the head that replaces it (`colo-gate`, `head-etags`, `chain-warm`).
  Two builders that fold at different times can each delete files the other's head names. An edge then streams a head
  whose pack is gone and cuts the answer after bytes left. That is the case `chain-warm` and `chain-hold` exist to
  stop. Making it safe needs a lease or a fence on the head, which is a new failure domain to row in "Failure modes".
- **Invariants.** None of `AGENTS.md`'s directly, but the design's single-writer rule for chain files would change.

### D. A larger `tailMax` (not proposed)

- **Change.** Raise the container's `tailMax` (8 MiB) so a want far past a frozen chain still answers `tail`.
- **Price.** It touches only site b's composes past a frozen chain: mt14 second's 3, mt20b's 0. It does nothing for
  the home's 2 s 503s, which are 6 of mt14's 9 second-phase misses over 5 s and mt20b's only one. The shard object
  holds a tail whole in a 128 MB isolate, so the ceiling is near.
- **Invariants.** None. With A the chain keeps stepping, so tails stay small without it.

## Recommendation and the hosted check

- **Build A.** B as a second commit only if the lead wants mt14's second phase covered with no install window.
  Neither touches member.rs, the merge path, or `catchup-lag`'s files.
- **B0-34's lines.** A and B change only the post-kill chain path: kill's and second's every-clone line, and the
  composed share of hostile. They do not touch the kill line's pushes, the hits line, or maint-yield.
- **Hosted check: one B0-34 row through the site day.** Read:
  - the home's 2 s 503 window after its restart, against mt20b's 2.6 min and mt14's 4.9 min, with a target under 30 s;
  - whether a base is built after the kill (shardprocs' `/data/compose/` `pack-objects` at 2.6 GB RSS), with a target
    of none;
  - the post-kill puts to `compose/<net>/` (only parts and `head` expected);
  - every-clone in kill and second.
- **Flip (for the DECISIONS entry when built).**
  - The home's window stays over 60 s with no base built: the install is slow, so price B, or an install at the
    container's start.
  - Site b composes past `tailMax` while the home steps: price D.
  - A post-kill bucket head names files the buckets lack: the install-first path has a gap in `HeadObject::plan`.

## Not measured

- The install's wall on the home under post-kill load (mt12's 13.8 s was site c, not the busiest container).
- A part step's wall right after an install.
- mt14's 11 of 23 home misses over 5 s from 14:39:30 to 14:41 (container 4 to 5 s each). They are after second
  ended, but suggest the first parts after a new base were slow [unverified].
