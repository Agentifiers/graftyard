# M5-WP5: the vault Worker and `graftyard vault init`, the pure half (2026-11-23)

**Code only, $0. Nothing is deployed, and no vault account exists.** This builds the vault half of M5-WP5's Build
line: `graftyard vault init` and the vault Worker, which pulls the feed, entries, snapshots, and packs under an
install-scoped export token into R2 under 35-day bucket locks, deletes a pack no sooner than `RESTORE_WINDOW` after the
install's GC, and keeps every `ChainId`. The hosted BF-49 row with the vault, BF-47, and `graftyard vault upgrade` are
not here. The design changes are DECISIONS "`vault-worker`"; the gaps are OPEN-QUESTIONS R39.

## What was built

- **`workers/vault` (`graftyard-vault`).** `pass<H: Host>` is plain Rust, tested natively; `src/worker.rs` is the
  Workers host.
  - **Layout.** `feed/<minute>/<seq>` (each `ExportObject` as served), `entries/<chain>/<from>-<to>` (entries the
    feed never carried), `snapshot/` and `pack/<network>/` under their install keys, all put create-only, and the one
    rewritten object `state/vault`, written compare-and-swap on its etag.
  - **Feed.** Pages `batches` after its position, runs `export::check_object` against its own cursors, stores, then
    moves them. A 410 on `batches` relists from the oldest object and fills the gap from the entries endpoint.
  - **Backfill.** A chain first carried past index 1 is filled from `chains/{c}/entries` from 1, or from the newest
    snapshot below its first carry once that range is gone (410).
  - **Copies.** Repository-family rows decode: every stored pack they name (`packs_of`, `Promoted`,
    `Compacted.produced`) streams into a multipart upload of 8 MiB parts, completed only if its length and SHA-256
    match the entry's `PackRef`; every snapshot an `Archived` names is copied object by object, each checked against
    its frame's SHA-256 (`SnapshotReader`). A 404, 410 or bad bytes count `missed` and alarm.
  - **GC.** Candidates are `Compacted.consumed` after `CONSUMED_KEEP` and a deleted repository's packs after
    `retain_until`. Each is probed at most daily; a pack is deleted only if the install still answers 404
    `RESTORE_WINDOW` after the first 404, and a 200, 410 or 5xx in between starts the clock over. A 410 is never a GC.
  - **Worker.** A `* * * * *` cron runs passes every 10 s for 45 s. A 401 stops the invocation; a 5xx is retried.
- **`graftyard vault init`** (`cli/src/vault.rs`). Under `CLOUDFLARE_VAULT_API_TOKEN`: refuses a key other than
  install.toml's customer key before any API call; mints and verifies an install-scoped token with holder `vault`;
  creates the bucket if missing; puts `vault-{feed,entries,snapshot,pack}-35d` (Age 3,024,000 s), keeping other rules
  and never locking `state/`; deploys the bundle with `VAULT`, `GRAFTYARD_URL` and the secret `EXPORT_TOKEN`; sets the
  cron. It prints the jti, scope, holder and exp, never the token.

## The bed: `tests/vaultbed/run.sh`, local workerd, $0

Four builds in one `wrangler dev`: exportbed's three site scripts (`workers/site`, test-faults, with the test install's
genesis), the edge's own build (test-faults, this install's anchor, Tokens 17.2 enabled), the vault's own build, and
`front.mjs` standing in for the internet between the accounts. The token comes from `graftyard vault token --scope
install` and reaches the vault only as its `.dev.vars` secret. Before the first round the bed plants one object of
`repo:900500` whose two entries name stored packs, a 9 MiB one whose bytes match and one whose bytes do not, with their
`Committed` markers in site 0's bucket.

```
drive: {"joins":[200,200,200],"role":"leader","put":200,"commit":3}
vault cron 1: {"ms":41294,"outcome":null}
pass1: ok   every export/ object (3) is under feed/, byte for byte (3)
pass1: ok   the good pack is stored with the entry's SHA-256, through two upload parts
pass1: ok   the bad pack is never stored
pass1: ok   one copy counted missed (got 1)
pass1: ok   cursors {'repo:900001': 3, 'repo:900500': 2, 'tokens:1788': 2}
pass1: ok   every key is under a locked prefix or state/
vault cron 2: {"ms":40167,"outcome":null}
pass2: ok   every export/ object (4) is under feed/, byte for byte (4)
...
revoke: 204
vault cron 3: {"ms":24,"outcome":null}
revoked: ok   nothing stored and the state unmoved, with a new object waiting (True)
wasm traps: 0
PASS
```

- The feed's own round carried `repo:900001` (led, reported) and the `tokens` shard the edge's `jti` read created;
  the vault's cursors match each chain's commit.
- Pass 1's line: `objects=2 packs=1 missed=1 lag_ms=42508`. The later passes in each 45 s invocation found nothing new.
- After `DELETE /api/graftyard/v1/export/token` (204) and 6 s past READ_CACHE, the third invocation stopped at its
  first request with `vault pass failed: batches: 401` in 21 to 24 ms (two runs), and the vault's keys and state were unchanged while a
  new object waited in the feed.
- Getting there: the edge answered 503 until the site scripts had `GRAFTYARD_GENESIS` (the `jti` read needs a Tokens
  shard), and `DELETE export/token` answered 503 until `FORMATS_ENABLED` held 17.2, as every hosted edge's does.

## Tests and plants

