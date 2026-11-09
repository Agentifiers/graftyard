# am1-runs: AM1's hosted plan on graft-m2b (2026-11-09)

Branch `am1-runs`, cut from main `0172f55`. Everything ran on **graft-m2b**. Nothing touched `graft-m1-*`, no load
went to bf20 (`repo:65536`), and B0-30 was not run.

Two rounds:

1. **Round 1** (12:00Z to 12:50Z): token rotation, the site day, the edge, `apps.mjs`, LF-4, and a B0-37 fetch smoke.
   Every agent row was blocked: graft-m2b's edge did not enable the agent formats, and graft-m2b had no `graft-ops`.
2. **Round 2** (12:50Z onward, after your OK): the agent formats on the edge, `graft-m2b-ops` stood up, the mint check,
   then every row in the plan.

**Short version.**

- **Pass:**
  - LF-4 (two resources);
  - L0-12's refusal counts for all six kinds;
  - L0-13's plan blocks, removed holder, and 5 h renew;
  - B0-36 for the confined vendor App and the Graftyard-run App (shipped and widened) on every surface the deployed
    edge routes, with MCP as the control;
  - B0-37's push half.
- **Fail, with a code fix not deployed:** LF-13. Install mode lost 6, 41 and 39 events across the snapshot's hole
  (`dfa5410` fixes it, and needs a site day). Gap mode never made a gap.
