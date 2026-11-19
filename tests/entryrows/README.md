# entryrows: member rows written per entry, as Cloudflare bills them

`run.sh` builds `bed/` and runs it in local workerd (`wrangler dev` from `tests/m0/node_modules`). Each Durable Object applies one entry the way a member's `host::SqlStore` does and answers each statement's `rows_written()`, the counter behind the "Durable Objects SQLite rows written" line. The bed compiles the member's own `workers/site/src/tier.rs` (tier DDL, `seal`, FTS5 rows) and `txn.rs` (the request insert), and `run.sh` checks that its copy of the other member DDL is still verbatim in `workers/site/src/host.rs` and `faults.rs`.

Run from the repository root (about two minutes, most of it the build):

```
bash tests/entryrows/run.sh                         # prints one JSON line per case
OUT=/tmp/entryrows.jsonl bash tests/entryrows/run.sh
```

Cases:
- each of the 48 `crates/git/tests/fold_corpus.json` kinds, applied after the corpus's setup entry, plus an agent push (`Refs,Provenance,PathSet`), a PrSync round (`Refs,TestMerged,ConsumerCursor`), and K12's `DefineLabel`. Each case is run as built and again with every tier table `WITHOUT ROWID` (`wr`). `fold` and `fts` are the tiers' sealed statements; `request`, `log` and `raft_state` are the host's.
- `blob family entry`: a family whose fold is one `fold` row (Actions, Identity, Tokens, Names, Authz, Ids).
- `later deletes`: an expired `request` row, a truncated `log` row, and a `_digest` row past `DIGESTS_KEEP`.
- `test-only arrival`: the leader's `arrival` row on `test-faults` builds.
- `size b=… wr=…`: bytes on disk per row of a two-column-key table, as built and `WITHOUT ROWID`.

`scripts/cost.py`'s `REPO_KIND_ROWS`, `HOST_ROWS`, `REPO_DELETE_ROWS` and `BLOB_ENTRY_ROWS` come from this output (`decisions/DECISIONS.md`, "`k12-rows`" and "`without-rowid`").

Result, 2026-11-19, main `efc92735`: fold-tier rows per entry run from 6 (`Refused`) to 23 (`PrOpened`), and 6 to 14 with `WITHOUT ROWID`. Every repository entry also writes `request` 3, `log` 1 and `raft_state` 1 in its input. `DefineLabel` writes 14 here, against graft-m3f's hosted 15 on followers and 19 on the leader, which add a second `raft_state` write and, on the leader, the 3-row test-only `arrival` (`tests/am1/results/lm1-rows-m3f.md`, K12). A blob-fold entry writes 6 here. Per row on disk, as built against `WITHOUT ROWID`: 122 against 104 B at 65 B of value, 712 against 681 B at 620 B, 1,398 against 4,679 B at 1,000 B, 2,080 against 4,679 B at 2,000 B, and 4,134 against 4,679 B at 3,000 B.
