# stream-push: the edge streams a push's pack, 2026-11-14

Branch `stream-push`. Before it, `read_capped` in `workers/edge/src/lib.rs` read each receive-pack body into one `Vec`, and `receive_pack` then copied it into the validate body and each bucket's put. On `graft-m2b`, every 60 MB push in LM-1's B0-34 seed ended `exceededMemory` or with a hung exception on the edge. B0-20 (`design/DESIGN.md`, Milestone 0) fails a push that "needs buffering", so this is a code gap, and the design row stays as written (`decisions/DECISIONS.md`, "Pushes stream through the edge").

## Result

| Push | How it was sent | Outcome | Edge max memory (wasm) | Time |
| --- | --- | --- | --- | --- |
| small (one 6-byte file) | git 2.47 from the orb, workers.dev | landed | 40 MB across the 02:30 to 02:33Z window | 6.0 s |
| 2 MiB | git, workers.dev | landed | (same window) | 6.4 s |
| 60 MiB, three pushes | git, workers.dev, Pro vars (cap 80 MB) | 3 of 3 landed; three bucket copies of 62,933,962 bytes each | **40.6 MB** (8 MB wasm) over 34 invocations | 25 to 27 s |
| 70 MiB, generated pack | driver Worker over a service binding | landed | 36.5 MB (6.4 MB wasm) | 30.5 s |
| 500 MiB, generated pack | driver Worker, Enterprise vars (cap 4.5 GB) | landed; 524,330,951 bytes in each of three buckets | **31.7 MB** (5.4 MB wasm) | 118 s |
| 2 GiB, generated pack | driver Worker, Enterprise vars | landed: `refs/heads/sp-drv-2gib` names the manifest's commit; 2,147,659,031 bytes and a `Committed` marker in all three buckets | **36 MB** (5 MB wasm) | about 10 min (sent from about 02:46Z; pack complete in the buckets 02:56:15Z) |

The edge's memory did not grow with the pack's size: 31.7 to 40.6 MB from 60 MiB to 2 GiB, against B0-20's 100 MB. The old edge on the same install, with the same 81 MB body, ended `exceededMemory` at 272 MB (268 MB wasm) and with a thrown exception at 158 MB, its peak over the probe run (02:20 to 02:26Z, version `13599b59`). The new edge's Workers Logs from 02:27:30Z to 02:55Z show 58 invocations, every one `ok`.

Each memory figure is the maximum `memoryUsageBytes` that `workersInvocationsAdaptive` reports for `graft-m3f-edge` in the push's window, so the 60 MiB figure is the maximum over 34 invocations, including the git client's other requests.

## How the large pushes ran, and what it leaves open

`graft-m3f`'s edge is on workers.dev, whose body limit is 100 MB [measured, `tests/m0/results/m0-wp6.md`], and the install has no Enterprise zone. So the 500 MiB and 2 GiB bodies were sent inside Cloudflare: the scratch Worker `graft-m3f-sp-driver` (`tests/streampush/driver.js`) built each pack from a manifest (`tests/streampush/packgen.mjs`: 64 MiB blobs of xorshift data in zlib stored blocks, checked locally with `git index-pack --strict` and `git fsck`) and streamed it to the edge over a service binding. For days B the edge ran with `PLAN=enterprise` and `EDGE_CAP_BYTES=4500000000`; the code path is the same on every plan. A service binding accepted a 2 GiB streamed body. That is this run's observation, not a documented limit.

What this does not show: a 2 GB push from a git client through an Enterprise zone. B0-20's Enterprise half still needs that zone. The hosted pushes measure the edge's memory on the same streamed path such a push takes.

The first 2 GiB attempt sent an empty pack: `mib << 20` overflows 32 bits in JavaScript at 2,048 MiB, so the generator produced no blobs and the edge correctly committed a commit with no blobs (`sp-drv-2g`, kept in `tests/am1/raw/streampush/drv-2g-empty-overflow.json` and not counted). The generator now multiplies.

## Refusal paths: byte diff against the main-era edge

`tests/streampush/probes.py run` posted 17 receive-pack bodies to `m3org/stream-push`, first to the edge that was running (`13599b59`, main `5293a6c` + `rehydrate-batch`, 02:20Z), then to the new one (`cfbec8e7`, 02:28Z). It compared status, kept headers, and body bytes (`tests/am1/raw/streampush/probes-{before,after}.json`).