- **Fail:** every 500-agent load row (B0-29, B0-31, B0-32, B0-33, B0-34, B0-35, and L0-12's A1 replay). All of them fail
  for one cause, a missing build piece rather than a tuning miss. The git shard serializes `/validate` per network, and
  `fanout` (k shards per hot network) is not wired. One network therefore validates about 2 pushes/s, against journey
  9's 4.17/s. At 100 and 250 agents the same driver passes its latency lines (see "Diagnosis").
- **Not reachable on this build:** the Graftyard-run kind's lease refusal, B0-37's session-ref listing, LF-4's other
  six resources, and the GraphQL label surface (fixed in code, not deployed).

## Deployed versions on graft-m2b

| Script | Version | Commit | Time (UTC) |
| --- | --- | --- | --- |
| `graft-m2b-site-a` | `6ec447e8-6fbb-414b-b0e4-c5b28c053b09`, then `7d0693fb-0be7-4414-80f2-61cbfbaee07a` | `612c5f4` (= `0172f55` + test drivers) | 12:18Z to 12:31Z |
| `graft-m2b-site-b` | `a41dd8d6-c6c4-40cb-90e5-df1f480caa82`, then `3adb655f-a2eb-468b-aa35-ec8dc88663f3` | `612c5f4` | same |
| `graft-m2b-site-c` | `710fc575-1c53-42e1-8c86-c226a5088c72`, then `bc1d0916-2eee-4cf7-8b8a-0ba39a5d10ae` | `612c5f4` | same |
| `graft-m2b-edge` | `aaf60641-b40c-4563-9d3b-eb212ce5995b` (round 1, was `d560e7d4`) | `612c5f4` | 12:32Z |
| `graft-m2b-edge` | `f850748e-5ee9-47e5-a4f4-7f56b5ba91ae`: formats plus `32.8,32.12,32.14,18.3` | `3f90259` | 12:55Z |
| `graft-m2b-ops` (new) | `c53e4459-2d82-47e3-bbf6-ceae7f759321` | `3f90259` | 12:56Z |
| `graft-m2b-edge` (**current**) | `c4eb6915-f053-43bb-9e32-b43187bb1ba0`: the same formats, plus `--ops graft-m2b-ops` | `3f90259` | 12:57Z |

- `612c5f4` and `3f90259` differ from `0172f55` only under `tests/` (drivers and results). No crate, worker, or
  container file differs, so every deployed script is `0172f55`'s code.
- **Edge `FORMATS_ENABLED`:**
  `32.1,32.4,32.5,32.6,32.7,32.9,32.11,32.13,16.2,17.2,18.2,20.1,20.2,32.8,32.12,32.14,18.3`. These are the same agent
  formats `graft-m1-edge` enables, and no site day was needed for them: the members already read them.
- **Shard image:** `graft-m1-site-gitshard-git@sha256:fb90615a825392d9521505dd662d9a38e1cc3b0af38ff5a5d1206a652249d5b1`
  (`:am1runs`, `containers/git` at `0172f55`, the session-ref build). The roll moved 70 running shard instances, all
  `kill 200, catchup 200`, including bf20 on shards 0 and 1.
- **`graft-m2b-ops`:** followed `m3-hosted`'s order:
  1. `mintkey 1` wrote a 0600 key file.
  2. The Secrets Store secret `graft-m2b-mint-wrap-key` (id `3e8ee5c1ff3944c7b9cad80491dd9db0`, store
     `bfaa0c1573de4775b1b92a12fb89acb4`) was created by an API POST whose body was read from that file. The value
     never reached argv.
  3. `workers/ops/deploy.mjs graft-m2b-ops` (`tests/am1/runs/deploy-ops.sh`).
  4. kid 1's `PutVerifyingKey` was committed on `tokens:0` at index 14.
  5. The edge was redeployed with `--ops`.
  6. The key file was deleted. Only the public half remains (`~/.graft-m2b/ops/putvk-kid1.json`).

## Step 1 (round 1): token rotation

`tests/am1/runs/rotate.mjs` handled `~/.graft-m2b/tokens.txt` lines 1–3: owner `m2b` 7000001, writer `m2b-writer`
7000002, and reader `m2b-reader` 7000003. These were taken to be the three tokens that leaked. Line 4 was already
revoked by the seed and was kept. For each line, the script:

1. revoked the old token through the install's own `DELETE /api/v3/applications/178c6fc778ccc68e1d6a/token`, with the
   token in the JSON body only;
2. stored a fresh `ghp_` for the same account as a `Store` on its `Tokens` shard, as `seed.ts` does;
3. rewrote `tokens.txt` through a 0600 temp file and a rename.

It prints statuses only. The backup copy was deleted.

| Line | Old shard | Old `GET /user` before | Revoke | Old `GET /user` 11 s after | Old over git (`info/refs`) | New shard | New `GET /user` |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1 owner | `tokens:2487` | 404 | 204 | 401 | 401 | `tokens:422` | 404 |
| 2 writer | `tokens:1327` | 404 | 204 | 401 | 401 | `tokens:2622` | 404 |
| 3 reader | `tokens:2569` | 404 | 204 | 401 | 401 | `tokens:1772` | 404 |

`GET /user` answers 404 for these seeded accounts both before and after, because they have no `Identity` profile the
route renders. So 404 rather than 401 means the token is accepted.

**Seeded drivers with the new tokens:** `tests/m3/repos/seeded.sh smoke` ran twice before the site day and three times
after (`seeded-after.tsv`). Every round matched:

- owner `200 true,true,true`, writer `200 false,true,true`, reader `200 false,false,true`;
- `gh issue list` rc 0, and REST `commits/main/statuses` 200 for all three;
- clones rc 0;
- the owner's and writer's pushes accepted, the reader's refused with 403;
- line 4 (revoked) gets `info/refs` 401.

## Step 2 (round 2): mint check

`tests/am1/runs/mintcheck.mjs` (`mintcheck.log`):

| App | `POST /app/installations/{id}/access_tokens` | Push to a test repository with the `ghs_` |
| --- | --- | --- |
| m3-ci (196608, installation 7190001) | 201, `ghs_` | accepted |
| `graftyard-workspace` (1441792, 7190011) | 201, `ghs_` | `[remote rejected]`, "push needs a live task" (expected: a Graftyard-run App pushes only under a task) |
| `graftyard-agent` (1507328, 7190012) | 201, `ghs_` | 403: its shipped permissions have `contents: read` (expected) |
| `graftyard-mcp` (1572864, 7190013) | 201, `ghs_` | accepted |

On graft-m2b, m3-ci's installation had only `contents`. `tests/am1/hosted/app-perms.mjs` gave it the AM1 set
(`contents,metadata,issues,pull_requests,checks`), as on graft-m1. After that, task create answers 201.

## Step 3: the rows

Pass and fail lines are quoted from `build/BUILD-PLAN.md`. Each load row ran on its own fresh repository
(`tests/am1/runs/mkrepos.mjs`, `POST /orgs/m3org/repos`) with the leader on site 0 (enam) unless noted, through
`tests/am1/runs/a1.sh` (`a1.mjs` plus `summ.sh`). "Commit" is the leader's own `commitMs`, because the client latency
from this orb carries about 2.7 s of edge round trips (`am1-fix.md`).

### The 500-agent load rows

| Row | Run, repository | Pass line | Measured | Verdict |
| --- | --- | --- | --- | --- |
| B0-29 | `mw9aff9g`, `a1-29` (131171) | "Commit p99 at most 600 ms, merge p99 at most 2 s, zero `Stale` on distinct refs, leader under 600 req/s with zero feed polls" | Commit p50/p99 **1,205/9,316 ms** (n 321). Agent pushes **5 of 2,500 ok**, 1,772 × 503, 722 client timeouts (120 s). Leader moved 0 → 2 (term 17), 2,305 failed member calls. Merges: 1 of 99 attempted got through. 0 feed polls reached the leader; feed events p99 1,962 ms (50 events) | **FAIL** |
| B0-31 | `mw9c8w2n`, `a1-31` (131172), leader moved to site 2 (apac) first | "Commit p99 at most 1 s with buckets up" | Agent pushes **4 of 2,501 ok** (1,974 × 503, 522 timeouts). The apac leader did not hold: it went back to 0 during setup and ended on site 1 (term 30). Commit n 0 on the final leader. Feed p99 42.3 s. The placement-failing half was not reached | **FAIL** |
| B0-32 | `mw9hjupk`, `a1-32` (131173), 180 s | "Every push acknowledged within 5 s or answered in an HTTP 200 with `ng <ref> agent admission full, retry after <n>s` … zero leader changes" | Burst **0 of 500 within 5 s**: 346 × HTTP 503 and 154 timeouts, neither the `ng` shape. 0 leader changes (term 8). Commit p50/p99 714/1,199 ms (n 84). Agent pushes 72 of 755 ok. Only 39 of 100 setup tasks were made | **FAIL** ("any other refusal shape") |
| B0-33 | `mw9kbt55`, `a1-33` (131174), 5,000 agents, 300 s | "Human and merge-queue commit p99 at most 1 s, at least 140 agent commands/s admitted, the user token admitted as `Agent`, every refusal in the git `ng` or REST 429 `retry-after` shape, zero leader changes" | Agent pushes **9 of 12,454 ok** (12,194 × 503). Agent side commands admitted: about 0.6/s (taskEdit 31, comment 39, label 17, lease 14, standIn 54 ok in 300 s). Fast user 6 of 508 × 200, 502 × 503. Human 4 of 487. Commit p99 1,086 ms (n 53). 0 leader changes (term 18). 3,195 failed member calls | **FAIL** (admitted rate and refusal shape) |
| B0-34 | `mw9hrq6y`, `a1-34` (131175), home shard killed at 300 s (200) | "human push p99 at most 3 s beyond transfer, and clone time to first byte p99 at most 1 s throughout" | Clone TTFB p50/p99 **23.1/86.7 s**: 80 × 200, 255 × 503. Human pushes 0 of 33 attempted ok (29 × 503). Agent pushes 0 of 2,500 ok. Commit p50/p99 5,516/5,659 ms. 0 leader changes. k stayed 1 (no `fanout`) | **FAIL** (TTFB over 5 s) |
| B0-35 | `mw9ia3rs`, `a1-35` (131176) | "Test-merge vCPU within `TEST_MERGE_BUDGET`, Actions dispatch p99 at most 10 s, and zero computes from list fields" | Setup opened **10 of 500** PRs, so the row's premise failed. `mergeable` polls: 187 `MERGEABLE`, 9,386 × 503, 50,404 skipped by the client cap. Agent pushes 63 of 2,500 ok. Commit p99 13,213 ms. Dispatch not measurable: no Actions on m2b | **FAIL** (not a valid run of its line) |
| L0-12, A1 replay with 500 pollers | `mw9ivycf`, `a1-l012` (131177), 500 tasks and 500 Exclusive leases held, a leader kill every 60 s | "500 long-pollers see each event within FEED_BATCH plus 2 s (3 s) at p99 outside leader kills"; "No two holders hold overlapping Exclusive leases for longer than `DRIFT`" | Feed: **0 events delivered to the pollers** in 600 s; 149,265 polls (74,432 no status, 52,602 `gone`, 12,099 `notServing`, 6,277 `lagging`, 3,855 held). Agent pushes 57 of 2,491 ok. Kill pushes: 10 sent, 1 ok, 9 × 503, and the leader did not change (term 21). Lease renews 5 of 1,451 × 200 | **FAIL** (no events seen; the kills did not land) |

### Diagnosis: one network validates about 2 pushes/s

The same driver at lower agent counts, with no other change (`a1.mjs b0-29 --agents N`):

| Run | Agents | Agent pushes ok | Leader changes | Commit p50/p99 | Merge p99 |
| --- | --- | --- | --- | --- | --- |
| `mw9bvrug` (`diag-2`) | 100 | 148 of 148 | 0 | 460/933 ms | 2,622 ms |
| `mw9ctxzt` (`diag-3`) | 250 | 379 of 379 | 0 | 523/992 ms | 2,738 ms |
| `mw9bmucv` (`diag-1`), reads and pollers off | 500 | 375 of 766 | 8 terms | — | — |
| `mw9b4q3q` (`a1-smoke`), reads off | 500 | 541 of 1,241 | 0 → 2 | — | — |

Workers Logs on all three sites during the 500-agent runs (`/tmp`-side `logs.py`, raw excerpts not kept) show the
following chain:

1. **The bottleneck is the shard's `/validate`.** `containers/git/server/main.go` `handleValidate` takes the network's
   exclusive lock (`l := lock(net); l.Lock()`), so one network's pushes validate one at a time on one container.
2. Unloaded, each validate takes 405 ms (mean at 100 agents) to 515 ms (at 250 agents). That caps a network at about 2
   pushes/s; journey 9 offers 4.17/s.
3. At 500 agents the queue diverges. Validate latency reached p90 17.7 s and max 58 s, so busy time exceeded 1.
4. The edge's calls then time out. Followers' `/raft` calls take about 3.1 s, and leaders `StepDown` on
   `CommitError`s. Some members hit "exceeded its memory limit" or a storage reset.

The design spreads a hot network's validates across k shards: "Pushes validate on any caught-up shard of the k, chosen
by rendezvous on the push id" (`design/DESIGN.md`, Shape, git caches). That `fanout` is not built. BUILD-PLAN AM1-WP1
lists it under "Not yet": "the network leader's per-minute `fanout` commit and the shard's `NetworkLoad` report". So
these rows fail on a missing build piece, not on their gate lines. The design's own sizing says k stays 1 at journey 9
("At journey 9's push every 2 minutes k is also 1", `design/DESIGN.md`, Cost). That figure counts git CPU, not the
validate lock (see "Design contradictions").

Also seen in these runs: `prsync 131171` ingest answers 422 "did not receive expected object" repeatedly on site c.

### Non-load rows

| Row | Pass line | Measured | Verdict |
| --- | --- | --- | --- |
| `apps.mjs` | — | `graftyard-workspace` App 1441792, installation 7190011, `AgentApp GraftyardRun`. `graftyard-agent` App 1507328, installation 7190012, `GraftyardRun`. `graftyard-mcp` App 1572864, installation 7190013, `Mcp`. All on `authz:1`, installed on m3org (7100001); keys 0600 in `~/.graft-m3r/agent-apps/`. All mint 201 | **Done** |
| LF-4 | "100% absent from `visible` and counted" | On `m3org/lf4-r` (131168): issues 16 of 16 and releases 16 of 16, every hidden range absent from `visible`, `invisible_chars` 2, raw `body` unchanged. Control without the media type: no `graftyard_trust`, `Vary: Accept` set. Pull requests, issue comments, reviews, review comments, commits, and check runs have no REST GET on the edge | **Pass** for the two resources the edge serves; the other six are not reachable |
| L0-12 refusals (`rows-j` 131180; MCP rerun on `rows-m` 131188) | "100 of 100 refused for each of the two `ActorKind::Agent` kinds, 0 of 100 for the other three" | See the table below | **Pass** on counts; the Graftyard-run kind is refused by `NoLiveTask`, not by the lease |
| B0-36, confined vendor App (m3-ci) | "100 of 100 refused for each" | See the B0-36 table | **Pass** on every surface the deployed edge routes; GraphQL labels not routed (fixed in `89cf205`, not deployed) |
| B0-36, Graftyard-run App (`graftyard-agent`, shipped permissions) | same | See the B0-36 table | **Pass**, same caveat |
| B0-36, Graftyard-run App widened to every permission (`widen.mjs wide`: actions, administration, checks, contents, issues, metadata, pull_requests, statuses, workflows; put back to shipped at 19:05Z) | "The Graftyard-run token runs once with its shipped permissions and once on an installation widened to every permission, and a refusal from either counts" | approve, ready for review: 100/100 refused by the agent rule. Release and tag push: 100/100 "agents may not create releases or tags". REST label: 100/100 "not in this repository's agent label allowlist". Merge: REST and GraphQL 100 × 403; auto-merge "not allowed". `PUT agents/policy`: 100 × 403 "agents may not edit rulesets…". Pushes and REST ref writes outside the prefix, and protected paths over git and REST: 100/100 refused, "push needs a live task; the token names no task". `graftyard/` status: 100 × 401. Controls: ordinary push 71 refused (`NoLiveTask`) + 29 rejected (token expired); draft PR 100 × 401 (token expired; this run started before `40eb192`) | **Pass**; the git-surface refusals come from `NoLiveTask`, as in L0-12 |
| B0-36, `graftyard-mcp` | `Graftyard MCP` is never `ActorKind::Agent`, so no agent refusal should apply | approve, ready for review, release, tag push, REST label, pushes and REST ref writes outside `claude/**`, protected paths over git and REST, the ordinary push, and the draft PR: **100/100 accepted each**. Merge (REST and GraphQL) and `PUT agents/policy`: 403 "Resource not accessible by integration" (permission, not an agent rule). `graftyard/` status: 401 (its `statuses: read`). Auto-merge: "not allowed for this repository". GraphQL label: not routed, as for every kind | **As expected**: no agent refusal hit the MCP kind |
| B0-37 push half (`rows-b037` 131181, m3-ci confined, 100 per kind) | "Zero pushes fail on parsing, every forged trailer stays `Claimed`, every `refs/graftyard/` update not by `graftyard-workspace` is refused" | **0 pushes failed on parsing**: 899 of 900 trailer pushes accepted, and the one other was a transport error ("unable to access", linked control). Stored provenance from the leader's log (1,009 entries): bad URL, control chars, 4 KiB line, another App's task, and unknown task: **100 `Claimed`** each, 0 linked. Repeated key: 100 with the first value `Session` and the second `Claimed`. No blank line: no trailer block, so no row. Linked control: 99 `Session`. Writer pushes to `refs/graftyard/` by m3-admin: **100/100 refused** ("deny updating a hidden ref"). The `Co-authored-by` user push was accepted with no provenance row, so it stays Human. "Forged: another repository": see the note below | **Pass** (scaled: 100 per kind, about 1,000 pushes, not 10,000) |
| B0-37 fetch half | "session refs appear only to a v2 `ref-prefix` that starts with it" | Round 1, 10 of 10 rounds (`b037-fetch.tsv`): a writer's push to `refs/graftyard/sessions/…` was `[remote rejected]`; v0 `ls-remote` 2 refs with 0 `refs/graftyard/`, v2 the same, `clone --mirror` 1 ref with 0; a raw v2 `ls-refs` with `ref-prefix refs/graftyard/sessions/` got 200 with 0 lines. No session ref exists to list, because only `graftyard-workspace` under a live Graftyard-run task writes one, and the edge creates only `external` tasks (AM3-WP3) | **Partial**: hiding and refusal shown; listing not reachable hosted (`containers/git/server/sessions_test.go` covers it) |
| LF-13 gap × 3 (`lf13-g` 131178), install × 3 (`lf13-i` 131179) | "Every held poll returns within 1 s with its cursor and no events, new polls are refused until it catches up, and no event is lost" | See the LF-13 table below | **FAIL** in both modes. Gap: the fault never made F refuse an AppendEntries, so no gap was shown. Install: F served `hold` from a cursor across the snapshot's hole and lost 6, 41 and 39 events. Fixed in code (`dfa5410`), not deployed |
| L0-13, holder loses push access (`l013-d` 131187) | "The removed holder's lease refuses nothing at the next `decide`" | A fresh org member held an Exclusive lease (201) under `AgentsExclusive`. Before removal: m3-ci's 2 of 2 pushes into the scope refused ("is leased Exclusive by user …", the holder 7100105); the lease listed (1). After `SetMember role: null`: 2 of 2 pushes accepted at once, and the list shows 0 | **Pass** (2 pushes each side; the line asks for the next `decide`) |
| L0-13, 600 + 600 plan blocks (`l013-p` 131186) | "Zero plan leases from non-writers and at most 500 from writers" | Non-writers (3 fresh users, no membership): 600 of 600 drafts opened, **0 plan leases**. Writers (75 fresh org members, 8 drafts each): 600 of 600 opened, **500 plan leases** (counted by a recount through `after` paging), from 63 holders, at most 8 each (`LEASES_PER_HOLDER`). The driver's own count read 2,000 because it paged with `page=`, which the list ignores, so it counted the first page 20 times. Fixed in `925a8f0` | **Pass** |
| L0-13, 5 h renew (`l013-r` 131185, acquired 14:13:59Z) | "The lease reads as Shared after 4 h" | Exclusive through 151 min in both the reply and the list; `shared` in both at 251 and 301 min; every renewal 200 | **Pass** |

**L0-12, 100 pushes and 100 REST ref writes into another holder's Exclusive scope under `AgentsExclusive`, per kind:**

| Kind | `ActorKind` | Pushes | REST ref writes |
| --- | --- | --- | --- |
| confined vendor App (m3-ci, `confine claude/**`) | Agent | **100/100 refused**, naming the lease | **100/100 refused** (422), naming the lease |
| Graftyard-run App (`graftyard-workspace`) | Agent | **100/100 refused**, "push needs a live task; the token names no task" | **100/100 refused** (422), same |
| flagged App without `confine` | Bot | 0/100 refused | 0/100 refused |
| unflagged App | Bot | 0/100 refused | 0/100 refused |
| human PAT | Human | 0/100 refused | 0/100 refused |
| `Graftyard MCP` (`graftyard-mcp`), rerun on `rows-m` | Bot | 0/100 refused | 0/100 refused |

Notes on the table:

- **Graftyard-run kind.** This kind is refused before the lease check runs, because a Graftyard-run App with no task
  cannot push at all. The lease refusal for this kind needs a Graftyard-run task, which the edge cannot create (AM3-WP3).
- **MCP REST writes in the first run.** In the first run (`rows-j`), MCP's REST writes all got "422 Object does not
  exist". That was a driver bug, not a gate result. The driver keyed each kind's scratch refs and files by the length
  of the kind's name, and "unflagged App" and "Graftyard MCP" are both 13 characters. So the MCP kind's scratch pushes
  landed on refs the unflagged kind had already moved, and the commits the REST writes named were never stored. Fixed
  in `356cf40` (refs keyed by the kind's index) and rerun on a fresh repository: 100 of 100 accepted.
- **Duplicate MCP run.** The MCP rerun's service restarted once after it finished and began a second run (holder lease
  acquired). I stopped it and set `rows-m`'s policy back to `advisory` (200). Its two setup rows are kept in
  `rows-l012-mcp-restart.jsonl`.

**B0-36, 100 per action:**

| Action (surface) | confined vendor App (m3-ci) | Graftyard-run App (`graftyard-agent`, shipped) |
| --- | --- | --- |
| approve (GraphQL `addPullRequestReview`) | 100 refused: "agents may not approve pull requests" | 100 refused, same |
| merge (REST `PUT pulls/n/merge`) | 100 × 403 | 100 × 403 |
| merge (GraphQL `mergePullRequest`) | 100 × 403 | 100 × 403 |
| auto-merge (GraphQL `enablePullRequestAutoMerge`) | 100 refused: "Auto merge is not allowed for this repository" | 100, same |
| ready for review (GraphQL `markPullRequestReadyForReview`, new) | 100 refused: "agents may not mark pull requests ready for review" | 100, same |
| release (REST `POST releases`) | 100 × 403: "agents may not create releases or tags" | 100, same |
| tag push (git) | 100 refused: "agents may not create releases or tags" | 100 rejected (`contents: read`) |
| label outside the allowlist (REST `POST issues/n/labels`, new) | 100 × 403: "not in this repository's agent label allowlist" | 100, same |
| label outside the allowlist (GraphQL `addLabelsToLabelable`) | 100 × "a mutation runs through plan": **not routed** (see below) | 100, same |
| `graftyard/agent-gate` status (REST `POST statuses`) | 100 × 401 "Requires authentication" | 100, same |
| push outside its prefix (git) | 100 refused: "app 196608 may push only refs/heads/claude/**" | 100 rejected |
| REST ref outside its prefix (`POST git/refs`) | 100 × 422, same message | 100 × 403 (no `contents: write`) |
| protected path in its prefix (git, `.github/workflows/`) | 100 refused: "is a protected path" | 100 rejected |
| protected path in its prefix (REST, `CODEOWNERS`) | 100 × 422: "CODEOWNERS is a protected path" | 100 × 403 |
| rule or settings edit (`PUT agents/policy`) | 100 × 403 | 99 × 403, 1 × 401 (token expired, see below) |
| control: ordinary push in its prefix | **100 accepted** | 100 rejected (`contents: read`, expected for this installation) |
| control: draft PR (GraphQL `createPullRequest`) | 49 accepted, 51 × 401 (token expired) | 100 × 401 (token expired) |

Notes on B0-36:

- **GraphQL labels.** On the deployed edge, `crates/api/src/graphql.rs` `route` did not find the repository of an
  `addLabelsToLabelable` by its `labelableId`, so the mutation fell through to "a mutation runs through plan". It was
  fixed in `89cf205`, with a test in `crates/api/tests/surfaces.rs` that fails without the fix. That commit is **not
  deployed**, because this task allowed no edge deploy beyond the two you named. So this surface is not shown hosted.
- **The 401s at the end of each run.** A 100-per-action run takes about an hour, and the driver minted its installation
  token once, so the last calls ran on an expired token. Fixed in `40eb192` (re-mint every 45 min), which applies to the
  later runs. The refusal rows are unaffected, because all of them finished inside the token's hour.
- **The `graftyard-agent` installation.** Its pushes and ref writes are rejected by permission (`contents: read`)
  before any agent rule runs. The widened run below exercises the agent rules for this kind.

**L0-13, the 5 h renew** (one Exclusive lease on `l013r/<tag>/**`, renewed every 50 min with a 3,600 s TTL):

| At (min) | Renew | Mode in the renew reply | Mode in the lease list |
| --- | --- | --- | --- |
| 0 (acquire) | 201 | exclusive | — |
| 50 | 200 | exclusive | exclusive |
| 100 | 200 | exclusive | exclusive |
| 151 | 200 | exclusive | exclusive |
| 201 | 200 | shared | exclusive |
| 251 | 200 | **shared** | **shared** |
| 301 | 200 | **shared** | **shared** |

The run ended after the 301-min renewal (stopped at 19:17Z, five hours after the grant).

- **The reply at 201 min.** The renew reply is rendered at the lease's new `expires` minus 1 ms (`crates/api/src/hosted.rs`,
  `What::Lease`). That is 261 min after the grant, past `EXCLUSIVE_MAX` (240 min), so the reply reads `shared` while the
  list, read at 201 min, still reads `exclusive`. Both are right for the instant they render.