- `cargo test -p graftyard-vault`: 4 unit tests and 7 integration tests (`workers/vault/tests/vault.rs`, against
  `export::serve` and a bucket that refuses overwrites and deletes under lock): every entry once with `Release` and
  `Ids`, a raced pass, a corrupt and a lost pack, a promoted pack, backfills by entries and by snapshot; consumed-pack
  GC timing; a pack served again after its GC; a feed object that skips an index refused; a vault past the feed's
  retention resyncing; a 401 on a copy keeping it owed; a revoked token moving nothing.
- `cargo test -p graftyard -- vault`: `vault init` on a fresh account, a rerun that keeps foreign lock rules, a wrong
  key refused before any call, and the token command.
- `tests/m0carried/plant88.py`: 23 mutants on `workers/vault/src/lib.rs` and `cli/src/vault.rs`. The first run missed
  six; tests were added for them, and one was a bug: a delete ran on the stored first 404 without asking the install
  again, so a pack restored after its GC would still leave the vault. The pass now probes before every delete.

## Design and cost

- DESIGN, Install operations, Vault: the locked prefixes, create-only puts, the checked copies, the unlocked
  compare-and-swap cursor, backfill, and GC counted from the first 404 (no 410). Failure modes, `vault` bad-config: the
  vault alarms, since the install cannot read the vault's cursors. DESIGN stays at 2,200 lines; both tables already had
  every vault row.
- `scripts/cost.py`: the vault's multipart Class A per pack, a state write per feed object, a Class B HEAD per pack and
  a state read per pass, and one `batches` request per pass (five a minute) instead of one per feed object. The vault
  line moves $157 → $160 at 2,000 developers, the base $9,332 → $9,335 (separate $9,229 → $9,232), the agent layer
  $2,935 → $2,939 and $12,266 → $12,274; 50 and 300 developers are unchanged. Copied to DESIGN, AGENT-LAYER, AWS.md;
  `costs_report.py` rerun. OPEN-QUESTIONS K5 cites the 50-developer base, which did not move.

## R39 (b) and the lock's 409 (2026-11-23, code only, $0)

- **Signed vault artifact** (`crates/release/src/ops.rs`, `check_vault`). A release's `vault/` folder is taken only
  when `release.json` verifies under `Keys.release`, lists every `vault/` file given with its SHA-256, every listed
  file is given, and `vault/entry.js` exists. `graftyard vault init --release D --release-key K` verifies before any
  API call; `graftyard vault upgrade` verifies first, refuses a script with no deployment, leaves a release already at
  100%, and otherwise uploads with `keep_bindings` (`r2_bucket`, `plain_text`, `secret_text`) and deploys at 100%. The
  release id is each version's `workers/tag`.
- **Lock before precondition.** Cloudflare's bucket-locks page (Notes) says a lock rule is checked before
  `If-None-Match`, so a repeated create-only put returns 409 `ObjectLockedByBucketPolicy`, and the old `put_new`
  would have failed the pass. Every locked object now stores its SHA-256 as metadata; any refused put is judged by a
  HEAD (same size and digest: stored, move on; another object or nothing: fail the pass). A pack is HEADed before its
  copy against its entry's length and checksum; a snapshot object by its size.
- **Tests.** `cargo test -p graftyard-vault`: 4 unit and 9 integration tests. The test bucket now answers a held key
  with 409 before the precondition, as R2 does: a pass rerun from an older `state/vault` moves on over every locked
  object and stores nothing new; another object at a feed key fails the pass and stays; another pack at a pack key is
  counted missed and stays. `cargo test -p graftyard -- vault`: 7 tests, including six tampered or unsigned releases
  refused by `init` and `upgrade` before any API call, and `upgrade` keeping the bindings. `crates/release`:
  `the_vault_worker_comes_only_from_a_verified_release`.
- **plant88**: 33 mutants (10 new: four on the refused-put judgment, three on `check_vault`, three on `upgrade`). The
  first run caught 31: a size mismatch with no digest known passed as the same object (a test now checks the snapshot
  case), and one mutant did not compile; both are caught now.
- **Bed.** `tests/vaultbed/run.sh` adds a rerun after pass 2: `state/vault` dropped, one more invocation, and the
  check that no object changed, the feed state equals pass 2's, every object carries its own SHA-256, and no pass
  failed. Local R2 has no locks, so the bed covers the 412 half; the 409 half is the hosted row's. Run from scratch
  (2026-11-23): PASS, 0 wasm traps. The rerun pass re-read all four feed objects and the pack and stored nothing
  (`objects=4 existed=1 missed=1`, the bad pack still missed), every object carried its own SHA-256, and no pass
  failed:

  ```
  vault cron rerun: {"ms":40159,"outcome":null}
  rerun: ok   a pass from no state stored nothing new (5 objects) and every object carries its sha256 (True)
  revoke: 204
  vault cron 3: {"ms":35,"outcome":null}
  revoked: ok   nothing stored and the state unmoved, with a new object waiting (True)
  ```
- **Hosted row, when cleared** (runner account as a labelled stand-in, stop at $3 or 2 hours): after BF-49's passes,
  put back an older `state/vault` and run one pass; expect no failure and nothing new stored. Teardown: PUT the lock
  config with no rules, GET to confirm none, empty the bucket, delete it, delete the script and its secret, then list
  to confirm each is gone.

## Left

1. **Hosted:** `graftyard vault init` into a vault account, then BF-49's row with the vault pulling a real install
   and the export `jti` revoked. Not cleared.
2. BF-47 (`release roll` running `vault upgrade` hosted); `vault upgrade` and the verified `vault init` are built.
3. **Per-org pull caps** (R39 (a)).