**15 of 17 are byte-identical:** the bare flush (200, empty), an empty body, a bad command at 100 bytes and at 3 MB, no flush, `shallow`, no pack, too many atomic refs, a non-pack at 2 KB and at 3 MB (validate's 422 as `unpacker error`), the cap refusal with `report-status-v2` and no side-band (81 MB), a gzip body, a corrupt gzip body (500), placement failing at 3 MB (503), and an anonymous push (401).

**The other 2 changed from a crash to the intended answer:**

- 81 MB over the 80 MB cap, `report-status side-band-64k`: old edge 503 `error code: 1102` (its isolate past its memory limit), new edge 200 with the cap refusal (`Push refused: over 80 MB ...`, then `ng refs/heads/sp-probe push over HTTPS cap, split the push`).
- 81 MB with placement failing (`x-graft-place-fail`): old edge 500 `error code: 1101`, new edge the same cap refusal, as the old code meant to answer: it read the whole body against the cap before it checked placement.

The third 81 MB probe, with `report-status-v2`, answered the cap refusal on both edges: that isolate happened to survive. Same input, same answer whenever the old edge did not crash.

## What changed

- `graftyard_git::smart::BodyReader` holds a body only while its pack may still be inline: until the commands parse and more than `INLINE_MAX` of pack follows them. Such a body, and every gzip body, takes the old buffered path unchanged. A body with bad commands is held to `INLINE_MAX` and then only counted, so it answers the same 400. Every byte counts against the cap, and the first 64 KiB are kept for the cap refusal.
- `PackStore::place_streamed` (`crates/git/src/store.rs`) tees the rest: SHA-256 and length as it reads, a one-chunk pipe to validate, and two part buffers (`JsPart`, a `Uint8Array` outside the wasm heap, handed to R2 as a view), each put to every bucket still taking the pack before it is filled again. A bucket whose put fails drops out, and the tee stops once two regions cannot hold the pack. The `DurablePack` still needs validate's index to name the SHA-256 and length the tee read.
- A streamed validate goes to the first shard of `shard_order` only, with no trailer and the 60 s wait. A non-422, non-429 answer is `shard_unavailable` 503, because the body cannot be sent twice.
- A refusal the edge gives before the pack (atomic refs, format, `COMMAND_MAX`, placement failing) still reads the rest of a streamed body first, so a body over the cap gets the cap refusal, as before.

## Checks

- `bash tests/am1/runs/checks.sh` on `06c55c2`: fmt; clippy for the workspace, and for wasm32 site and edge with and without `test-faults`, and ops; `cargo test --workspace`; `check_design`, `check_names`, `check_links`; the Go tests. All RC=0.
- Kept tests that fail on the buffering code: `the_edge_holds_a_push_body_only_while_its_pack_may_be_inline` (plant 0, the old buffering behavior, is caught by it) and `a_streamed_pack_holds_two_parts_whatever_its_size` (a third buffer, plant 9). Also `a_push_body_the_edge_does_not_stream_reads_as_when_it_was_held_whole`, `a_streamed_pack_is_durable_only_on_the_bytes_validate_indexed`, `a_streamed_pack_that_breaks_off_places_nothing`, and `a_streamed_body_counts_its_head_against_the_cap`.
- `python3 tests/streampush/plant.py`: **14 of 14 caught.** The plants are: never stream; stream at exactly `INLINE_MAX`; stream as soon as the commands end; hold a long bad body; answer a short bad body early; the cap off by one; count only the held bytes; the gzip reader parsing; the streamed rest counted from zero; a third part buffer; no SHA-256 check; no length check; a failed bucket kept live; open uploads not aborted. The first run caught 12; the two misses led to a test for the bad-body cap count and to dropping a duplicate abort in the tee.

## Deploys and cost

Edge days only, logged in `tests/am1/results/install-m3f.md`: `cfbec8e7` (Pro vars), `dbb28d4a` (Enterprise vars), `f375f0eb` (Pro vars again, the version left running), all from `sp-deploy` = `rb-deploy` `119fcaa` + this branch, because the m3f sites run `rb-deploy`'s build and main's edge binds classes they lack. Sites unchanged. Shards torn down after: 22 of 22 answered 200, and no `graft-m3f` shard container is running. `m2bcost.py` for 02:15 to 03:05Z: **$1.76** (containers $1.73).
