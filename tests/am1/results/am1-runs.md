# am1-runs: AM1's hosted plan on graft-m2b (2026-11-09)

Branch `am1-runs`, cut from main `0172f55`. Everything ran on **graft-m2b**; nothing touched `graft-m1-*`, and no
load went to bf20 (`repo:65536`). B0-30 was not run.

**Short version.** The token rotation, the site day, the edge redeploy, `apps.mjs`, LF-4, and a B0-37 fetch-half
smoke passed. Every other row in the plan is **blocked on this install, not run**, for two reasons the am1-rest plan
missed:

1. **graft-m2b's `FORMATS_ENABLED` lacks the agent formats.** It is
   `32.1,32.4,32.5,32.6,32.7,32.9,32.11,32.13,16.2,17.2,18.2,20.1,20.2`. The agent surfaces need 32.8
   (`REPO_FORMAT_AGENT`), 32.12 (`REPO_FORMAT_TASKS`), 32.14 (`REPO_FORMAT_LEASES`) and 18.3 (`AUTHZ_FORMAT_V3`),
   which graft-m1-edge enables (`tests/am1/hosted/deploy-edge.sh`). The task said "`FORMATS_ENABLED` unchanged", so
   they were not added.
2. **graft-m2b has no `graft-ops`.** The Worker `graft-m2b-ops` does not exist on the account (wrangler: "This Worker
   does not exist on your account. [code: 10007]"). The edge has no `OPS` binding, so `POST
   /app/installations/{id}/access_tokens` answers 503 "Binding `OPS` is undefined." for m3-ci and for all three
   install-owned Apps. m3-rows did not deploy one either ("`graft-m2b-ops` was not redeployed", `m3-rows.md`).

The am1-rest plan said "with the install's `FORMATS_ENABLED` unchanged: no new format". That holds for record
formats, because no new format was taken, but not for which formats graft-m2b *enables*. That was my error in the
plan.

## Deployed versions on graft-m2b

| Script | Version | Commit | Time (UTC) |
| --- | --- | --- | --- |
| `graft-m2b-site-a` | `6ec447e8-6fbb-414b-b0e4-c5b28c053b09`, then `7d0693fb-0be7-4414-80f2-61cbfbaee07a` | `612c5f4` (= `0172f55` + test drivers) | 12:18Z to 12:31Z |
| `graft-m2b-site-b` | `a41dd8d6-c6c4-40cb-90e5-df1f480caa82`, then `3adb655f-a2eb-468b-aa35-ec8dc88663f3` | `612c5f4` | same |
| `graft-m2b-site-c` | `710fc575-1c53-42e1-8c86-c226a5088c72`, then `bc1d0916-2eee-4cf7-8b8a-0ba39a5d10ae` | `612c5f4` | same |
| `graft-m2b-edge` | `aaf60641-b40c-4563-9d3b-eb212ce5995b` (was `d560e7d4`) | `612c5f4` | 12:32Z to 12:34Z |

- `612c5f4` differs from `0172f55` only in `tests/am1/runs/` (the rotation script and the three deploy drivers). No
  crate, worker, or container file differs.
- **Shard image:** `graft-m1-site-gitshard-git@sha256:fb90615a825392d9521505dd662d9a38e1cc3b0af38ff5a5d1206a652249d5b1`
  (`:am1runs`, built from `containers/git` at `0172f55`, the session-ref build), via `tests/am1/runs/image.sh`.
- **Site day:** `tests/am1/runs/siteday.sh`, which runs `workers/site/deploy.mjs --test-faults --anchor … --image …
  --roll-shards <edge>`, two passes, with `GRAFTYARD_AUDIT` unset as before. The first attempt stopped before
  deploying anything, because wrangler needs the image's full `registry.cloudflare.com/<account>/…@sha256:` form. It
  was rerun with that form.
- **Roll:** 70 running shard instances moved off the old image (`…43264a8c37bd`). All 70 answered `kill 200,
  catchup 200`, including bf20 on shards 0 and 1.
- **Edge:** `tests/am1/runs/deploy-edge.sh`, with `FORMATS_ENABLED` unchanged, the seed's `LOGINS`, and no `--ops`
  (none existed before either).

## Step 1: token rotation

`tests/am1/runs/rotate.mjs` handled `~/.graft-m2b/tokens.txt` lines 1–3: owner `m2b` 7000001, writer `m2b-writer`
7000002, and reader `m2b-reader` 7000003. These were taken to be the three tokens that leaked. Line 4 was already
revoked by the seed and was kept unchanged. For each line, the script:

1. revoked the old token through the install's own `DELETE /api/v3/applications/178c6fc778ccc68e1d6a/token` (gh's
   public client), with the token in the JSON body only;
2. stored a fresh `ghp_` for the same account as a `Store` on its `Tokens` shard, the way `seed.ts` does;
3. rewrote `tokens.txt` through a 0600 temp file and a rename.

The script prints statuses only. No part of any token was printed, and the backup copy was deleted.

| Line | Old shard | Old `GET /user` before | Revoke | Old `GET /user` 11 s after | Old over git (`info/refs`) | New shard | New `GET /user` |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1 owner | `tokens:2487` | 404 | 204 | 401 | 401 | `tokens:422` | 404 |
| 2 writer | `tokens:1327` | 404 | 204 | 401 | 401 | `tokens:2622` | 404 |
| 3 reader | `tokens:2569` | 404 | 204 | 401 | 401 | `tokens:1772` | 404 |

`GET /user` answers 404 for these seeded accounts both before and after: they have no `Identity` profile the route
renders. 404 rather than 401 is the signal that the token is accepted.

**Seeded drivers with the new tokens:** `tests/m3/repos/seeded.sh smoke`, run twice before the site day and three
times after (`seeded-after.tsv`). Every round matched:

- owner `200 true,true,true`, writer `200 false,true,true`, reader `200 false,false,true`;
- `gh issue list` rc 0, and REST `commits/main/statuses` 200 for all three;
- clones rc 0;
- the owner's and writer's pushes accepted, the reader's refused with 403;
- line 4 (revoked) gets `info/refs` 401.

## Step 3: the rows

Pass and fail lines are quoted from `build/BUILD-PLAN.md`.

| Row | Verdict | Measured |
| --- | --- | --- |
| `apps.mjs` | **Done** | `graftyard-workspace` App 1441792, installation 7190011, `AgentApp GraftyardRun`. `graftyard-agent` App 1507328, installation 7190012, `GraftyardRun`. `graftyard-mcp` App 1572864, installation 7190013, `Mcp`. All on `authz:1`, installed on m3org (7100001); keys 0600 in `~/.graft-m3r/agent-apps/`. Installation tokens for them: **503, no `OPS`** |
| LF-4 | **Pass for the two resources the edge reads over REST.** Pass line: "100% absent from `visible` and counted" | On `m3org/lf4-r` (131168, made by `POST /orgs/m3org/repos`): issues 16 of 16 and releases 16 of 16, with every hidden range absent from `visible`, `invisible_chars` 2, and the raw `body` unchanged. Control without the media type: 0 issues and no release carry `graftyard_trust`, and `Vary: Accept` is set. Not reachable over REST on the edge (as in am1-rest): pull requests, issue comments, reviews, review comments, commits, check runs |
| B0-37 fetch half | **Partial: hiding and refusal shown, session-ref listing not reachable.** Pass line: "every `refs/graftyard/` update not by `graftyard-workspace` is refused, and session refs appear only to a v2 `ref-prefix` that starts with it" | `tests/am1/runs/fetch.sh lf13-r 10` on the new image, as m3-admin. In 10 of 10 rounds, a push to `refs/graftyard/sessions/…` was `[remote rejected]`. v0 `ls-remote` showed 2 refs with 0 `refs/graftyard/`, v2 likewise, and `clone --mirror` 1 ref with 0. A raw v2 `ls-refs` with `ref-prefix refs/graftyard/sessions/` got 200 with 0 lines. No session ref can exist here: only `graftyard-workspace` with a live task writes one, which needs 32.12 and an `OPS` mint. So the "appear to a matching v2 `ref-prefix`" half is not shown hosted; `containers/git/server/sessions_test.go` covers it |
| B0-29 | **Blocked, not run** | `a1.mjs` flags m3-ci (`PUT /api/graftyard/v1/agents/apps/196608`: 503 "need record format 18.3"), mints `ghs` (503, no `OPS`), and opens tasks and leases (503: 32.12 and 32.14) |
| B0-31 | **Blocked, not run** | as B0-29 |
| B0-32 | **Blocked, not run** | as B0-29 |
| B0-33 | **Blocked, not run** | as B0-29 |
| B0-34 | **Blocked, not run** | as B0-29 |
| B0-35 | **Blocked, not run** | as B0-29 (its 500 draft PRs are pushed under `ghs`) |
| L0-12, A1 replay with 500 pollers | **Blocked, not run** | as B0-29, plus leases (32.14) |
| L0-12, refusals with the Graftyard-run and MCP kinds | **Blocked, not run** | `agentToken("graftyard-workspace")` and `agentToken("graftyard-mcp")`: 503 "Binding `OPS` is undefined." |
| LF-13, gap and install, 3 runs each | **Blocked, not run** | Its commits are lease acquires and releases under a task (32.12 and 32.14) and a `ghs` token. The feed carries only lease, task, agent-push and agent-PR events, all behind agent formats, so a non-agent commit source would leave "no event is lost" with nothing to count. Its repository `m3org/lf13-r` (131169) is made |
| B0-36 rerun with the Graftyard-run and MCP kinds | **Blocked, not run** | no `OPS` (tokens), 18.3 (flags) |
| B0-37 push half rerun | **Blocked, not run** | flags (18.3), tasks (32.12), `ghs` |
| L0-13: plan blocks, the holder losing push access, the 5 h renew | **Blocked, not run** | leases (32.14) |
| B0-30 | **Not run**, as instructed | |

Probe log: `tests/am1/raw/runs/probe-blocked.log` (statuses only).

## What unblocks the rest

Both items are reversible and cheap, but each goes past what this task allowed, so they wait for your OK:

1. **Redeploy `graft-m2b-edge` with 32.8, 32.12, 32.14 and 18.3 added to `FORMATS_ENABLED`**: `bash
   tests/am1/runs/deploy-edge.sh <log> 32.8,32.12,32.14,18.3`. This is safe on this site day: `family.rs`'s `reads()`
   for `Repo` already lists `REPO_FORMAT_AGENT`, `_TASKS` and `_LEASES`, and `Authz` lists `AUTHZ_FORMAT_V3`, so every
   graft-m2b member now reads them. No site day is needed.
2. **Stand up `graft-m2b-ops`**, as m3-hosted did for graft-m1:
   - `tests/m3/hosted/tools` `mintkey 1 <0600 file>`;
   - a Secrets Store secret holding that text, with the value from the file on stdin;
   - `node workers/ops/deploy.mjs graft-m2b-ops <store id> <secret name>`;
   - commit the printed `PutVerifyingKey` on `tokens:0`;
   - redeploy the edge with `--ops graft-m2b-ops`.

   This creates a new Worker and a new secret on the account, which is why it was not done unasked.

After both, the plan runs as written (`am1-rest.md`, "Hosted run plan"), at the same estimate: about $5.44 upper
bound, about 3.5 h. No row is near the $15 stop line. B0-33 is the largest at $1.97.

## Costs incurred

- Two site-script deploys per site (six versions), one edge deploy, one image push, about 70 shard rolls, and fewer
  than 1,000 API and git requests: rotation, five seeded rounds, `apps.mjs` (21 commands), LF-4 (32 writes and about
  20 reads), and 10 fetch rounds.
- None of it is a load run. By `run_cost.py`'s per-operation figures this is well under $0.05 [inference: no line of
  `run_cost.py` covers deploys, and the request counts are about 1/1000 of one B0-29 run's $0.50].

## Design contradictions

None found in this round.

## Bf20 numbers, kept for reference (not loaded)

From `tests/m3/results/wire-fix.md`:

- 571,872 refs;
- receive-pack `info/refs` is 41,954,822 bytes, streamed in 24.2 s;
- a v2 `ls-remote` takes 1.7 s;
- a hand catch-up of 58 pages took 140.6 s.

In this site day's roll, bf20 answered `kill 200, catchup 200` on shards 0 and 1.

## Token scan

A scan of every file under `tests/am1/raw/runs/`, `tests/am1/raw/lf4.jsonl` and `tests/am1/runs/` checked for:

- the exact text, and the first 12 characters, of every token in `~/.graft-m2b/tokens.txt` (new) and
  `~/.graft-m3r/m3-tokens.txt`;
- the bed token and the tools token;
- any `gh[pousr]_`, `github_pat_` or JWT shape.

0 hits in 20 files: the raw logs, `lf4.jsonl`, the drivers, and this file.
