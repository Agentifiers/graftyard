# M5-WP5: the change feed and the export endpoints, the hosted half's code (2026-11-22)

**Code only, $0. Nothing is deployed.** This builds items 1 to 4 of `tests/m5/results/m5-wp5.md`, "Left for the hosted
half": the export bucket's deploy path, the site's feed consumer, the edge's four export routes, and a puller with an
export-token command for the row. The hosted BF-49 row runs only from merged main, on graft-m3f under the standing
queue. BF-47 and `graftyard vault upgrade` still wait on M5-WP4. The design changes are DECISIONS "`export-host`".

## What was built

- **Leader reports** (`workers/site/src/host.rs`, `report_commit`).
  - A leader's alarm sends its commit index and record to site A's `Registry` object `export`.
  - It reports at most once every `FEED_EVERY_MS`, only past its last report, and only on installs with
    `GRAFTYARD_EXPORT=1`.
  - The object keeps the highest index per chain (`workers/site/src/registry.rs`, `/export_report`).
- **Rounds** (`workers/site/src/export_feed.rs`).
  - The `*/15` cron kicks a round on the site that binds `EXPORT`. Each round sends the next as a
    `{"exportFeed": true}` message on site A's queue, delayed 10 s (`prsync::consume` routes it).
  - The object's `/export_round` claim (`round_due`, 9 s) collapses a kick and a message that meet into one round.
  - A round:
    1. Reads `FeedState` and folds in any objects listed after it.
    2. Runs `plan`.
    3. Reads each chain's rows six at a time through `graftyard_lookup::log_rows`.
    4. Runs `fill` and `check_object`.
    5. Puts the object with `If-None-Match: *`, then writes the state.
- **Source reads.**
  - A member's `/log_rows?from=&to=` (`Host::log_rows`) answers an encoded `LogPage`: its first stored index at or
    after `from`, its commit index, rows up to 1 MiB, and the `archive_keys` range when `from` is below its stored log.
  - A leader with any other member answers JSON `{"error": "leader"}` (`export::serves_log`).
  - The lookup asks learners, then followers, and never the cached leader. A Solo group's one voter serves.
- **Edge routes** (`workers/edge/src/export.rs`, dispatched before `purge::route`).
  - **Tokens.** An `ExportClaims` bearer token, verified against the `Release` view's customer key (the anchor's
    before any `Keys`). The `jti` is read through `ghs_`'s path and cache (`ident::jti_revoked`, `rcache::JTIS`).
  - **Scope.** `in_scope` runs before any read. An org token's repository chain or pack is checked against the
    repository's `RepoOwner`.
  - **Batches.** These list `EXPORT` after the cursor, at most 100 objects or 8 MiB, and need an `Install` token.
  - **Entries.** These come from `/log_rows`. Below the member's stored log, they come from the archive object it
    names, read as it streams (`export::ArchiveRows`).
  - **Snapshots.** These stream from the site bucket as frames, hashing as they go.
  - **Packs.** These need `?network=` and a `Committed` marker in some bucket, and stream with `X-Graftyard-Checksum`.
  - **Revocation.** `DELETE export/token` commits `RevokeJti` through `DELETE /installation/token`'s path
    (`ident::revoke_jti`).
  - Every nested future is boxed, since the edge's `serve` future already sat near wasm's 1 MiB stack (B0-34).
- **Deploy.**
  - **Sites.** `workers/site/deploy.mjs --export-bucket <name>` creates the bucket in site A's region with the
    lifecycle rule `expire7` on `export/` only, binds it as `EXPORT` on site A, and sets `GRAFTYARD_EXPORT=1` on
    every site.
  - **Edge.** `tests/m1/integ/deploy-edge.mjs --export-bucket <name>` binds it on the edge.
  - **No Terraform.** The repository has no Terraform, so the deploy script stands in for it.
- **For the row.**
  - **Token command.** `graftyard vault token --key <file> [--scope] [--holder] [--days] --out <file>` (`cli/src/vault.rs`)
    signs an export token offline and writes it at mode 0600, printing only its `jti`, scope, holder and expiry.
  - **Puller.** `tests/m5/bf49/pull.py` pulls every endpoint the way a vault would, verifies every frame, compares
    entries byte for byte with the feed, then revokes and probes every 250 ms. It passes only if no probe sent more
    than READ_CACHE after the 204 got a 200.
