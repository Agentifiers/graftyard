# inbound-trace: every request a member takes, in arrival order, for F74

Branch `inbound-trace`, built off `main` b79a2b26. It is the instrument the operator asked for after B0-33 round 29
(`tests/am1/results/lm1-memory.md`, "Round 29"): the voters' `/raft` calls spent seconds between the caller's fetch
and the callee's handler entry, and the trace could not say what entered the callee ahead of them. It runs under
`test-faults` only. A product build records no new row and changes no header, wire message, or stored format. Nothing
in `crates/raft` changed.

## What a member records

At the top of `Routes::fetch` (`workers/site/src/member.rs`), before any injected fault awaits and on the same clock
reading as F74's `CallLeft` entry, each request adds one `Stats::trace` row, served by `/trace?since=` as before:

| Kind | `at` | `peer` | `batch` | `index` | `ms` |
|---|---|---|---|---|---|
| `Entered` 25 (new) | handler entry | sender (`x-graft-from`; 255 for an edge or harness request) | `x-graft-call` for a `/raft` call, else 0 | arrival number on this host × 64 + route code (`host.rs` `ROUTES`) | requests open in this host's handler, this one included |

Arrival numbers start at 1 for each host, so a reset object starts a new segment. Route codes are `ROUTES` index + 1,
0 for any other path; the list is append-only, as trace kinds are, and `tests/f74/inbound.py` keeps a copy. The open
count is a guard held for the handler's life (`Inbound::enter`).

A busy leader takes about 330 requests a second (round 29), so the 60,000-row ring now holds about 2 to 3 minutes of
a loaded leader; `tsave.mjs` should run with `EVERY=10000`, as round 24b did.

## The script

`tests/f74/inbound.py tsave-TAG.jsonl [--from ISO] [--to ISO] [--deadline 1000] [--rows out.tsv]` joins each
voter-to-voter `/raft` call's `CallSent` (23) to the callee's `Entered` row by (caller, call id). It takes the out leg
as `calls.py` does, with the pair's skew estimate, and counts the requests that entered the callee during that leg:
those whose arrival number is below the call's own and whose entry stamp is after the callee clock reads
`entered - out`. It prints them by out leg, with the share from the edge (sender 255) and by route, and the open count
at entry. It first prints a completeness check per member: rows, host segments, arrival numbers missing inside a
segment (rows lost to the ring or between `tsave` reads), and rows whose clock runs backwards.

Counting by arrival number is exact on the callee. The window start crosses two clocks and inherits the skew estimate
[inference]; Workers' clock moves only on I/O, so stamps from one turn share a value.

## Checks

- `inbound_rows_number_every_arrival_in_order_and_count_the_open_handlers` (`workers/site/src/host.rs`): arrival
  numbers run 1, 2, 3, 4 with none skipped while handlers end out of order, each row carries its route's code (0 for
  an unlisted path), and the open count is the handlers not yet ended, itself included.
- `tests/f74/inbound_test.py`: a hand-made tsave file with one complete member, one with a lost row and a reset, and
  one call whose leg had three edge requests enter ahead. The script reports `missing 0` and `missing 1, segments 2`
  for the two members, and out 500 ms, 3 requests ahead (2 `/read`, 1 `/suspect`), all from the edge, 5 open.
- Full checks: fmt, workspace clippy, wasm32 clippy for site, edge and ops (site and edge with and without `test-faults`), `cargo test --workspace` (1,010 passed, 0 failed), `go test` in `containers/git/server`, and the three `scripts/check_*.py`: clean.

## The hosted run

With LM-1's next B0-33 rerun on a test-faults build from a tree with this branch, run `tsave.mjs` with `EVERY=10000`
and then `python3 tests/f74/inbound.py tsave-TAG.jsonl --from ... --to ... --rows inbound-TAG.tsv`. It decides
whether the out legs track the requests that entered the callee ahead of them (an input queue) or not (transit).

## Round 29's `/suspect` load, from Workers Logs

Asked with the branch: why the end leader counted 46,384 inbound `/suspect`. Read from graft-m2b's Workers Logs
(telemetry query API, `calculations` and `events` views, 06:30:42Z to 06:35:45Z), no hosted spend.

**Every voter took them, not only the leader.** In the 303 s load window the three voters of repo:131342 (`#1#f0` on
site a; the largest objects on b and c) took 30,039, 29,892 and 42,087 `/suspect`, 99.9% answered 200 with wall p50
0 to 1 ms. Per voter, `/suspect` was 29 to 40% of all its inbound requests (97,461 / 101,541 / 104,299, about 330 a
second each), second only to `/read` (49 to 56%); `/raft` was 4 to 7%.

**The sender is the edge, not the members.** Each `/suspect` shares a trace id with one edge request. 30,072 / 29,953
/ 42,160 reports came from 12,785 / 13,072 / 15,640 distinct edge requests (traces). The edge served 45,986 requests
in the window. A 40-request sample (random over one 15 s slice) traced end to end: 30 `git-receive-pack`, the rest
lease renewals, GraphQL, labels and task edits; each ran p50 81 s and sent p50 11 `/suspect` (p90 26), p50 4 to each
voter (p90 11), in bursts of 1 to 8 spread across the whole request.

**What fires them** (the nearest non-`/suspect` row before each burst in the sample, `crates/lookup/src/lib.rs`):
- `/commit` answered 200 but after `COMMIT_RECEIPT` (2 s): 182 of 512. `commit_hedged` treats a missed receipt as a
  dead leader and reports it to the other voters.
- `/read` 409 at `repo:131342#9#f1` on site b, a member not joined (an old generation's name), then a lookup: 133.
  That object answered 59,114 `/read` 409s in the window. A lookup that finds no leader in its pass reports every
  member that missed.
- `/read` answered slowly (wall p50 30 ms here, but past `CALL` on others), `/outcome_by_request`, `/place`: the rest.

**Repeats.** `SUSPECT_GAP` (500 ms) dedups only within one edge isolate, per (chain, member, term, all), and the term
the edge knows lags during churn. Within one traced request, repeats to the same voter came p50 5.5 s apart (p10 422
ms), and in one 15 s slice on site a, 690 repeats from the same trace came p50 911 ms apart. So most reports are not
duplicates in one burst: they are the same requests reporting again on each retry, for the 80 s each request runs.

**Reading [inference].** `/suspect` is an amplifier, not the cause: slow receipts (the legs above) make the edge
report the leader, the reports add about a third of every voter's inbound load, and that load lengthens the next
receipts. Cutting it alone removes up to 30 to 40% of voter inbound; `/read` is still half. The 409 stream from
`#9#f1` is a separate defect: a stale member name the edge keeps asking first.
