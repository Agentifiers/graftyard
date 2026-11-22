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
