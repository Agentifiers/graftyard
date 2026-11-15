# R22: site a's `*/15` cron dies in the sweeper's fill of network 131296 (2026-11-15)

## Summary

Every run of site a's cron on `graft-m2b` since 2026-11-13 dies with "Promise will never complete." in `sweep::run`, while it fills network 131296's third buckets. `graftyard_git::sweep`'s `fill` read each committed pack whole into wasm memory, checked its SHA-256, and put it again. The fills run 16 at once (`CONCURRENCY`), and 131296's first 16 missing copies are 8 to 53 MB each, 328 MB in all. The isolate's memory limit ends the invocation, and nothing is logged, because `sweep {` is logged only after `sweep::run` returns and `fold_audit::run` runs after it. Every run starts again from the same cursor, so it dies on the same network every time.

The row's lead, the first step's `networks` over every bucket with `repo_owner` and `owner_record` per network, is not the cause. It ran to completion in the failing run measured: 351 `Member /read` calls in 206.7 s, then the cursor read, then the fills.

The fix streams the copy from bucket to bucket, so no byte of a pack enters the isolate's memory, and the binding checks each copy's length and SHA-256 against the marker. In local workerd, one sweep of 16 fills of 20 MB went from 382.3 MB of wasm memory and a 1,491 MB peak workerd RSS on `origin/main` to 1.3 MB and 363 MB with the fix, with every copy byte-identical, metadata kept, and markers written.

## The failing case, measured

Account `graft-m2b`, script `graft-m2b-site-a`, version `17d8d623`. Workers Logs (the observability telemetry API), `workersInvocationsAdaptive`, `r2OperationsAdaptiveGroups`, and the R2 object listing API, read on 2026-11-15 at about 13:30 to 15:00Z.

**One run end to end:** the cron at 12:00Z, trace `7c24400cb108dacaf7a186fea5d2aa1f`, 3,925 events across the three sites' scripts.

| Time from 12:00:00Z | Site a's invocations in the trace |
| --- | --- |
| 19.9 s | `GET registry/row?chain=names:195`: the first step starts |
| 20 to 206 s | 351 `POST member/read` calls (`repo_owner` and `owner_record` for each network), 11 to 25 per 10 s, each 0.3 to 2 s of wall time |
| 206.7 s | `GET registry/sweep`: the cursor read, so `networks` and every home lookup finished |
| 262 to 276 s | about 270 `POST member/read` calls on one object (`071f1d3b`, the `request` reads of the network being swept) and the three shards' `warm` |
| 278.6 s | `scheduled` ends `exception`, "Promise will never complete.", wall 279,068 ms, CPU 4,823 ms |

`workersInvocationsAdaptive` for that run: `scriptThrewException`, wasm memory 165.6 MB. Site b's run in the same window ended `success` at wasm 1.9 MB and logged `sweep {"swept":[], ...}`, with nothing of its own to sweep.

**What the run was doing when it died.** R2 GETs on site a's bucket at 12:04:36 to 12:04:39Z, the only GETs of the run, were of network 131296's packs:

| Object (`pack/131296/...`) | Requests | Bytes |
| --- | --- | --- |
| `01M7A527FQJERPT7XYWR3KHDNG` | 2 | 106.3 MB |
| `01M7A54PY7NS5P1RYCHV7GBD07` | 6 | 113.3 MB |
| `01M7A5AQSDE3NZF0DDN1JCJQ6R` | 2 | 40.9 MB |
| `01M7A5MYD6X09DZHFXT6Q7PANE` | 2 | 32.9 MB |
| `01M7A5FRGBPJE800J8ZEQD7P7W` (site b's bucket) | 1 | 8.1 MB |

Each is a fill source. From the three buckets' listings of 131296, the sweeper's plan holds 71 fills (27 into b, 43 into c, 1 into a), 996 MB in all, and these are fills 0, 1, 5, 13 and 8 in plan order: the first phase that reads bytes. The repeated GETs of one key at once are the fills of the same pack into two buckets. The runs at 12:15, 12:30 and 12:45Z and 09:00Z read the same keys, fills 1, 2, 5, 6, 8 and 11 to 15. The plan's first 16 fills are 8.1 to 53.1 MB, 328.0 MB in all.

**Why it began on 2026-11-13.** The last runs that finished, 05:33 to 09:45Z on 11-13 and 03:15 to 03:30Z on 11-14, logged only `markers` and `fills` of at most 24 per network at sizes of about 1 to 4 MB, on networks 131287 to 131301. B0-34's 5 GB monorepo (network 131296) took its pushes at about 09:47 to 10:50Z on 11-13: its multipart journals date from then, and R2 shows its upload journals and markers going to two buckets at a time through those minutes. The first non-ok run was 10:00Z on 11-13, `exceededMemory` at wasm 258.8 MB; then `exception` at 10:15 and 10:30Z (wasm 180 to 217 MB). The runs from 10:45 to 12:15Z finished while the cursor was on other networks. From 12:30Z every run started at 131285, reached 131296, and died. Two runs finished on 11-14, 03:15 and 03:30Z: by then the orphans that networks 131291 and 131292 got on 11-13 near 03:00Z had turned 24 h old, and their deletes (543, then 217 and 1,155 planned) used each run's 600 actions before the cursor reached 131296. The 03:45Z run continued at 131292, reached 131296, and died. The cursor is written only at the end of a run, so every run since starts at 131292 and dies on 131296.

**Ruled out.** The first step's reads (the row's lead): 178 networks, 351 member reads, done at 206.7 s with no error, and its own memory is a `BTreeMap` of 178 entries. The `request` reads: their answers are rows, small. The upload journals of 131296 (6, 69 and 91 in the three buckets) are aborts, which read no bytes. The stored `.idx` copies: 181 objects of at most 6.8 MB.

