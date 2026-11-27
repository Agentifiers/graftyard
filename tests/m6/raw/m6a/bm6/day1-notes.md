# BM-6 prep day (day 1), graft-m6a, 2026-11-27

Run from main 70aa964b, a clean checkout on branch bm6-day1, one `tests/m6/hosted/bm6.sh` phase per log here.

- start 00:05:41Z. Image `graft-m6a-gitshard@sha256:45243417…` (image.log). Edgeboot (test-faults) passed with 0 traps.
- Site day 00:19–00:33Z on a, b, c and the spare, with `--export-bucket graft-m6a-export`; edge day with
  FORMATS_ENABLED = live plus 17.5 (day.log).
- Site crons stay at `*/15`: `bm6.sh day` does not reset them to `[]` as `bf53-day.sh` does, by the orchestrator's
  ruling (2026-11-27), because the span needs the sites' normal behaviour. That is how this row differs from bf53.
- Bundles R1 b4a1f526…, R2 fd2dc2f9…, 20 formats each including 17.5 (bundles.log).
- Deviation, ops: the deploy token minted with Workers Scripts Write and Containers Write made graft-ops' own upload
  answer 400 10021 "Secrets store binding authorization failed" (OPEN-QUESTIONS A54). Reminted at 00:41Z with Secrets
  Store Write added, it uploaded (ops-token-fix.log). `bm6.sh ops` now mints with that third group.
- Roll R1 done 01:36:41Z, R2 done 02:38:24Z (one undo per step), Workflows included both times: no fallback
  (roll-r1.log, roll-r2.log, tags-2.log: every site on fd2dc2f9).
- FormatsEnable 17.5: 201 at index 1314 (enable.log). Deploy token 200 before revoke, 401 after (revoke.log).
- Seed through the bench's test-only `/op` (R14): owner r66-bm6-1879, repository r66-bm6-1879/bm6, id 131072, two
  3 MiB stored packs, one inline push, branch f, PR #1 open (seed.log, seed-pushes.jsonl, refs-seed.txt).
  LifeboatAdd for lifeboat.bm6.test: 201 at index 1316 (init.log).
- Warm shards: the seed's client pushes warmed net:131072's shard on sites a, b and c, which a client request keeps
  `WARM_FOR` (24 h); 266's lapse covers only a move's warm. All 3 were still active at 03:03Z. By the orchestrator's
  ruling they were stopped at 03:05Z through each shard's `/teardown` (`bf50-teardown.sh`, a scratch Worker, no site
  script version), so the span starts cold and no 24 h lapse falls inside it (teardown.jsonl: 3 × 200).