- **After 4 h.** Every read after 240 min reads `shared`, which is the pass line.

**B0-37, the forged-origin kind.** In the main run, "forged: another repository" stored `Session` (linked) for 100 of
100, which looks like a forged trailer that linked. It was a driver error, not a gate result. The runner set
`ROWS_REPO` and `ROWS_B037_REPO` to the same repository, so that kind's "another repository" origin named the push's
own repository and its own live task: a valid trailer. Rerun on its own with `ROWS_REPO=rows-j` (the trailer names
`m3org/rows-j` while the push goes to `rows-b037`; `7a7574d`): **100 of 100 accepted, 100 of 100 stored `Claimed`**,
0 linked.

**LF-13, per run** (`tests/am1/rest/lf13.mjs`; F is the follower under the fault; 1,000 polls held on F before it):

| Run | Fault on F | Leader after | Held polls | New polls to F after the lift | Events: committed / on the reference / missing on F |
| --- | --- | --- | --- | --- | --- |
| gap 1 | 11.6 s, no transfer took effect | 0 (unchanged) | 1,000 `hold`, all returned **before** the lift, each with a cursor and events | 2 × 503, then `hold` from 1.0 s; **0 `lagging`** | 8 / 2 / 0 |
| gap 2 | 8.1 s, leader moved 0 → 2 | 2 | same | 1 × 503, then `hold` from 0.66 s; 0 `lagging` | 8 / 0 / 0 |
| gap 3 | 23.6 s, leader moved 2 → 1 | 1 | same | 1 × 503, then `hold` from 0.56 s; 0 `lagging` | 4 / 2 / 0 |
| install 1 | 64.2 s; leader archived and truncated to 18 with F at 11 | 0 | same | 3 × 503, 1 `hold`, then **10 `lagging`** | 34 / 16 / **6** |
| install 2 | 62.3 s; truncated to 63, F at 60 | 0 | same | 1 × 503, then **45 `lagging`** | 48 / 45 / **41** |
| install 3 | 60.7 s; truncated to 165, F at 161 | 0 | same | **48 `lagging`** | 46 / 43 / **39** |