- **Design.** DESIGN, Shape, "Lifeboat feed":
  - the reports;
  - the self-scheduling round on site A;
  - the create-only put;
  - `/log_rows` from a learner or follower;
  - the 200-chain and 8 MiB caps;
  - `Install`-only batches;
  - the pack's `?network=` and its marker.

  DESIGN stays at its 2,200-line cap. The export bucket stays folded into the `event pipeline` rows, as the reviewer
  accepted for `export-feed`.
- **Cost.** `scripts/cost.py` adds the following as DO requests: per round, one claim; per moved chain, one report and
  one source read. It also adds one queue message per round. The base moves $1,016 → $1,017 and $9,325 → $9,330 a
  month, and agents $2,931 → $2,935. The figures are copied into DESIGN Cost, AGENT-LAYER and `alternatives/AWS.md`,
  and `research/costs.md` is regenerated.

## Checks

- **Unit tests.**
  - `export::` in `graftyard-git`.
  - `crates/git/tests/export.rs` 2/2, unchanged.
  - `graftyard-lookup` `export_sources_skip_the_leader`.
  - The CLI's two `vault` tests.
  - `tests/m5/bf49/test_pull.py` 8 OK.
- **`tests/m0carried/plant84.py`: 25 of 25 caught.** The mutants cover `fill`, `check_object`'s first carry,
  `report_due`, `report_newer`, `round_due`, `serves_log`, `log_page`, the page's archive field, `in_scope` for the
  feed, `batches_take`, `ArchiveRows`, the entries link, the lookup's source order, and the scope text.
- **`tests/m0carried/plant83.py`: 29 of 29.** Four anchors moved with this change, and each was planted again as the
  same mutation on the new code.
- **Full checks, 16 of 16 PASS** on the tree before its rebase onto origin/main 2d7429a2: fmt, workspace clippy, wasm32
  clippy for site and edge with and without `test-faults` and for ops, front and runners, `cargo test --workspace`,
  `go test` in `containers/git/server` and `tests/m0/porcupine`, the three `scripts/check_*.py`, and node h1. After
  the rebase, fmt, edge clippy and tests, and `check_design.py` were rerun clean.
- **edgeboot** (`tests/edgeboot/run.sh`, on the rebased edge, which has the `b034-tailfix` box): PASS on the
  test-faults build and on `PRODUCT=1`, with 0 wasm traps. The REST read, advertisement and fetch answered 503 with no
  members, as the script expects, and the test-faults build took the cold-guess path.
- **The export routes in local workerd.** This ran a copy of edgeboot with the routes as its paths, an anchor of a
  throwaway customer key, and a token from `graftyard vault token` under that key. No members were bound. All passed
  with 0 traps:
  - With no token: 401 for batches, entries and `DELETE`.
  - 422 for `after=x` and for a pack without `?network`.
  - 405 for `POST batches`.
  - With the verified token: 503 with retry-after for batches, entries, a pack and `DELETE`. The `jti` row has no
    group to read.

  The first probe answered 500 there. The edge returned the failed `jti` read as an error; it is now a 503 that says
  the revocation row did not answer, which is what a client retries.

## Left for the hosted row

1. **A site day on graft-m3f from merged main.** This change adds a binding and a variable, so it ships only through a
   site day (AGENTS.md). Steps:
   - Sites: `node workers/site/deploy.mjs ~/.graft-m3f/sites.json --test-faults ... --export-bucket graft-m3f-export`.
   - Edge: `node tests/m1/integ/deploy-edge.mjs ... --export-bucket graft-m3f-export`.
   - Each one only once `tests/edgeboot/run.sh` passes on that build.
2. **Seed and token.** Seed a few repositories with pushes, so the feed carries entries and a pack. Then run
   `graftyard vault token --config ~/.graft-m3f/install.toml --key ~/.graft-m3f/customer.key --out <0600 file>`.
