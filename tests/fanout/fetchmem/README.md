# fetchmem: site memory with stored packs streaming four at once

`run.sh` runs `worker.js` in local workerd (`wrangler dev`, the R2 binding persisted under `/tmp/fetchmem-state`) and
`sink.py`, a stand-in for the container's `/spool` that reads each body at `RATE` bytes/s per connection. It seeds four
R2 objects of `SIZE` bytes (207 MB, repo:131296's largest promoted pack), restarts workerd so seeding's heap is gone,
then samples the isolate process's RSS every 200 ms while `/fetch?k=K` sends the first K objects to the sink at once.
`/fetch` does what `ShardHost::put_stream` (`crates/git/src/host.rs`) does for `/spool` and `/spool-idx`: the R2
object's body is the request body, so no byte of it is copied into the isolate. `mode=buffer` is the control: each
pack read whole with `arrayBuffer()` first, as a path that buffered one would.

Run: `bash tests/fanout/fetchmem/run.sh` (about 1 minute; wrangler from `tests/m0/node_modules`).

Result, 2026-11-15, cited by `decisions/DECISIONS.md`, "`fetch-ahead`":

| Fetch | Packs at once | Peak RSS | Wall |
| --- | --- | --- | --- |
| idle after restart | | 113 MB | |
| streamed | 1 × 207 MB | 115 MB | 3.46 s |
| streamed | 4 × 207 MB | 118 MB | 3.49 s |
| streamed, again | 1 / 4 | 118 / 119 MB | 3.46 / 3.46 s |
| buffered (control) | 1 × 207 MB | 532 MB | 3.84 s |
| buffered (control) | 4 × 207 MB | 1,849 MB | 4.81 s |