What the runs show:

1. **Held polls.** "Every held poll returns within 1 s with its cursor and no events" fails as written in all six
   runs. The held polls did not wait for the fault: all 1,000 returned before it lifted, with a cursor and with
   events. They had been held with a 25 s wait on `lf13/**`, and the lease commits that drive the fault themselves
   are `lf13/**` events. So the driver's own writes woke them. The run cannot tell "returned within 1 s of the lift"
   apart from "returned because an event arrived".
2. **Gap mode.** No new poll was refused (0 `lagging` in 50 probes over 10 s from the lift). The site marks a gap only
   when F answers an AppendEntries with `ok: false` (`workers/site/src/family.rs`, `stepped`), so by the time of the
   first probe no gap was marked. Moving the leader while F's calls fail did not leave F a gap that the probes could
   see. So this mode does not make the row's fault. That agrees with
   `crates/raft/tests/lf13.rs` (see "Design contradictions").
3. **Install mode.** It reached `InstallSnapshot`, and new polls were refused (`lagging`) while F caught up, which
   passes that half of the line. But afterwards F served a `hold` from the cursor taken before the fault, and that
   hold was missing the events of every entry the snapshot covered: 6, 41 and 39. That is "any event skipped", the
   fail line.
4. **Cause and fix.** The site feeds F's ring each applied entry. An install moves F's base past entries the ring
   never saw, but `Ring::feed` (`crates/git/src/feed.rs`) still took the next entry as contiguous. So its floor stayed
   below the hole, and a cursor from before the hole was served past it. Fixed in `dfa5410`: the ring restarts after
   the first entry fed past a hole, so such a cursor gets `gone` (410 with the oldest cursor it can serve, the design's
   ring rule). There is a test (`a_ring_fed_past_a_hole_refuses_cursors_from_before_it`) and two plants
   (`tests/am1/runs/plant.py` 1 and 2), both caught. **Not deployed**: it is a site-script change, so it needs a site
   day, which this task did not include.

