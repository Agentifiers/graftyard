# B0-34 midx: the multi-pack-index write leaves the writer's section, chain packs open four at a time

Code-only branch `b034-midx` from main 6356ced5, for the orchestrator (thread T-01a17bda). Three parts: the measures
the next row needs, the background multi-pack-index write (`decisions/DECISIONS.md`, "`midx-background`"), and the
edge's chain opens from the second-opinion review's rec 3 (`chain-open`). Records only: G31, a stored index for push
packs. Then mt7 re-judged against the re-based clone line (`FRAME.md`, "Operator decision, 2026-11-20 …"), and the
scope of a Cloudflare-hosted B0-34 client.

## What changed

- **Measures (ship regardless).**
  - `/ingest-batch` answers `midx_ms`, which the shard sums into `stored.midx_ms`; mt7's inline catch-ups reported none.
  - `catch_up` and the rehydrate's tail report `pages_ms`: `room` (`make_room`), `ingest`, `refs` and `save` (cursor or
    place puts), summed over the pages. The site's `shard catch-up stored` line logs it.
  - The edge's `server-timing` carries `chain_open` on a manifest hit and on a composed miss (test installs).
- **`midx-background`** (`containers/git/server/main.go`, `midxAfterIngest`, `midxMs`): after every 50th ingest the
  write starts in a goroutine under the repository's `midxState.mu` (`TryLock`); a trigger beside a running write is
  skipped and stays primed; evict (409), a fresh init and the background repack wait for it. `midx_ms` is now the
  writes that ended since the network's last ingest answer, once each, no longer part of `lock_ms`.
- **`chain-open`** (`crates/git/src/host.rs`, `open_chain` and `open_pack`; `graftyard_git::compose::open_in_order`,
  `CHAIN_OPEN_AT_ONCE` = 4): the code confirmed the review's reading. `open_chain` awaited each pack's `get` and first
  chunk in a `for` loop before asking for the next, and no header left before it returned. Now four at a time,
  in manifest order, each header still checked before the answer starts.
- **G31** (`decisions/OPEN-QUESTIONS.md`): the 4 push packs per hot-network rehydrate with no stored `.idx`. A push's
  own `.idx` cannot be stored as it is: a thin pack's index covers the bases `--fix-thin` appended, and one trimmed to
  the thin pack's objects leaves git unable to read a delta whose base is elsewhere ("cannot unpack … at offset 258"
  [measured, git 2.56.0]). So it needs the fix-thin appendix stored too. Estimate: a hot network's cold rehydrate from
  21–33 s to about 10–12 s; about 140 lines across container, edge and store. Distinct from G28 (the builder's chain
  head), so a new row; Git and storage 31, Total 205.

## Tests and plants

| Test | Fails on main because |
|---|---|
| `TestAMidxWriteHoldsNoIngestOrValidate` | the triggering ingest waits for the held write |
| `TestASecondMidxTriggerIsSkippedNotQueued` | same; with only `TryLock` removed, a second write runs |
| `TestReadersSeeTheOldMultiPackIndexOrTheNew` | same; with the old file removed first, a reader finds none |
| `TestRepackAndFreshInitWaitForAMidxWrite` | (new interaction; planted below) |
| `TestIngestReportsTheMidxWrite` (now `/ingest` and `/ingest-batch`) | ingest 50 answers its own write; batch answers none |
| `a_catch_up_times_room_ingest_and_refs_apart` | no `pages_ms` |
| `a_composed_answer_opens_its_chain_packs_at_once_and_keeps_their_order` | serial opens take 3× the bound |

Existing midx tests wait for the background write (`midxWait`); `TestIngestWritesAMultiPackIndexEveryMidxEveryPacks`
now accepts a second write that covers a few packs ingested beside it.

