# learner-dumps: learners serve journey 9's whole-tier API reads

Branch `learner-dumps`, from main 0a1d60c plus origin/lf4-rest-2 9612268 (site code that graft-m3c-edge's routes need). Closes K9 (`decisions/OPEN-QUESTIONS.md`); decision record `decisions/DECISIONS.md`, "K9: learners serve whole-tier reads from one shared dump, and are priced". Hosted on `graft-m3c`, 2026-11-12.

## Summary

At journey 9, 500 agents' REST and GraphQL reads were three quarters shed on the repository's learners. The learners were not short of CPU or memory: they shed at `HEAVY_READS` and `DUMP_EVERY_MS`. Learners now share one serialized dump per tier state across every read, are not held to `HEAVY_READS`, rebuild at most every 500 ms, and hold a read whose rebuild is due instead of shedding it.

On the same install, repository shape, learner placement, and 300 s load:

| `read:api` (500 agents, 200 reads/s) | Before | After |
| --- | --- | --- |
| 200 | 13,305 | 45,194 |
| 503 | 38,485 | 2,552 |
| Skipped at the driver's 400 in flight | 7,435 | 10,737 |
| Served, of answered | 25.7% | 94.7% |
| p50 / p99 | 1,681 / 5,125 ms | 1,885 / 4,999 ms |

Everything else on the group got better too, because shed API reads had been retried and their 503s held the edge:

| Same run | Before | After |
| --- | --- | --- |
| `read:ref` served, of answered | 72.7%, p99 82.9 s | 99.8%, p99 6.6 s |
| Agent pushes ok | 761 of 1,251 (384 × 503 on access rows) | 1,183 of 1,234 (1 × 503) |
| Human GraphQL writes ok (`side:human`) | 102 of 503 | 464 of 483 |
| Merges 200 | 3 of 49 | 24 of 51 |
| Leader change | 0 → 1 | none |
| Commit p99 (`/stats`) | no samples | 1,308 ms (3,184 commits) |
| Feed polls answered `lagging` | 8,543 | 0 |
| Peak member memory, voters | 264 / 109 / 302 MiB | 128 / 101 / 96 MiB |
| Peak member memory, learners | 56 to 71 MiB | 61 to 68 MiB |
| Learner CPU in 300 s | 26 to 60 s (9 to 20%) | 55 to 105 s (18 to 35%) |

API read p99 did not move, and it is not within a 1 s target. Member reads are: learner `/read` wall p99 was 1.2 to 1.3 s, p50 0.6 to 0.9 s. The rest is on the edge, which spends 68 ms of CPU (p99 171 ms) and about 2 s of wall time per 200 parsing and rendering a 1 MB dump (edge GraphQL p50 2,070 ms, p99 5,031 ms). So the reads are served, not shed, at a learner latency inside the B0-18 learner gate (p99 1 s for narrow reads, here 1.2 to 1.3 s for 1 MB dumps), and the API latency is now set by the edge's whole-tier answer, not by the learners. That is the narrow-read change below, recorded on F65.

## Which routes ask for a dump

a1.mjs's `read:api` is half `GET /api/v3/repos/m3org/R/issues?per_page=30` and half GraphQL `repository { pullRequests(last: 10) { nodes { number isDraft mergeable } } }`. In `workers/edge/src/api.rs`, every REST repository route resolves through `read` or `repo_name`, and GraphQL through its repository query, to `read_repo(..., Rows::All, ...)`, which calls `tier_rows` with `{"rows": true, "ranges": true, "packed": true}`: the whole packed dump of both tiers (1.06 to 1.08 MB on a 500-PR repository). Only a merge reads `{"rows": {"merge": id}}`, and the read gate and agent pushes read `{"rows": {"access"|"agent": true}}`. So at journey 9 every API read is a whole-tier read; there is no subset of routes to move.

Round 5 on `graft-m2b` (B0-29, repo 131255, 19:46 to 19:56Z, read-only telemetry) had the same shape: edge GraphQL 15,158 × 200 and 23,220 × 503, REST 14,557 × 200 and 22,898 × 503; learners used 34 to 60 s of CPU in 600 s and peaked at 51 to 60 MiB.

## The change

Commits on `learner-dumps`: 6204808 (the change and its plants), 4ec1bf0 (a named cache type for clippy, and the run scripts in `tests/ld/`), then the design, cost, and results commit.

- `workers/site/src/family.rs`: `dump_bytes` builds the packed dump once per `DumpKey` (the tiers' own meta rows) as `Rc<[u8]>` and caches it per repository; `Repo::read_json` and the new `Family::read_bytes` both answer from it. `dump_every(me)` is `LEARNER_DUMP_EVERY_MS` (500 ms [guess]) on a learner's address and `DUMP_EVERY_MS` (1 s) on a voter's; `dump_wait` takes it.
- `workers/site/src/host.rs`: a granted read's answer is `fold_answer`: `Shed` when a stale dump's rebuild is not due, else the shared bytes, else `read_json` serialized. Before, each input that granted reads serialized its own copy of the dump (`shared_answer` shares only within one input).
- `workers/site/src/member.rs`: `gate` takes the member's address; `HEAVY_READS` does not hold on a learner, `READS_MAX` does, and a follower and the leader are unchanged. A learner's read-index read answered `Shed` waits the rebuild's ms and is granted again, while the rebuild comes before its `FOLLOWER_WAIT` deadline (`waits_for_rebuild`); it then answers the state at its new grant, so it is still fresh. `/stats` adds `waitedReads` and `dumpBuilds`.

No edge change and no wire change.

### Tests and planted bugs

- `member::tests::a_learner_serves_whole_tier_reads_past_the_heavy_cap_and_waits_for_a_due_rebuild`: a learner past `HEAVY_READS` serves; `READS_MAX` holds on it; a follower and the leader keep `HEAVY_READS`; a learner's leader-only read still names the leader; only a learner's read-index read waits, and only for a rebuild before its deadline (499 ms waits, 500 ms at the deadline does not).
- `fanout::tests::a_dump_is_built_once_per_state_and_shared_by_every_read`: two reads at one state get the same `Rc`, built once; the bytes equal `read_json`'s; an applied entry rebuilds; another repository at the same index gets its own; narrow and merge reads are not dumps; two grants through `host::fold_answer`, one with a voter's spacing and one with a learner's, share the cached copy with no build; `dump_every` by address.
- `fanout::tests::dump_rebuilds_are_spaced_but_cache_hits_are_not` now also checks a learner's own 500 ms spacing.
- `python3 tests/lm1mem/plant.py 41 … 52`: 12 of 12 caught. A follower exempt from `HEAVY_READS`; `HEAVY_READS` kept on learners; `READS_MAX` off on learners; a voter waiting for a rebuild; a wait past the deadline; every read building its own copy; the cache keeping another repository's bytes; a merge read answered with the dump; learners spaced as voters; voters spaced as learners; spacing that ignores the member's interval; the host skipping the shared dump. The earlier round's moved strings (6, 9, 39, 40) were updated and are caught too.

## Cost

Learners were priced at zero (`scripts/cost.py`: "no group in TIERS reaches that rate"). They now have a line in `scripts/cost_agents.py`. `GroupLoad` counts a whole-tier read as 50 narrow reads (`LEARNER_READS` 1,000 / `LEARNER_TIER_READS` 20, measured above at about 19 per learner), the rest of the group's reads once, and `place_learners` adds `ceil(load / 1,000)`. Each learner holds both tiers (8.3 GB, as `--a1`'s member bytes), applies every entry (log row plus fold rows), and is active 9 hours a day [guess]:

| Largest network | Whole-tier API reads/s | `GroupLoad` | Learners | Storage | Duration | Applied rows | Requests | Per month |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 50 devs | 5 | 253 | 0 | $0 | $0 | $0 | $0 | $0 |
| 300 devs | 30 | 1,518 | 2 | $3 | $3 | $5 | $5 | $16 |
| 2,000 devs | 200 | 10,122 | 11 | $18 | $17 | $169 | $33 | $237 |

The agent layer goes from $21, $274, and $2,572 to $21, $289, and $2,808 a month (9%, 24%, and 31% of the base). No price in `research/prices.json` changed. 11 learners is more than the 8 the hosted run used; 8 served 94.7% at 18 to 35% CPU, so 11 is the placement rule's answer, not a measured need.

Hosted spend for both runs and the site day: $5.14 (`INSTALL=graft-m3c T0_FILE=tests/ld/raw/ld-t0.t0 bash tests/fanout/cost.sh`), under the $25 cap.

## Runs

Both runs: a fresh repository with 500 open PRs (B0-35's setup), learners placed with `node tests/fanout/learners.mjs repo:ID enam:4000,weur:2000,apac:2000` (8 learners, `place_learners` giving enam 4, weur 2, apac 2), then `a1.mjs b0-29 --seconds 300 --as-admin` (`tests/ld/run.sh`). `--as-admin` because m3-admin cannot flag the App on `graft-m3c`, so App tokens mint but are refused.

| | Before | After |
| --- | --- | --- |
| Sites | 74dfbef2 / 7866b0b2 / e0156642 (m0-carried's test-faults build: main 9a6d955 plus B0-07 stand-ins) | 59c140dc / f96862ec / 42660230 (`learner-dumps` 4ec1bf0) |
| Repository | m3org/ld-before2, repo:65544 | m3org/ld-after, repo:65547 |
| Dump at load start | 1,079,993 B at index 791 | 1,061,772 B at index 823 |
| Load window | 03:27:56 to 03:32:56Z | 05:24:58 to 05:29:58Z |
| Rows | `tests/ld/rows/a1-b0-29-before2.jsonl` | `tests/ld/rows/a1-b0-29-after.jsonl` |

Learner `/stats` at 05:26Z, about 70 s into the after run (the counters reset with the members at the end of the run): per learner 1,585 to 2,612 read-index reads, 1,102 to 1,929 waited for a rebuild and granted again, 83 to 409 shed, 88 to 132 dumps built, 1,528 to 3,562 answered from another grant in the same input.

`python3 tests/ld/summary.py tests/ld/rows/a1-b0-29-before2.jsonl tests/ld/rows/a1-b0-29-after.jsonl` prints the `read:api`, `read:ref`, and agent push lines above.

## What is left

- API read p99 of 5.0 s: the edge parses and renders the whole 1 MB dump for every list read. Serving `GET /repos/{o}/{r}/issues` and GraphQL `pullRequests` from narrow reads (the rows the route renders plus logins) would cut it and drop the 50-times weight; it is the larger change and was not made here (F65).
- 5.3% of answered API reads still shed: reads past `FOLLOWER_WAIT` on a learner, and the voters' `HEAVY_READS` when no learner answers.
- `LEARNER_DUMP_EVERY_MS` is a guess. The learners rebuilt about every 0.5 to 1 s under journey 9's pushes.
- LM-1's 500-agent rows on `graft-m2b` rerun this (thread T-01a1c694-6112-72b4-83ca-dc9337a9646e).