## What LM-1 still lacks (besides B0-30)

1. **A git shard path that holds journey 9 on one network.** B0-29, B0-31 to B0-35, and L0-12's A1 replay all fail at
   500 agents on the per-network validate lock. This needs `fanout` and the `NetworkLoad` report (AM1-WP1, "Not yet"),
   or validate concurrency inside one shard. B0-34 also needs `fanout` and the warm pool for its own line, and B0-35
   needs a setup that can open 500 PRs on a working group.
2. **Graftyard-run task creation** (AM3-WP3). It is needed for:
   - the Graftyard-run kind's lease refusal in L0-12 and B0-36 (today it is refused earlier, by `NoLiveTask`);
   - B0-36's "push with no live task and a fork push after its task is cancelled" for that kind;
   - B0-37's session-ref listing through a v2 `ref-prefix`.
3. **LF-13.** It needs:
   - a site day with `dfa5410` (the feed ring's hole), then a rerun of install mode;
   - a fault that actually leaves a gap, for gap mode;
   - a driver whose held polls cannot be woken by its own writes, for the "returns within 1 s with no events" half.
     For example, the polls could hold on a path the fault's commits do not touch.
4. **An edge deploy of `89cf205`**, for B0-36's GraphQL label surface.
5. **LF-4's other six resources** (pull requests, comments, reviews, review comments, commits, check runs), which need
   REST GETs on the edge.

What passed and closes its part of LM-1:

- L0-12's refusal counts;
- L0-13's plan blocks, removed holder, and 5 h renew (with the six bad acquires from `am1-hosted`);
- B0-36 for the confined vendor App and the Graftyard-run App (shipped and widened), with MCP as the control;
- B0-37's push half;
- LF-4 for issues and releases.

## Costs incurred

From `usage.py` (Cloudflare GraphQL Analytics for the account), priced at overage rates from `research/prices.json`.
Containers are excluded, because Analytics does not report them here.

| Window (UTC) | Workers requests | Workers CPU | DO requests (m2b) | DO rows written (account) | DO active s (account) | Cost |
| --- | --- | --- | --- | --- | --- | --- |
| 12:00–12:50 (round 1) | 473 | 9.1 s | 26,874 | 28,012 | 11,660 | about $0.05 |
| 12:50–18:06 (round 2, through B0-33) | 1,132,133 | 10,892 s | 9,070,037 | 1,526,430 | 442,859 | about $4.15 |
| 18:06–21:35 (B0-36 reruns, B0-37, LF-13, L0-13) | 22,394 | 565 s | 641,064 | 1,972,466 | 454,721 | about $2.81 |
| **Total** | | | | | | **about $7.0** |

Prices: Workers $0.30 per million requests and $0.02 per million CPU-ms, DO $0.15 per million requests, $1.00 per
million rows written, and $12.50 per million GB-s at 128 MB. The last window's rows and active time are mostly
account-wide, not graft-m2b's (it made only 641 k DO requests). The plan estimated about $5.44.

The account-wide figures include anything else running on the account, so they are an upper bound for graft-m2b. No
row came near the $15 stop line. B0-33's window (17:56Z–18:05Z) cost about $0.27 by the same rates; a 500-agent row
about $0.2 to $0.3.

## Design contradictions

1. **Validate concurrency at journey 9.**
   - `design/DESIGN.md`, Cost: "At journey 9's push every 2 minutes k is also 1, with 0 extra shards ($0 a month)."
   - `design/DESIGN.md`, Shape: "Pushes validate on any caught-up shard of the k".
   - With k = 1, every push to a network validates on one shard. The shard's validate holds the network's exclusive
     lock (`containers/git/server/main.go`, `handleValidate`), and each validate takes 405–515 ms unloaded.
   - Measured: one network holds 250 agents (1.04 pushes/s) and diverges at 500 (4.17/s offered).
   - So either k must exceed 1 at journey 9, which contradicts the $0 line, or the shard must validate one network's
     pushes concurrently, which the code does not do and the design does not say. The design sizes k from git CPU
     (2.68 vCPU, 67% of one `standard-4`), not from this lock.
2. **LF-13's wording.**
   - As in `am1-rest.md`, the row's fault ("its AppendEntries dropped for 5 s so the next one leaves a gap",
     `design/AGENT-LAYER.md`, Milestone 0 row LF-13) cannot leave a gap under one leader.
   - Hosted, a leader change during the fault did not leave one either (gap runs 2 and 3: 0 `lagging`).
   - Only truncation, which forces `InstallSnapshot`, put F behind.
