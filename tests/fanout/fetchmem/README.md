# fetchmem: site memory with stored packs streaming four at once

`run.sh` runs `worker.js` in local workerd (`wrangler dev`, the R2 binding persisted under `/tmp/fetchmem-state`) and
`sink.py`, a stand-in for the container's `/spool` that reads each body at `RATE` bytes/s per connection. It seeds four
R2 objects of `SIZE` bytes (207 MB, repo:131296's largest promoted pack), restarts workerd so seeding's heap is gone,
then samples the isolate process's RSS every 200 ms while `/fetch?k=K` sends the first K objects to the sink at once.
`/fetch` does what `ShardHost::put_stream` (`crates/git/src/host.rs`) does for `/spool` and `/spool-idx`: the R2
object's body is the request body, so no byte of it is copied into the isolate. `mode=buffer` is the control: each
pack read whole with `arrayBuffer()` first, as a path that buffered one would.

Run: `bash tests/fanout/fetchmem/run.sh` (about 1 minute; wrangler from `tests/m0/node_modules`). `SEED=8
QS="k=1 k=4 k=8 k=1&mode=buffer k=8&mode=buffer"` seeds eight objects and picks the queries.

Result, 2026-11-15, cited by `decisions/DECISIONS.md`, "`fetch-ahead`":

| Fetch | Packs at once | Peak RSS | Wall |
| --- | --- | --- | --- |
| idle after restart | | 113 MB | |
| streamed | 1 × 207 MB | 115 MB | 3.46 s |
| streamed | 4 × 207 MB | 118 MB | 3.49 s |
| streamed, again | 1 / 4 | 118 / 119 MB | 3.46 / 3.46 s |
| buffered (control) | 1 × 207 MB | 532 MB | 3.84 s |
| buffered (control) | 4 × 207 MB | 1,849 MB | 4.81 s |

Eight at once, 2026-11-16 (LM-1 round 27b, `SEED=8`), for raising `FETCH_AT_ONCE` from 4 to 8:

| Fetch | Packs at once | Peak RSS | Wall |
| --- | --- | --- | --- |
| idle after restart | | 113 MB | |
| streamed | 1 / 4 / 8 × 207 MB | 115 / 118 / 122 MB | 3.46 / 3.48 / 3.46 s |
| streamed, again | 1 / 4 / 8 | 122 / 122 / 123 MB | 3.50 / 3.46 / 3.46 s |
| buffered (control) | 1 / 8 × 207 MB | 537 / 3,961 MB | 3.80 / 6.57 s |

Local workerd does not enforce the six connections waiting for headers per invocation: eight `/spool`-shaped calls
that answer only after the whole body ran all at once here, which the hosted runtime queues past six [doc,
`research/cloudflare.md`, Workers limits]. So this table bounds memory only.