3. **Pull.** Wait for two rounds, then run `python3 tests/m5/bf49/pull.py --edge <edge> --token-file <file>
   --pack <net>:<id> --revoke`. Record `report.json` here.
4. **Teardown.** Teardown and purge as graft-m3f's other rows do; the bucket goes with the install.

## Hosted BF-49 m3fx49 (graft-m3f, 2026-11-22): fail, the feed carried nothing

Site day and edge day m3fx49 from main `70c4f7ca` (`tests/m5/bf49/siteday-m3f.sh`, log
`raw/bf49/siteday-m3fx49.log`): start 10:54:42Z, site exit 0 at 11:06:28Z, edge exit 0 (DONE) at 11:08:08Z; sites
`15312ada` / `cca26544` / `dc7f578a`, edge `0b02633c`; edgeboot passed on both builds first (test-faults and product, 0
traps). `graft-m3f-export` was created in enam with `expire7` on `export/`; `EXPORT` is bound on site a and
`GRAFTYARD_EXPORT=1` is set on every site. Seed (`tests/m5/bf49/seed.mjs`, `raw/bf49/seed.log`): `m3org/bf49-mwrqiiup` =
`repo:65609`, an inline push (2 KB) and a stored push (3 MiB), head `8e984d01`, pack
`01M81F6HSG87RDP8AG8306QRY7`. Tokens from `graftyard vault token`, mode 0600: install scope (jti
`01M81F1ME0D4J1VE5AA70377ZH`) and `owner:7100001` (m3org, jti `01M81F1MHG1SEWA5WWR35ZRTEH`). Pull at 11:12:41Z
(`raw/bf49/try1/report.json`).

| Check | Result |
|---|---|
| Every chain's entries exactly once, Release included | **Not measured: the feed carried 0 entries.** `batches` answered one page with 0 objects |
| Entries read straight from non-leaders | `repo:65609` 1..10, `release` 1..657, `ids` 1..214, each contiguous; not compared with the feed |
| Pack | 200, 3,146,983 bytes, checksum equal to the body's SHA-256 |
| Org token's 404s | batches 404, `release` entries 404, `tokens:0` entries 404; its own `repo:65609` 200 |
| Org token, another owner's chain | Not measured: the control `repo:65536` is m3org's own (`m3org/j4`) and is retired (purged 2026-11-18), so it answered 503 "no learner or follower answered" |
| Revocation against READ_CACHE (10,000 ms) | DELETE 204 in 533 ms; of 100 probes over 25 s, 4 answered 200 (sent at 250, 1,250, 4,750 and 5,000 ms), 96 answered 401 (the first sent at 0 ms). Last 200 at 5.0 s, inside READ_CACHE |
| Cost | graft-m3f $0.18 from 10:54:42Z to 12:37Z (containers $0.15, about one standard-4 hour, see below); the account $0.83 over the same window, mostly other installs |