3. **The feed ring and `InstallSnapshot`.**
   - `design/AGENT-LAYER.md`, Shape: "A cursor older than the ring gets 410 with the oldest".
   - A follower that installed a snapshot has a ring with a hole in it. The design does not say what a cursor inside
     that hole gets.
   - The code served it as if the ring were whole, and lost events. `dfa5410` gives such a cursor 410, as for a
     cursor older than the ring. That reading is mine, not the design's.

## Bf20 numbers, kept for reference (not loaded)

From `tests/m3/results/wire-fix.md`:

- 571,872 refs;
- receive-pack `info/refs` is 41,954,822 bytes, streamed in 24.2 s;
- a v2 `ls-remote` takes 1.7 s;
- a hand catch-up of 58 pages took 140.6 s.

In this site day's roll, bf20 answered `kill 200, catchup 200` on shards 0 and 1.

## Commits on `am1-runs`

From `0172f55`, oldest first. Nothing pushed or merged.

| Commit | What |
| --- | --- |
| `bba90cb` | token rotation through the install's API |
| `612c5f4` | image, site day, and edge drivers |
| `3f90259` | round 1 results |
| `b5ce9ba` | `graft-m2b-ops` and agent-format deploy drivers; mint check |
| `288c4c0`, `3cb2f41`, `bd20257`, `d2d59a9`, `5f366be`, `fd7c6f5` | load and row drivers (A1 window and diagnosis flags, transfer, L0-13, B0-36 widened, runners) |
| `89cf205` | **code:** GraphQL label mutations route by `labelableId`, with a test (not deployed) |
| `ee97b23` | checks driver |
| `356cf40`, `40eb192`, `7a7574d`, `925a8f0` | driver fixes found by the runs (L0-12 scratch refs, B0-36 token re-mint, B0-37 forged origin, L0-13 lease paging) |
| `0b873f5` | results so far |
| `dfa5410` | **code:** the feed ring restarts after a hole, with a test and `tests/am1/runs/plant.py` (not deployed) |
| (this commit) | results |

