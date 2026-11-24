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

## Hosted vault row vh1 (2026-11-23): pass. The runner account stands in for the vault account

**The vault account on this row is the runner account, labelled as a stand-in.** Its token is not one of graft-m3f's
tokens, which is the property under test; it shares graft-runners' account but not its script, bindings, or secrets.
The vault build and the signed release came from main `7db2a284`, whose `workers/vault` is `f2d67ea2`'s. Release
`b32c4e31…` (`release.json`, `release.sig` by `~/.graft-m3f/release.key`, which is graft-m3f's `Keys.release`; `vault/`
is `wrangler deploy --dry-run`'s `entry.js` and wasm). graft-m3f ran what the m3f thread's day m3fa2 left (sites a
`8e3084cf`, b `b10d26f5`, c `ba8635f4`, edge `67c9c2f5`, `--export-bucket graft-m3f-export`); this row deployed
nothing to it. Driver `tests/m5/vaulthosted/row.sh`, logs `raw/vault/vh1/`. Start 04:43:59Z, done line 05:13:09Z (no
shard running, every vault-side resource gone).

| Check | Result |
|---|---|
| `vault init` on the real API | **Pass.** Bucket created in ENAM, the four 35-day rules (`vault-{feed,entries,snapshot,pack}-35d`, 3,024,000 s) read back, the script with `VAULT`, `GRAFTYARD_URL` and `EXPORT_TOKEN` (`secret_text`), cron `* * * * *`. Export token jti `01M83BFM4ENPP12CXC3BRTX36B`, scope install, holder `vault`, 1 day, held only as the script's secret |
| Feed copied byte for byte | **Pass.** At 05:07Z the vault held 461 `feed/` objects, every one of the export bucket's 461 `export/` objects, each with the same etag; 3,196 chain cursors, nothing owed, `undecoded=0`, no key outside the five prefixes (`raw/vault/vh1/check-3.log`) |
| A stored pack and a snapshot under the lock | **Pass.** The seed `repo:65611` (an inline and a 3 MiB stored push, 04:57Z) reached the vault by 04:58:27Z: two packs (3,146,982 and 3,149,444 bytes, each through a multipart upload checked against its entry) and both snapshot objects |
| The 409 half under the real lock | **Pass.** Over a held `feed/` key: a create-only put and a plain put each answered 409, code 10069 "The object is locked by the bucket policy."; deletes of it and of a held pack answered HTTP 200 **with** error 10069 and deleted nothing; the object read back unchanged. `state/` took a put and a delete (`raw/vault/vh1/locks.log`) |
| A pass rerun from an older `state/vault` | **Pass.** `state/vault` set back to the 04:56Z one (position `29923495.1`) at 04:59:43Z. The next invocation's pass re-read 9 objects, found 3 already held (`existed=3`) and failed nothing; no locked object changed or went, and the 4 new objects were all past the newer position. No `holds another object` and no `vault pass failed` in any log (`raw/vault/vh1/rerun.log`) |
| `vault upgrade` | **Pass.** The same release: "already serves 100%", nothing uploaded. One byte appended to `entry.js`: refused before any API call (`BadRecord`). A second release signed by the same key (`9d7b46d5…`): uploaded as `7d6c7685…`, deployed at 100%, with the three bindings, the secret and the cron kept; that version's first passes moved the position from `29923501.2` to `29923504.0` |
| Teardown in the operator's order | **Pass.** Lock rules PUT empty, GET `rules: []`; emptied; bucket deleted; `EXPORT_TOKEN` deleted; script deleted. Lists at 05:12:01Z: buckets `[]`, scripts `["graft-runners"]` only, lock GET 404 (no bucket), the script's secrets 404 (no script) (`raw/vault/vh1/teardown.log`, `lists.log`) |
| Cost and time | graft-m3f $0.08 from 04:43Z to 05:13Z (`tests/fanout/m2bcost.py`). Runner side: 20 Worker invocations and about 1,500 R2 operations (711 puts, 468 deletes, 197 heads, 85 gets, 21 multipart calls) on a few MB, inside the plan's included use [inference: $0]. 29 minutes, under the $3 and 2 h stop |

**The first pass and the swap.** The cron started 8 minutes after `vault init` (Cron Triggers take up to 15 minutes
to propagate, https://developers.cloudflare.com/workers/configuration/cron-triggers/). Its first invocation's first
pass took 178 s for 444 objects and 3,194 chains, so the next two invocations started under it, each stored the same
first 100 objects, and lost the compare-and-swap on `state/vault` (`lost`), as built. After that every pass ran in
about 11 s.

**Missed copies.** The first pass counted 6 copies missed (404 or 410) [inference: packs of `repo:65609` and
`repo:65610`, which were purged on 2026-11-22 while the feed still names them]. They are alarmed, as built, and not
retried.

**Lag.** Over the 18 passes that stored something, `lag_ms` was a median 29.9 s, p90 68 s, max 69 s. `lag_ms` counts
from the feed object's minute, so it runs up to 60 s high; DESIGN's "about 30 s" is not contradicted, and R39's
constants stay [guess] until a row times them against feed rounds.

**Not on this row.** The export `jti`'s revocation against the vault: the token died with the script. BF-49
(m3fx49c) measured revocation with `pull.py` against the same edge.

**Finding.** Under a lock, the R2 REST API answers a refused DELETE with HTTP 200 and error 10069, so a client that
reads only the status takes it as deleted. The vault deletes only through its binding, and only `RESTORE_WINDOW` (35
days) after the install's GC, when the lock no longer holds the object; `r2.py` reads the errors.

## BF-47's local half: the vault step of `release roll` (2026-11-23, code only, $0)

Operator ruling: `vault upgrade` stays outside the `Release` group's intents and is a step of `release roll` on the
operator's machine (`decisions/DECISIONS.md`, "`vault-roll`"; DESIGN, "Install operations", Release roll, step 4).

- `graftyard_release::ops::vault_due` (the order through `McpRoll` is now `next_code`, which `next_roll` and
  `vault_due` share; `next_roll` is otherwise unchanged): due once R lists a `vault/` module and no code intent is
  owed for R. A `ContainerDay` does not hold it back.
- `graftyard release roll R --vault-account A --vault-release DIR --release-key K [--vault-script S]`: when the step
  is due, it reads the release the vault script serves under `CLOUDFLARE_VAULT_API_TOKEN` and runs `vault upgrade`
  when that is not R, printing `{"vault": "upgraded"}` or `{"vault": "at"}`; with no vault account or no token it
  prints `{"vault": "owed", "why": ...}` and goes on. It refuses, before any API call, a vault token equal to
  `CLOUDFLARE_API_TOKEN` (BF-47's fail line) and a vault account equal to install.toml's, and refuses a folder whose
  `release.json` is not R's.
- Tests: `crates/release` `bf47_the_vault_step_follows_the_code_intents_and_does_not_wait_for_container_days` (four
  site days, `EdgeRoll`, `OpsRoll`, the vault step, then four `ContainerDay`s; no step for a release without `vault/`;
  not due while a site day is owed). `cli/src/release.rs`
  `release_roll_upgrades_the_vault_under_its_own_token_once_the_code_intents_are_done` (not due while `FrontRoll` is
  owed; owed with no account or token; `release roll` stops on the install's token; the install's account refused; an
  upgrade that keeps the bindings with every request under the vault's bearer token, then "at"; another release's
  folder refused).
- `tests/m0carried/plant93.py`: 9 mutants, 9 caught.
- BF-47's hosted row (a full `release roll` with `SiteDay { Workflows }` and the vault step) is not cleared.

## BF-47 hosted: plan (2026-11-23, not run; waits for the lead's go)

Gate line (`build/BUILD-PLAN.md`, M5-WP5): `release roll` end to end, including `SiteDay { Workflows }` and
`graftyard vault upgrade`. Pass: the order is as in DESIGN's Release roll, and the vault is upgraded with no install
token. Fail: any other order, or an install token used.

**The vault account is the runner account, standing in** (`FRAME.md`, "Operator decision, 2026-11-23, the runner
account stands in as BF-47's vault account"). It also holds `graft-runners`, so the vault is not isolated from Actions
runners the way a production vault account is. The verdict line says so.

**State read at 13:10Z, read-only, $0.**
- graft-m4a's `Release` group holds 193 entries, nothing current, nothing drained, and no slot moved.
- `site_release`: a and b on R2 `a3a1f620`, c on R6 `761a720d`. `rolled`: Edge on e4 `63644eb6`. No Ops, Front, Mcp
  or Workflows release is recorded.
- The install has no Workflows script and no export bucket. The runner account holds only `graft-runners`, with no
  bucket.

**Rows, each with a $5 stop** on graft-m4a's day plus the runner side, counted with `tests/fanout/acctcost.py`
(`PREFIX=graft-m4a`). They run after BF-46 has restored graft-ops and before anything else on graft-m4a.

1. **Setup (row 47a).** Sets up four things:
   - **Export bucket:** `graft-m4a-export`.
   - **Workflows script:** `graft-m4a-workflows`. DESIGN has no `workers/workflows` yet, so it is the site build
     deployed under that name with no member classes. It is a stand-in that `SiteDay { Workflows }` deploys and gates,
     and the results label it as one.
   - **Vault:** `graftyard vault init` into the runner account, with `CLOUDFLARE_VAULT_API_TOKEN` minted for that row
     and scoped to Workers Scripts Write and R2 Storage Write on that account only, never the runner's own token. The
     vault Worker and bucket are `graft-m4a-vault-standin`, with `--days 1` for the lock.
   - **Edge:** an edge day that binds `EXPORT`.
2. **The roll (row 47b).**
   - R8 and R9 bundles, signed by m4a's release key, each holding `site/`, `workflows/`, `edge/`, `ops/` and `vault/`
     files. R9 differs from R8 by a marker in each folder, so nothing is skipped as unchanged.
   - `graftyard release roll R9 --vault-account <runner> --vault-release R9/ --release-key release.pub` runs after
     each `IntentDone`, under the customer key, with `CLOUDFLARE_API_TOKEN` set to the install's token.
   - graft-ops walks each intent at 60 s dwells with `GATE=pass`. Expected order: SiteDay a, b, c, then SiteDay
     Workflows (no drain), then EdgeRoll, then OpsRoll, then the vault step. There are no Front or Mcp scripts on
     graft-m4a, so those two are skipped as not in R9's versions.
   - The vault step prints `{"vault": "upgraded"}` once, then `{"vault": "at"}` on the next run.
3. **Checks.**
   - The order, read from the group's entries.
   - The vault script's deployed `workers/tag` is R9.
   - Which token made each write on the vault account, from that account's audit log (`GET /logs/audit`, whose
     `actor` names the API token's id): every upgrade write must name the vault token's id and none the install
     token's. The install token cannot reach the runner account at all; it answered 403 (code 10000) on a read at
     13:15Z.
   - A negative probe: with `CLOUDFLARE_VAULT_API_TOKEN` set to the install's token, `release roll` stops before any
     API call. That is the fail line, refused.
4. **Teardown.**
   - The vault: lock rules emptied, bucket emptied and deleted, the script and its secret deleted, and the vault token
     revoked.
   - The Workflows stand-in script and the export bucket are deleted.
   - Sites, edge and graft-ops are left on R9's build, which is main's.
   - Then lists of buckets, scripts and tokens on both accounts.

**Cost estimate.** BF-7's four site days, edge roll and ops roll cost $0.66 on graft-m4a (`tests/m5/results/m5-wp3.md`).
This row adds one more site day, a vault running about 2 hours, and the export feed. The estimate is under $2 for
47b and under $1 for 47a [inference].

**What only this row shows.** A full `release roll` against live scripts, with `SiteDay { Workflows }` among them, and
the vault step running from `release roll` itself (not `vault upgrade` by hand, as vh1 ran it), under a token that is
not the install's.

## Left

1. **Hosted:** done on a stand-in vault account (vh1, above). A customer-owned vault account, and the export `jti`
   revoked against the vault, remain.
2. BF-47 passed hosted on stand-ins (2026-11-24, "BF-47 hosted" below). A real vault account and a real Workflows
   script remain for its rerun.
3. **Per-org pull caps** (R39 (a)): closed 2026-11-24 by copies in turns, below; no cap on the export routes.

## BF-47 hosted (2026-11-24, graft-m4a): pass, on two stand-ins

Gate line (`build/BUILD-PLAN.md`, M5-WP5): `release roll` end to end, including `SiteDay { Workflows }` and
`graftyard vault upgrade`. Pass: the order is as in DESIGN's Release roll, and the vault is upgraded with no install
token. Fail: any other order, or an install token used.

**Result: pass (done line 01:50:19Z, the roll's `{"roll": "done"}`; torn down by 02:00Z).** One `graftyard release
roll R9`, rerun after each `IntentDone` by the driver's loop, signed every intent with the customer key. graft-ops
executed every step, and the vault step ran from `release roll` itself under the vault's own token.

**Stand-ins, as ruled.**
- **The vault account is the runner account** (`FRAME.md`, "Operator decision, 2026-11-23, the runner account stands
  in as BF-47's vault account"). It also holds `graft-runners`, so this vault is not isolated from Actions runners as a
  production vault account is.
- **`graft-m4a-workflows` is a minimal Worker, not Workflows**: a fetch handler that answers its version metadata, with
  no cron, queue, Durable Object, container or bucket. It tests the roll's order, not Workflows. R8's and R9's
  `workflows/` hold that script and its marker.
- **No export edge day, and no export bucket.** The plan's step 1 edge day that binds `EXPORT` was dropped, with the
  lead's approval before 00:05Z. With no export feed behind it, the vault's pulls failed for the whole row
  (`vault pass failed: batches: 404` ×34 and `503` ×15 in its logs), which the lead ruled outside BF-47's lines.

**Builds.** Site, edge and ops from main `05aace32` (test-faults), the vault and the CLI from the same tree. Both
edgeboot builds passed on it with 0 traps. Bundles, each signed by m4a's release key:
- R8 `47a4ac5c…`: `formats.txt` (R6's 27 formats, including 65000.1), `site/`, `workflows/`, `edge/`, `ops/`, `vault/`.
- R9 `e23f69e9…`: R8 plus a marker in each folder, so nothing is skipped as unchanged.

**Row 47a, setup (00:58:26Z to 00:59Z, $0.00).**
- The vault token `graft-m4a-bf47-vault` (`52570767…`): Workers Scripts Write and Workers R2 Storage Write, on the
  runner account only. The runner's own token minted it, and it is never the runner's or the install's.
- `graft-m4a-workflows` was deployed with R8's `workflows/` (`ebb2f6e4`).
- `vault init` under the vault token: bucket `graft-m4a-vault-standin` in ENAM, the four 35-day lock rules, the
  script at R8 (`bac66742`, `workers/tag` `47a4ac5c…`), `EXPORT_TOKEN` and the pull cron.

**Row 47b, the roll (00:59:51Z to 01:50:19Z, $0.05).**
- graft-m4a-ops `53485950`: the test-faults build of `05aace32` with `GATE=pass`, 60 s dwells and `OPS_SCRIPTS`
  naming the three sites, `workflows`, `edge` and `ops`. This is a test-install deploy outside the measured path. Its
  deploy token `graft-m4a-bf47-deploy` was minted and PATCHed into `graft-m4a-ops-deploy-token`.
- R9 was put into `graft-m4a-releases/r9/`.
- The deploy's Cron Trigger fired neither after the deploy nor after one re-PUT. A reset (a yearly cron, then `*/1`)
  at 01:06:31Z made it fire (`raw/bf47/m4abf47/notes.txt`).

**The order, from the group's entries** (`raw/bf47/m4abf47/checks.log`). Each `Intent` is signed `031923…` (the
customer); every other entry is signed by the executor key `d38725…`, except the drain-key `SiteMode`s, signed
`477de5…`.

| Index | Time | Entry |
| --- | --- | --- |
| 221 | 01:07:20Z | `ReleaseUploaded` R9 |
| 222 | 01:08:30Z | `SiteDay { Site(0), R9 }`, 1 undo; drain 223, `SiteRelease` 228, undrain 230, `IntentDone` 233 at 01:16:40Z |
| 234 | 01:16:43Z | `SiteDay { Site(1), R9 }`, 1 undo; `SiteRelease` 240, `IntentDone` 245 at 01:25:39Z |
| 246 | 01:25:58Z | `SiteDay { Site(2), R9 }`, 1 undo; `SiteRelease` 252, `IntentDone` 257 at 01:34:41Z |
| 258 | 01:35:12Z | `SiteDay { Workflows, R9 }`, no undo (no N recorded), no drain; `IntentDone` 263 at 01:40:43Z |
| 264 | 01:41:21Z | `EdgeRoll { R9 }`, 1 undo (`EdgePointer` to e4); `IntentDone` 269 at 01:46:37Z |
| 270 | 01:47:30Z | `OpsRoll { R9 }`, no undo (no Ops release recorded); `IntentDone` 273 at 01:49:38Z |
| — | 01:50:12Z | the vault step: `{"vault": "upgraded"}`, R9 uploaded to `graft-m4a-vault-standin` as `7612b32d` at 100% |
| — | 01:50:19Z | the next run: `{"vault": "at"}`, then `{"roll": "done"}` |

That is DESIGN's Release roll order: each member site's `SiteDay`, then `SiteDay { Workflows }`, `EdgeRoll`,
`OpsRoll`, then the vault step. `FrontRoll` and `McpRoll` were skipped because R9 names neither script, and no
`ContainerDay` was due because R9 names no image. Before the last `IntentDone` the vault step stayed silent on every
run. After the roll, `site_release` and `rolled` are R9 for all six scripts, and every script's deployed version is
tagged `e23f69e9…`: sites a `75bdd60d`, b `627ed36c`, c `4600b130`, edge `eef95142` (with e4's `f4e193b9` at 0%), ops
`c7079529`, Workflows stand-in `347df6b3`, whose fetch answered marker `bf47-r9`.

**The vault's `workers/tag`:** `47a4ac5c…` (R8, `vault init`) before; `e23f69e9…` (R9, message `graftyard vault
upgrade`) after.

**Which token wrote on the vault account** (its audit log, `GET /logs/audit`, every non-read from setup to the checks):

| Time | Action | Token |
| --- | --- | --- |
| 00:58:53Z | Create Token (the vault token) | `6ca3ce49…`, the runner's own token (the operator's mint, not the roll) |
| 00:58:53Z | `TOKEN_CREATE` (its account-settings record) | none named |
| 00:58:58Z | Create Bucket | `52570767…`, the vault token |
| 00:58:58Z | Set Bucket Lock Rules | `52570767…` |
| 00:58:59Z | Upload Worker | `52570767…` |
| 00:59:00Z | Update Worker Script Schedules | `52570767…` |
| 01:50:36Z | Upload Version (the roll's vault step) | `52570767…` |
| 01:50:37Z | Create Worker Deployment (the roll's vault step) | `52570767…` |

Every write by `vault init` and by `release roll`'s vault step names the vault token. None names the install token
(`cd2328dc…`), which cannot reach the runner account at all (403 on a read there at the plan's check, 2026-11-23
13:15Z). The audit log's times are
the API's; the orb's clock read the upgrade at 01:50:12Z.

**The negative probe (the fail line), 01:57:54Z** (`raw/bf47/m4abf47/probe.log`). `release roll R9`, with the API and
the edge pointed at a local recorder that answers every request 599:
- `CLOUDFLARE_VAULT_API_TOKEN` set to the install's token: refused, "CLOUDFLARE_VAULT_API_TOKEN is the install's
  token: the vault upgrades only under its own", exit 1, **0 requests**.
- `--vault-account` set to the install's own account, under the vault token: refused, "--vault-account is the
  install's account …: the vault lives in its own account", exit 1, **0 requests**.

**Teardown (01:59:20Z to 01:59:57Z, `teardown.log`, `lists.log`).**
- graft-ops parked on R9's build: `8734edc8`, the test-faults build of `05aace32` with `exec-ck-pending.json`
  (`GATE=pending`, `OPS_STEP_MIN_ENTRIES` 50, 60 s dwells, no slots, `OPS_SCRIPTS` the three sites), schedules `[]`.
  Its deploy token was revoked.
- The vault, in the operator's order: lock rules PUT empty, GET `rules: []`, bucket emptied (0 objects: the pulls
  stored nothing), bucket deleted, `EXPORT_TOKEN` deleted, script deleted, vault token revoked.
- `graft-m4a-workflows` deleted; token files shredded.
- The parked graft-ops kept running Idle passes after its schedules went to `[]` (the last seen at 02:07:35Z), the same
  tail M4-WP4 saw. A reset at 02:08Z (a yearly cron, then `[]`) left GET `schedules: []`.
- Lists. Runner account: buckets `[]`, scripts `["graft-runners"]` only, tokens only the runner's own, the vault
  bucket's lock answering 404 (no bucket). Install account: no `graft-m4a-workflows`, no `graft-m4a-bf47` token,
  graft-ops schedules `[]`. The group holds 274 entries, nothing current, every slot 0.
- Left on R9, as ruled: sites a, b and c, the edge, and graft-ops. Their `RELEASES` bucket keeps `r9/`.

**Spend.** 47a $0.00, 47b $0.05 (graft-m4a, `tests/fanout/acctcost.py`, from 00:58:26Z). Runner side: about 60 vault
invocations and a few R2 and API calls, inside the plan's included use [inference: $0].

**Verdict: pass.** The order is DESIGN's Release roll, and the vault was upgraded by `release roll` under the vault's
own token with no install token, which the probe refused before any request. Both stand-ins hold: the vault account is
the runner account, not isolated from `graft-runners`, and `graft-m4a-workflows` is a minimal Worker that tests the
order, not Workflows. The export edge day was dropped by ruling, so the vault's pulls failed throughout, which is
outside BF-47's lines.

## R39 (a): owed copies in turns (2026-11-24, code only, $0)

DESIGN's vault noisy-tenant row said "Pull bandwidth caps per org", and nothing built it. Worse, on origin/main
`2011060d` one network's backlog held back every other: `Copy`'s derived order put every `Pack`, by network, ahead of
every `Snapshot`, and a pass copied the first `COPIES_PER_PASS` (40). A copy held back past `CONSUMED_KEEP` or a purge
is lost, counted missed, and not retried.

- **Built** (`workers/vault/src/lib.rs`, `in_turns`, `Turn`, `VaultState.copy_turn`): a pass takes one owed copy per
  turn, round and round, up to 40, starting with the turn after the one the last pass tried. A pack's turn is its
  network; a snapshot's is its chain, since the state holds no chain-to-network map. `copy_turn` is serde-defaulted, so
  an older `state/vault` reads. A 5xx keeps the copy owed and a 401 stops the pass, as before.
- **Test** (`workers/vault/tests/vault.rs`,
  `one_network_s_backlog_waits_its_turn_and_the_next_pass_starts_after_the_last`): network 1 owes 100 packs, networks
  2 to 60 one each. Pass 1 copies networks 1 to 40 once each and records turn 40; pass 2 starts at 41, copies 41 to 60,
  then 20 more of network 1. Every network from 2 to 60 is held after 2 passes; the rest of network 1 drains after.
  Run against origin/main's `lib.rs` (with the new field stubbed), pass 1 copied network 1 forty times and the test
  failed; in key order networks 2 to 60 need 4 passes.
- **Ruling:** no per-org cap on the export routes; DESIGN's row and `vault-turns` record it as not built, and its flip
  names a hosted row that breaks the ⌈networks/40⌉ bound, or a managed tenant whose pulls load the front door.