**Why the feed was empty** (site a's Workers Logs): every leader report failed with `near "commit": syntax error at
offset 66: SQLITE_ERROR` (repo:65609 indexes 1 to 10, authz:1, names:*, tokens:*), and the cron's round claim at
11:18:52Z failed with the same error. The registry object's `export_report` table named a column `commit`, which
SQLite reserves; no native test ran that SQL. Fixed on branch `export-commit-col` (column `idx`, the statements as
constants, rusqlite `sql_prepares`, and `tests/exportbed/run.sh`, which runs the site build in local workerd: on the
fix one report, one round, one 482-byte `export/` object; on the old schema both errors and no object).

**Teardown.** Shards torn down at 11:24Z with `KEEP_NETS=65609`, which left `git#0#net:65609` running until
12:04:41Z (a mistake: holding the seed needs the members, not the shard); then `teardown.py 65609 65609` (200, 1)
and a sweep with no shard running at 12:06Z. Held for the rerun: `repo:65609` (joined on all three sites), the
export bucket and the org token. The install token is revoked. The ssh fronts were not touched;
`ssh.graftyard.dev:22` answered throughout.

**Rerun** (approved under the standing queue once `export-commit-col` merges): a site-only day from merged main with
`--export-bucket graft-m3f-export`, a fresh install token, two rounds, then `pull.py --revoke` with the org control on
`identity:7100002` (m3-member, not owned by m3org), and the org token revoked at the end. A second fail on the same
cause stops the row.

## Hosted BF-49 m3fx49b (graft-m3f, 2026-11-22): fail on the Release line, a new cause

Recorded (operator) as a fail on "every chain's entries exactly once, Release included", on a cause other than m3fx49's, so
the two-in-a-row stop did not trip. The org, revocation and pack lines pass; the revocation's last 200 at 9,394 ms is
606 ms inside READ_CACHE.

Site-only day m3fx49b from main `6f60e609` (`export-commit-col`; `raw/bf49/siteday-m3fx49b.log`): 12:49:25Z to
13:01:28Z, exit 0, with `--export-bucket graft-m3f-export`; edge `0b02633c` kept (no edge change since `70c4f7ca`). One
push to the held `repo:65609` at 13:02:15Z (head `ce60842b`). A fresh install token (jti `01M81MXXV7YN46HQPQAKW9PW7J`);
the org token from m3fx49. Pull at 13:19:37Z (`raw/bf49/rerun/report.json`) after two rounds.

| Check | Result |
|---|---|
| Every chain's entries exactly once, Release included | **Release not carried.** Two cron rounds (13:04:22Z, 13:18:58Z) wrote 2 objects: 11 chains, 6,557 entries, 569,255 bytes, each chain contiguous from index 1. The 6 chains pulled through the entries endpoint match the feed byte for byte: `names:708` 1..1791, `names:727` 1..901, `names:500` 1..897, `names:661` 1..895, `tokens:482` 1..425, `repo:65609` 1..19. `release` (1..657) and `ids` (1..214) are served by the entries endpoint but were in no object |
| Org token's 404s | batches, `release`, `tokens:0` and `identity:7100002` (m3-member, a user m3org does not own, from `/api/v3/users/m3-member`) each 404; its own `repo:65609` 200 |
| Revocation against READ_CACHE (10,000 ms) | Install token: DELETE 204 in 555 ms; of 100 probes, 12 answered 200, the last sent at 9,394 ms, and 88 answered 401, the first at 527 ms. Org token: DELETE 204 in 782 ms at 13:21:08Z, 401 12 s later |
| Pack and snapshot | pack 200, 3,146,983 bytes, checksum equal to the body's SHA-256; `repo:65609` snapshot 200 |
| Cost | graft-m3f $0.05 from 12:49:25Z to 13:22Z |

**Why Release was not carried.** The feed takes a chain only once its leader reports (`Host::report_commit`, from the
leader's alarm). A site day resets every member, and a group with no traffic elects no leader: at 13:21Z `release`,
`ids` and `repo:65609` each had three followers and no leader. The chains that were carried are the ones the site
day's own reads and the 13:02 push woke. The SQL fix holds: no `commit` error after the deploy, and the reports land.

**Second fault: the queued next round never ran.** Each round sends the next as a queue message delayed 10 s; it
arrived as `{}` (site a, 13:04:38Z: `prsync: a message that does not decode: missing field repo`), so only the cron's
rounds ran, one per 15 minutes. `json!` built the body, and worker 0.8's queue send turns a `serde_json` map into a JS
`Map`, which the JSON content type sends as `{}`. Fixed on branch `export-next-round` with a struct, as the PrSync and
MergeQueue messages already are; `tests/exportbed/run.sh` now also waits for the queued round and fails on main.

**Teardown.** `teardown.py --sweep` tore down `git#0#net:65609` (the push started it) and found no shard container
running. Both tokens are revoked. `repo:65609` retired at 13:24:41Z (`tests/fanout/retirenets.mjs`, 3 members) and
purged at 13:25:36Z (`purge.py`, 33 objects, second pass 0; `raw/bf49/purge-65609.log`); a sweep at 13:26Z found no
shard running. The next rerun reseeds. The ssh fronts were not touched (slot A still `SHA256:TMEOnX…`).

**Ruling (operator):** a chain reaches the feed whether or not anyone has led it since the feed came on. Leader reports
stay the freshness signal; each round also covers the system chains from the genesis and `Keys` records, and a slower
sweep covers the repository chains from the `Names` and `Placement` folds, reading `/log_rows` from a non-leader up to
that member's commit. The next rerun must show `release` and `ids` in the feed, matched byte for byte against the
entries endpoint.

## export-sweep (code only, 2026-11-22): a chain reaches the feed whether or not anyone leads it

The ruling above, built on branch `export-sweep` from main `898dd7af`; no hosted run yet.

- **What a round covers.** The leaders' reports stay the freshness signal. Each round also probes a non-leader's
  commit index with an empty `/log_rows` range (`from` = cursor + 1, `to` = cursor, so no rows move). It probes
  `Release` and `Ids` from the genesis records every round (`export::ROUND_PROBES`), and the chains of one
  `MemberRegistry` shard of one site (`SWEEP_SHARDS` = 1, `sweep_slots`). Each chain is probed on the site its row names
  first (`swept`), under its row's record (`RegistryRow::record`, which `registry_record` now shares). The `export`
  object's new `export_sweep` table counts rounds, so a sweep of 3 × 64 shards takes 192 rounds, 32 minutes [guess].
  `with_probes` merges probes into the reports with the higher index winning. A reported chain is probed too, because a
  leader that resets before its next report would otherwise leave that report behind its log for good.
- **Why the registry rows, not the `Names` and `Placement` folds.** The ruling named the folds as the repository
  chains' source. The sweep reads `MemberRegistry` rows instead, for two reasons. Each group's members write their row
  before they store an entry, so the rows list every group with entries: repositories, networks, owner, Actions,
  `Names`, and `Tokens` chains alike. Reading a fold goes to its `Names` leader through `fresh`, which would elect every
  `Names` shard the sweep touches after a site day. The rows are read from each site's own registry objects, with no
  election. One probe per chain per sweep keeps an idle chain at one cheap read every 32 minutes (`scripts/cost.py`:
  the base moves $238 → $239, $1,017 → $1,018 and $9,330 → $9,335). Probing all 1,024 + 3,072 `Names` and `Tokens`
  shards every round would cost a read per shard per round.
- **Tests.**
  - `crates/git/tests/export.rs`, `a_chain_whose_leader_never_reports_still_reaches_the_feed_exactly_once`: 39 chains
    nothing reports, plus `Release`, `Ids`, and one chain whose leader reports. Over two sweeps, with a lost state write
    every 23 rounds, every entry is in the feed exactly once, and `Release` and `Ids` are in the first object.
  - `export.rs` unit `the_sweep_visits_each_shard_once_and_a_probe_never_lowers_a_report`.
  - `tests/exportbed/run.sh` drives a second group, `repo:900099` in registry shard 0, which is the sweep's first slot.
    After its put, every member resets (`/abort`), so it never elects again. On this branch the round's object carries
    it from 1 to 3 with all three members followers (`reports=1 probed=1/1 planned=2`), and the bed passes. On main
    `898dd7af`'s build the same bed fails: the object holds only the led group, and the idle group is "carried []".
    workerd has no trust anchor, so the bed's rounds log "no record for release/ids". The hosted sites deploy with
    `--anchor`.
  - `tests/m0carried/plant84.py`: 33 of 33 caught, including six new sweep mutants: planning from reports only, a
    probe lowering a report, `Ids` not probed, the sweep stuck on site 0, a chain probed on every site that holds it,
    and the sweep counter's SQL.
  - The full checks pass: fmt; workspace clippy; wasm32 clippy for site and edge, with and without `test-faults`, and
    for ops, front and runners; `cargo test --workspace`; `go test` in `containers/git/server` and
    `tests/m0/porcupine`; the three `scripts/check_*.py`; and `tests/am1/rest/h1.test.mjs`. The edge is unchanged, so
    edgeboot did not run.
- **The rerun after merge** is a site-only day with a fresh seed. It must show `release` and `ids` in the feed, matched
  byte for byte against the entries endpoint. After the day's reset their groups have no leader, so only the probes can
  carry them.