**Plants** (`tests/am1/runs/plant.py`): 3 of 3 caught.

| # | Mutant | Caught by |
| --- | --- | --- |
| 0 | `addLabelsToLabelable` not routed by `labelableId` | `ready_for_review_and_issue_labels_commit_and_render_as_ghes_does` |
| 1 | a ring fed past a hole serves cursors from before it | `a_ring_fed_past_a_hole_refuses_cursors_from_before_it` |
| 2 | the ring restarts at the entry past the hole, not after it | the same test |

The new test fails on the code as deployed (a cursor from before the hole is served: "oldest Index(7), cursor
Index(8)") and passes with `dfa5410`.

**Full checks at `dfa5410`** (`tests/am1/runs/checks.sh`, log `tests/am1/raw/runs/checks.log`): every step `RC=0`.
That covers `cargo fmt --check`, workspace clippy, wasm32 clippy for site and edge with and without `test-faults` and
for ops, `cargo test --workspace` (70 suites, **599 passed**, 0 failed, 3 ignored), the three `scripts/check_*.py`,
and the shard's `go test`.

## Token scan

**What was checked.** A Python scan (kept outside the repository, printing file names and counts only) read 136 files:
every file under `tests/am1/raw/` (raw logs and JSONL of both rounds), `tests/am1/runs/`, `tests/am1/hosted/`,
`tests/am1/rest/`, this file, and `crates/git/tests/lease_gates.rs`. It looked for:

- the exact text and the first 12 characters of every token in `~/.graft-m2b/tokens.txt` (rotated) and
  `~/.graft-m3r/m3-tokens.txt`;
- the bed token, the App client secret, and the account API token;
- 40-character slices of every private key on disk: the App key, the three agent-App keys, and the install's customer
  and release keys;
- any `gh[pousr]_…`, `github_pat_…`, JWT, or `BEGIN … PRIVATE KEY` shape.

**Result.** 1 hit, a false positive: in `checks.log`, `cargo test` prints the test name
`stateless::tests::a_token_is_ghs_appid_then_a_jws_of_varying_length_and_verifies`, whose tail matches the `ghs_`
shape. No token, key, or secret text appears.

**Mint wrap key.** The key file was deleted after its secret was stored. Only its public half
(`~/.graft-m2b/ops/putvk-kid1.json`) is kept, outside the repository.