Plants, every one caught:
- `tests/midxbg/plant.py`, 11 of 11: the write in the section (main's rule); a second trigger queued; a skipped
  trigger resetting the count; the old index removed before the write; evict, fresh init, and the repack each
  ignoring a running write; `midx_ms` never cleared; `/ingest-batch` without `midx_ms`; `/refs` counted with the
  ingest; `make_room` untimed.
- `tests/chainopen/plant.py`, 4 of 4: one at a time (main's rule); `buffer_unordered`; every pack at once; a missing
  pack skipped.
- `tests/b034tails/plant.py` 12 (`midx_ms` dropped): anchor moved to the background write's `ms.Add`, caught.
- Residue: none in `tests/locksplit`, `tests/b034tails`, `tests/midxbg`, `tests/chainopen`.

## mt7 against the re-based clone line

The line is now clone-cache hits' TTFB p99 ≤ 1 s in steady state, and every clone ≤ 5 s through the kill and cold
phases. mt7's gated probes (`read:cloneTtfb`, a1 run `mwo4va3y`, samples split by `x-graft-clone-cache`), client
clock / edge clock:

| Phase | Hits: n, p50, p99, max | Misses: n, p50, p99, max | Clones over 5 s |
|---|---|---|---|
| j9 (steady) | 239, 2.17 s, **5.19 s** / 4.19 s, 5.78 s | 70, 3.17 s, 9.41 s / 9.36 s, 9.41 s | 13 (3 hits, 10 misses) |
| kill | 97, 1.18 s, 3.37 s / 3.33 s, 3.37 s | 25, 2.13 s, 10.37 s / 10.34 s, 10.37 s | **6** (all misses) |
| second (cold) | 151, 2.48 s, 5.04 s / 5.01 s, 6.58 s | 31, 2.67 s, 7.66 s / 7.63 s, 7.66 s | **6** (3 hits, 3 misses) |
| hostile | 230, 1.90 s, 4.40 s / 4.37 s, 5.45 s | 82, 2.72 s, 10.33 s / 9.99 s, 10.33 s | 24 (2 hits, 22 misses) |

**Verdict against the new line: still fails, on both halves.**
- **Hits in steady state:** p99 5.19 s client, 4.19 s edge, against 1 s. Hits are already slow at p50 (1.2–2.5 s),
  which is the `chain-open` regression: hits went from p50 0.6 s to 1.6–2.0 s when `edge-compose` began serving
  them from manifests, and mt7's 2.2 s p50 is the same. The edge clock agrees within 1 s, so the orb is not the gap.
- **Every clone ≤ 5 s through kill and cold:** 6 in kill and 6 in second over 5 s (max 10.4 and 7.7 s). Misses carry
  it; three cold-phase hits pass 5 s too.
- The new line reads "every clone"; at 90% (AGENTS.md, "How to pick work") kill has 116 of 122 (95%) and second 176
  of 182 (97%) under 5 s. Whether "every" admits the 90% rule is the operator's to say; I judge it as written.

## A Cloudflare-hosted B0-34 client (scope, not built)

**What it needs.** One Worker script, `graft-m1-load`, holding a `Load34` Durable Object class as `Load9` did
(`tests/m0/src/wp9/load.ts`), driven from its alarm, created with location hints so clients start near each site.
It calls the deployed edge over its public URL with `fetch`, so it measures the same path a client takes, now from
inside Cloudflare's network instead of through the orb's proxy (A32, A33, R32).
- **Port of a1's B0-34 load** (`tests/am1/rest/a1.mjs`, 1,535 lines; the B0-34 parts are about 600): the Poisson
  scheduler with caps; agent pushes, human git-shaped pushes, side commands, API reads, `ls-refs`, feed polls, the
  depth-1 `cloneTtfb` probes and the hot networks' pushes and whole clones. a1's pack builder (SHA-1 and deflate) runs
  in a Worker with `crypto.subtle` and `CompressionStream`; no git binary is needed except for `tip:move`.
- **`tip:move`** pushes real linux history with `git push`, which a Worker cannot run. The plan's 600 future commits'
  packs are built once on the orb and staged in an R2 bucket of the load script (about 600 small thin packs); the
  client sends each as a receive-pack POST. A one-time tool, about 80 lines.
- **Phases:** the kill at 300 s is a site-side action a1 triggers through the tools edge; the client asks the orb's
  driver to do it (or reads a start time and the orb does it on schedule), so the load script holds no Cloudflare API
  token (the invariant that no request path calls the API is unaffected, since the load script is not a request path
  of Graftyard either way).
- **Clocks and output:** each client records its samples with `Date.now()` (Workers time advances on I/O, which is
  what TTFB is) and writes them to R2 at the end; the orb's existing rows tooling (`rows.sh`, miss503.py) reads that
  JSONL in place of a1's. Tokens come from the same m3-ci installation and the admin PAT, as Worker secrets on the
  load script (set with the site day, never printed).
- **Size:** the port about 900 lines of TypeScript, the stager 80, the orb driver changes about 100; a smoke row
  against `graft-m1` before a gated one.

**Spend per row.** Priced from `research/prices.json` at mt7's measured volume: 250,198 client operations in the
15-minute window (176,161 API reads, 29,607 `ls-refs`, 20,335 feed polls, 3,776 agent pushes, 605 human pushes,
925 clone probes, 4,105 hot whole clones, the rest side commands).
- Durable Objects: 24 clients (6 per region hint) × 1,200 s × 128 MB = 3,600 GB-s, **$0.045**; alarm and control
  requests about 29,000, **$0.004**.
- CPU: about 280 CPU-s (0.5 ms an operation, 5 ms a built pack, 30 ms a hot whole clone read through), **$0.006**.
  [guess: a1's per-operation CPU on the orb is unmeasured]
- R2: results and the staged `tip:move` packs, **$0.005**.
- Outbound `fetch` from a Durable Object is a subrequest, not a billed Worker request. The edge's own requests are
  billed as they are today, since the same load arrives either way.
- **Client total about $0.06 a row.** The row's total stays mt7's: $3.68 on the run meter (site day excluded), so a
  hosted row is about $3.7, inside today's $2–5. No approval threshold is crossed on this estimate. The risk to it is
  CPU: if hot whole clones cost more to read through than 30 ms each, 4,105 of them at 300 ms would add $0.02.
- **What changes in the numbers:** the orb's proxy is gone from both clocks, so `push:human` and clone TTFB should drop
  toward the edge clock (mt7: hits' edge p99 4.19 s against 5.19 s client in j9). The edge clock itself does not
  change; the `chain-open` and midx fixes are what move it.

## Checks

From the worktree, `CARGO_TARGET_DIR=.worktrees/target-am1`, on the branch rebased onto main 6356ced5:
- `cargo fmt --all -- --check` 0; `cargo clippy --workspace --all-targets -D warnings` 0.
- wasm32 clippy: site and edge, each with and without `test-faults`, and ops: all 0.
- `cargo test --workspace`: 1,281 passed, 0 failed.
- `go test ./...` in `containers/git/server`: ok on reruns. The first full run failed
  `TestConcurrentValidatesOfOneNetwork` once ("ingest refs/heads/b3: 422"). It is a flake that predates this branch: at
  `-count=250` on unchanged main it failed 5 times, each an ingest of an empty body ("checksum e3b0c442…, want …")
  that found no aside to adopt. That means the test's writer asked to ingest a sum whose aside a concurrent validate's
  `finishValidate` was replacing (`discard` then rename), and the test sends no body. Not this change; the branch's
  rate (1 in 40) is within main's (5 in 250, about 1 in 50).
- `go test -race -count=3` on the four new tests, `TestIngestReportsTheMidxWrite` and `TestValidateSerialEquivalence`: clean.
- `scripts/check_design.py`, `check_names.py`, `check_links.py`: OK. OPEN-QUESTIONS rows 205 = Total.