## The fix

`Bucket::copy_new(from, key, expect)` (`crates/git/src/bucket.rs`): a create-only copy of `from`'s `key` with its custom metadata, streamed from store to store. On the Workers host (`crates/git/src/host.rs`) the source's `get` body is piped into a `FixedLengthStream` of the marker's length, and that stream's readable half is the put's body, with `onlyIf: {etagDoesNotMatch: "*"}` and `sha256` set to the marker's checksum. The runtime moves the bytes; the length is checked as the stream ends and the SHA-256 before R2 stores the object. `fill` (`crates/git/src/sweep.rs`) copies the pack with the marker's size and checksum and the `.idx` with no expectation, then writes the marker, as before.

The DESIGN line on the filler (Shape, `R2Key`) now says the filler streams the copy and the binding checks it.

## Local check in workerd (`tests/r22/bed/run.sh`)

The bed is `crates/git`'s real `sweep` over three R2 bindings in local workerd (`wrangler dev`, tests/m0's wrangler 4.153.0). It seeds 16 packs of 20,000,000 B and their markers in bucket A, restarts workerd, and samples the isolate process's RSS every 100 ms through one sweep, which fills B and C (32 fills, 16 at once). `wasmMb` is the module's linear memory read after the sweep.

| Arm | Idle RSS | Peak RSS | Wasm memory after the sweep | Sweep | Copies |
| --- | --- | --- | --- | --- | --- |
| `origin/main` 4833094d (`GIT_ROOT=` a checkout of it) | 118 MB | 1,491 MB | 382.3 MB | 32 of 32 done, 20.4 s | 16 in each bucket, all bytes right |
| this branch | 118 MB | 363 MB | 1.3 MB | 32 of 32 done, 13.5 s | 16 in each bucket, all bytes right, metadata kept, 16 markers each |

Hosted, the cron's isolate died at wasm 150 to 195 MB, so main's 382 MB here is the failing case's shape; local workerd sets no memory limit. With the fix, workerd's RSS still grows with the bytes in flight: 245 MB over idle at 16 × 20 MB, and 129 MB at 16 × 5 MB (`SIZE=5000000`, peak 247 MB, wasm 1.3 MB, 32 of 32 done in 3.8 s). Locally the destination buckets are in the same process, so that growth is likely the local R2 taking the puts [inference]. `tests/fanout/fetchmem` streamed R2 bodies to a sink outside workerd, and RSS stayed flat at 115 to 119 MB for 4 × 207 MB. This check cannot show whether the hosted isolate's counted memory, which includes the JS heap, stays under its limit while 16 copies stream at once. The site day is the test.

`EDGE=1` runs `/edge` on one 3 MB pack copied into an empty key:
- a wrong SHA-256: `Err` "put: The SHA-256 checksum you specified did not match what we received" (10037), and nothing stored;
- a length one byte short: `Err` "Attempt to write too many bytes through a FixedLengthStream", and nothing stored;
- the right size and checksum: `Created`, and the stored bytes equal the source's;
- the same copy again: `Exists` with the stored SHA-256;
- a missing source key: `None`.

## Tests and plants

- `sweep::tests::committed_pack_in_one_bucket_with_a_marker_is_filled_never_deleted` now asserts the source bucket handed no bytes to the sweeper (`MemBucket::bytes_read`). On `origin/main` with only that assertion and the counter added, it fails: `left: 218, right: 0`, the pack and its `.idx` read once for each of the two fills.
- `sweep::tests::a_multipart_copy_whose_bytes_differ_from_its_marker_is_not_spread`: a multipart copy (no SHA-256 in R2) of the marker's size but the wrong bytes is not copied, and no marker goes beside it. It passes on main too, where `fill` checked the bytes itself; it holds the check now that the bucket makes it.
- `python3 tests/r22/plant.py`: 6 of 6 caught, each by one of the two tests: `fill` reading the pack whole, as main did; copying without the marker's size and checksum; the copy checking the size but not the checksum; writing the marker after a failed copy; skipping the `.idx`; the copy dropping the custom metadata.

## Not done

No site day and no deploy. Site a's cron stays broken on `graft-m2b` until a site day ships this: the next site day (LM-1 or m3f) should see `sweep {` with `network: 131296` and its 71 fills, then `fold audit {`, on site a.
