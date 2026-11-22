# M5-WP5: the change feed and the export endpoints, the pure half (2026-11-22)

**Code only, $0.** This covers the change feed's rules, the four export endpoints' request-to-reply logic, and export
tokens with their revocation, all as pure code with tests. BF-49's pure half passes in
`crates/git/tests/export.rs`. Nothing is hosted, deployed, or wired into a Worker yet. BF-47 and `graftyard vault
upgrade` wait on M5-WP4 and are not here.

## What was built

- **`crates/git/src/export.rs`.**
  - The feed: `ChangeBatch`, `ExportObject`, `ExportCursor`, `FeedState`, and `plan`, which picks the next batch from
    the leaders' commit indexes, at most 2,000 chains, the furthest behind first.
  - `FeedState::advance` and `recover`. The consumer writes the object, then the state, and a round first folds in
    objects stored after the state, so a crash between the two writes repeats and skips nothing.
  - `check_object`, run before each write: each chain holds exactly its entries after its cursor.
  - Frames (`frame` and `unframe`: a 4-byte length, the bytes, and their SHA-256), and `encode_object` and
    `decode_object`.
  - `Route::parse` for the four `GET` routes and `DELETE export/token`, with 422, 404 and 405.
  - `serve` over an `ExportSource`: batch pages with `Link rel="next"`, entries pages (1,000 entries or 8 MiB, never
    past the member's commit index, 410 below its first stored index with no archive, a gap never served), the
    newest snapshot at or below `at` (core first), and packs with `X-Graftyard-Checksum`. A chain or pack outside the
    scope is 404, like a chain with no group, and a batches cursor past the bucket's lifecycle is 410.
- **`crates/git/src/key.rs`.** `R2Key::Export { minute, seq }` as `export/<minute:012>/<seq:010>`, which sorts in feed
  order, and `R2Key::ExportState`.
- **`crates/identity/src/export_token.rs`.**
  - `ExportClaims`; `sign` (what `graftyard vault token` runs); `verify` against the install and customer key: the
    signature, the install, the 400-day bound, `CLOCK_SKEW`, and `exp`.
  - `covers`: `Install` covers every `ChainId`, `Release` included, where `graftyard_release::view::covers` alone says
    no; a narrower scope covers what a `StaleMode` flag does.
  - `revoke_command`: `RevokeJti` on `jti_shard(jti)` under the token's own `CredentialKey::Jti`. No wire enum changed.
- **Design.**
  - DESIGN, Shape, "Lifeboat feed": `ExportState`, the batch cap, `FeedState` and the write order, frames, the 404 rule,
    the token's form, and `Install` covering `Release`.
  - The export bucket is folded into the `event pipeline` rows of "Failure modes" (all 8 classes) and "Deployment
    modes", which already named the lifeboat feed, not added as a new component.
  - DECISIONS "`export-feed`".
- **Cost.** `scripts/cost.py` counted the vault's copy of each feed object, but not the install's own writes. It now
  adds two Class A writes per round to the install's R2: the object and the state, 0.52M a month. The base moves
  $236 → $238, $1,013 → $1,016 and $9,323 → $9,325 a month; with agents, $259, $1,344 and $12,256. The figures are
  copied into DESIGN Cost, AGENT-LAYER, `alternatives/AWS.md`, OPEN-QUESTIONS K5 and K12, and `research/costs.md`
  (`costs_report.py`).

## BF-49, pure half

`bf49_vault_pulls_every_endpoint_then_the_revoked_jti_is_refused_within_read_cache`:

- 231 feed rounds of 10 s over `repo:1`, `repo:2`, `release`, `tokens:9` and `ids`, plus a 2,345-entry `repo:3`.
  Every 17th round loses its state write, and the next round recovers.
- The vault pulls every batch page by `Link` (3 or more pages). Each chain's entries arrive once, in order, equal to
  the chain's log, `Release` and the system chains included.
- It pages `repo:3`'s entries (3 pages), takes the snapshot at or below 149 (the one at 50), and a pack with its
  checksum.
- An org-scoped token from the same customer key reads its own repository and gets 404 for another owner's, for
  `release`, for `tokens:9`, and for a pack of another owner's network.
- Then `RevokeJti` commits under the token itself. The edge model reads the `jti` row through `IdentityCache` as a
  read-class request, which is the `READ_CACHE` rule. It is probed every 250 ms from the revocation to READ_CACHE +
  2 s:
  - It serves no request more than READ_CACHE after its cached read, and none more than READ_CACHE after the
    revocation.
  - It refuses at READ_CACHE + 1 ms, + 60 s and + 1 day.
  - A cold isolate refuses at once.

`a_batches_cursor_past_retention_is_410_and_the_newest_is_empty` covers the 410 line and the empty last page.

## Checks

- `tests/m0carried/plant83.py`: 29 of 29 caught.
  - The first run caught 21. The 8 it missed showed two gaps:
    - The plant's test filter skipped the integration test.
    - Four rules had no test that failed on them: a cursor moving back, `check_object` accepting a late start of the
      right length, a stray frame after the last chain, and a revoke sent again under the revoked token.
  - Those tests were added, and the 8 rerun alone all print CAUGHT.
- The full checks: fmt; workspace clippy; wasm32 clippy for site and edge, with and without `test-faults`, and for
  ops, front and runners; `cargo test --workspace`; `go test` in `containers/git/server` and `tests/m0/porcupine`;
  the three `scripts/check_*.py`; `node --test tests/am1/rest/h1.test.mjs`. Results are in the commit message.

## Left for the hosted half

1. **The export bucket.** Created by Terraform in site A's region with the 7-day lifecycle rule, and bound to site A's
   script. This is a binding change, so it ships only through a site day (AGENTS.md).
2. **The site consumer.** A queue or cron round on site A that reads the leaders' commit indexes, reads entries from a
   follower (`/entries?local=1` gives JSON now; the object needs the stored row bytes), and runs `plan`,
   `check_object`, `encode_object`, then writes the object and `FeedState`.
3. **The edge routes** under `/api/graftyard/v1/export`.
   - Token verification against the cached `Keys`' customer key, and the `jti` read through `rcache::JTIS` as
     `ident::stateless` does.
   - An `ExportSource` backed by follower or learner reads and the export and site buckets, never the leader.
   - `DELETE export/token`, committed through `commit_system`.
4. **The vault Worker and `graftyard vault init` / `vault token`.** These are not built. BF-49's hosted row needs a
   puller; a script that pulls all four endpoints like the vault would do for the row, before the vault Worker exists.
5. **The hosted BF-49 row.** Run from merged main on one install under the standing queue ($10 per row, $40 per install
   per day): pull every endpoint, revoke, and probe until READ_CACHE + a margin.
