# C15: FTS5 in Durable Object SQLite

The hosted probe for `decisions/OPEN-QUESTIONS.md` C15, run on 2026-11-13 before M4-WP2 built search on FTS5. It answers yes [measured].

The probe was `probe.ts`, deployed as its own throwaway Worker `graft-c15-fts` with `wrangler.jsonc`, never as a site script. It created one `Fts` object with no jurisdiction and one under `eu`, and ran the same checks in each. The results are in `run-none.json` (the object ran in DFW) and `run-eu.json` (WAW).

| Check | No jurisdiction | `eu` |
| --- | --- | --- |
| `sqlite_version()`, `PRAGMA compile_options` | refused (`SQLITE_AUTH`) | refused (`SQLITE_AUTH`) |
| Contentless FTS5 with `contentless_delete=1`: insert, `MATCH`, delete by rowid | pass | pass |
| `porter unicode61` content table, 5,000 documents | pass | pass |
| `bm25()` and `snippet()` | pass | pass |
| Term, column filter, prefix, phrase, and stemmed counts | pass | pass |
| Filter on an unindexed column with rank order | pass | pass |
| `trigram` tokenizer | pass | pass |
| `optimize` and `integrity-check` | pass | pass |
| Transaction rollback leaves the index unchanged | pass | pass |

`persist.txt` covers persistence. The probe counted 5,000 documents and one match, reset the object with `ctx.abort`, and counted the same 5,000 and one match after the restart, in both jurisdictions. The database with 5,000 documents of about 45 words was 2.2 MB.

Not learned: the SQLite version and compile options, because the platform refuses both functions. M4-WP2's in-memory tests run `search::DDL` and every `query_sql` shape on bundled SQLite through `rusqlite` 0.37. The one shape the probe did not run is bm25 inside a subquery with a window function outside it, which bundled SQLite accepts. `SearchShard`'s first hosted run should run that query once.

The Worker was deleted after the run, and the API token it used was removed. Cost: under $0.01 of requests and storage.
