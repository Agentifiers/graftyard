# BF-76 (`upload-artifact@v4` and `download-artifact@v4` through the forks and the pin map) on graft-m4a

Status (2026-11-26): **hosted r1 ran from main 12107824: 3 of 4 gate lines PASS, the annotation line FAILS** on an edge bug (`pinmap::policy` skips the install's current map when nothing is approved), and withdrawing an approval is a no-op (`{pin_map: null}` reads as "keep" on the site). Results are in "Hosted r1" below. Cost $0.30 against the $3 stop. 0 sheds.

The row (`build/BUILD-PLAN.md`, M4-WP3) passes when:

- the tag-substituted jobs run the `graftyard-actions` forks, and every file arrives SHA-256 identical;
- the unapproved SHA pin fails at the same step and with the same log line as on GHES 3.22;
- that job's check run lists a failure annotation naming the pin-map approval page and the matching fork SHA, in REST `check-runs/{id}/annotations` and in GraphQL `CheckAnnotation`;
- after the org admin approves the pin map, the same workflow passes.

## What is built

- **Head 1** (`bf76-policy`, merged as main fa8db187): Authz 0x12.15 `SetActionsSubstitution { tags, pin_map }`, an org admin's, and `graftyard_actions::pin_map`. The pin map is verified against `Keys.release`, then the approved digest, then its content. `substitute` handles tags, approved pins, and unapproved v4+ pins. Plant254 caught 9 of 9.
- **Head 2** (`bf76-annot`):
  - **Annotations.** `completejob`'s `stepResults[].annotations` and its job-level `annotations` are mapped by `graftyard_actions::annotations`, together with Graftyard's pin-map annotation for each unapproved v4+ pin a step ran. The mapping is GHES's: level, path `.github` when none, lines at least 1, title up to 255 characters. A job keeps at most 50 annotations and 8 KB of their text. The edge sends the annotations only once `FORMATS_ENABLED` holds 21.10. The shard commits them as `ActionsEvent::JobAnnotations` (tag 20) in the entry of the job's `Finished`, through `ActionsOp::FinishAnnotated` (tag 25, Actions format 0x15.10). `RunOwner` writes them as the completed check run's `output`.
  - **Reads.** REST `GET /repos/{o}/{r}/check-runs/{id}/annotations` serves them in GHES's shape, paged, with `blob_href`. GraphQL `CheckRun.annotations` serves them as `CheckAnnotation` with `location { start end }`. An Actions job's check run shows a null `output.title` and `output.summary`.
  - **Pin map and policy routes.** `PUT /api/graftyard/v1/actions/pin-map {document, signature}` is a site admin's. It verifies the map under `Keys.release`, then stores it at `actions/pin-maps/<digest>` and as `actions/pin-maps/current` in the `Release` group's home bucket. `GET` and `PUT /api/graftyard/v1/orgs/{org}/actions/substitution {tags?, pin_map?}` is an org admin's, and it approves only a digest the install holds and verifies.
  - **Launch substitution.** For each call, the launch service reads the run owner's policy and both maps. A tag under tag substitution, or an approved pin, resolves to the fork: `resolved_name` is the fork, `resolved_sha` is the fork's commit, and `name` and `version` stay as written. So the runner's line reads `Download action repository 'actions/upload-artifact@<as written>' (SHA:<fork sha>)`, and the edge logs one line per substitution naming both references and both SHAs. An unapproved pin resolves as written and runs the upstream bytes, which fail in their own step with GHES's lines.
  - **Plant258** caught 12 of 12.

- **Head 3** (`bf76-twirp`): v4 artifacts through Twirp `ArtifactService` and the Azure block shim, and the forks (`decisions/DECISIONS.md`, "`actions-artifacts`", the v4 amendment).
  - **Twirp** at `/twirp/github.actions.results.api.v1.ArtifactService/*` on the install's root (the client keeps only the origin of `ACTIONS_RESULTS_URL`), with the job token: `CreateArtifact` (409 for a name the run finalized), `FinalizeArtifact`, `ListArtifacts`, `GetSignedArtifactURL` (BF-75's one-minute zip URL), `DeleteArtifact` (BF-75's delete). A call naming another run is 403.
  - **Block shim** at `/_services/artifact-blob/<owner>/<shard>/<repo>/<ticket>`: Put Block stores one object per block, and Put Block List commits the listed blocks of that upload in order. The zip route streams a committed v4 blob as stored, so REST's zip and `gh run download` serve v4 artifacts too.
  - **Job token and message**: the job token carries `scp: Actions.Results:<plan GUID>:<job GUID>`. Under the site var `GRAFTYARD_ARTIFACTS_V4=1`, the message names `ResultsServiceUrl` and sets `GRAFTYARD_ACTIONS=true` as job-level `environmentVariables`. No format and no Actions event: 0x15.11 is unused.
  - **Forks**: `bash tests/m4/wp3/bf76/build-forks.sh <out> [<mirror dir>]` changes every bundled `isGhes()` in `dist/` and commits with a fixed author and date. Two builds gave the same SHAs:

    | Fork | Upstream tag and SHA | Fork SHA |
    | --- | --- | --- |
    | `graftyard-actions/upload-artifact` | v4.6.2 `ea165f8d65b6e75b540449e92b4886f43607fa02` | `aeb6043fcd99c3bdff3a1b86b98a3d66bf60897e` |
    | `graftyard-actions/download-artifact` | v4.3.0 `d3f86a106a0bac45b974a628896c90dbdf5c8093` | `373906d2915ced2e53ba59693e2d2acf30171209` |

  - **Edgeboot**: `tests/edgeboot/run.sh` runs the real v4.6.2 and v4.3.0 bundles with the forks' change against the edge's own wasm (`tests/m4/wp3/bf76/v4bed.sh`). It uploads a 20,000,000 B file in three blocks, a nested file, and an empty file (final zip 20,000,347 B); a second upload of the name is refused; the download's digest matches and every file is identical.
  - **Plant269** caught 12 of 12. On the edge's own wasm, a planted `commit` that reverses the list failed the edgeboot v4 check: `download-artifact` gave up after 5 retries and all three files differed. A plant that sorts the blocks by key instead passed it, and that is expected: `generateBlockID` builds every id as one UUID prefix plus a zero-padded index, so key order is list order for this client. Plant269's staging-order mutant is caught by the unit test, whose ids are not in key order.

## The hosted plan (on GO)

Install graft-m4a, $3 stop, read by `acctcost.py` before each step.

1. **Site day** from the row's main: the 0x15.10 reader (`ACTIONS_FORMATS`), the 0x12.15 reader, and `GRAFTYARD_ARTIFACTS_V4=1`. Every live flag is kept.
2. **Edge day**: `FORMATS_ENABLED` adds 21.10 and 18.15, keeping every value it holds, 21.7, 21.8 and 32.28 among them.
3. **Setup**: the org `graftyard-actions` with the forks `upload-artifact` and `download-artifact`, pushed from `build-forks.sh`'s output (fork SHAs above, tags `v4` and the upstream tag). The upstream tags go in the mirror org (`actions/*` is held Deleted on m4a, so `actions-mirror`, as BF-71 did). A pin map naming both pins, signed with the key in `~/.graft-m4a/release.key`, is uploaded by the site admin.
4. **Tag job**: org policy `{tags: true}`. One workflow uses `actions/upload-artifact@v4` and `actions/download-artifact@v4`. Pass: both jobs succeed; the job log's download line names the fork's SHA; every file's SHA-256 matches.
5. **Unapproved pin**: policy `{tags: false}` and no approval. The workflow pins `actions/upload-artifact@<upstream v4.6.2 sha>`. Pass:
   - the job fails at that step, with `##[warning]Artifact upload failed with error: GHESNotSupportedError: ...` and then `##[error]@actions/artifact v2.0.0+, upload-artifact@v4+ and download-artifact@v4+ are not currently supported on GHES.`;
   - the job's check run lists the step's failure annotation and Graftyard's annotation, which names `/organizations/<org>/settings/actions/pin-map` and `graftyard-actions/upload-artifact@<fork sha>`, in both REST and GraphQL.
6. **Approval**: policy `{pin_map: <digest>}`, then the same workflow re-run. Pass: it succeeds and the files match.
7. **Teardown**: the repositories and runs are deleted, and the policy is withdrawn. No containers.

The sheds are reported apart, as elect and tierOff (`tests/fanout/tieroff503.py`).

## Hosted r1 (2026-11-26, 07:10Z to 07:40Z, main 12107824)

Driver: `tests/m4/wp3/bf76/row.sh r1 <step>` with `bf76.mjs`. Raw output: `tests/m4/raw/wp3/bf76/r1/`, scrubbed, with no runner keys.

**Days.**
- **Site day:** a `a80a78cf`, b `82ab652f`, c `1cf7c2b4`, from `777ea0b0`/`c45c5df4`/`de2bbc0f`. Every live flag is kept (REUSABLE, OIDC_CLAIMS, OIDC_HEAD_REF, the rest of BF-73's), and `GRAFTYARD_ARTIFACTS_V4=1` is added. No push trigger.
- **Edge day:** `faaf6f06`, from `d6ab185a`. `FORMATS_ENABLED`, read live, kept its 34 formats and added 21.10 and 18.15. The binding diff is `FORMATS_ENABLED` only.
- **Ops:** `ac4cd9a5`, untouched.

**Setup (PASS).**
- `build-forks.sh` gave the fork SHAs in the table above again.
- `graftyard-actions` (account 7100013) holds `upload-artifact` (repo 4653056, `v4` and `v4.6.2` at `aeb6043f`) and `download-artifact` (4653057, `v4` and `v4.3.0` at `373906d2`).
- `actions/upload-artifact` (3997698) holds `v4.6.2` at `ea165f8d`, and `actions/download-artifact` (3997699) holds `v4.3.0` at `d3f86a10`. Mirroring into `actions` worked: neither name had been held.
- The pin map, signed with `~/.graft-m4a/release.key`, was stored with 201 and digest `5277fe93c5ab4d8679114e3e784778f64094fbb13736ea530191c2642f613b93`. The same document under another key got 422, and with no token 401.

| Gate line | Verdict | Evidence |
| --- | --- | --- |
| Tag job runs the forks, files identical | **PASS** | Run 1972513, policy `{tags: true}`: `up` and `down` both Success. `Download action repository 'actions/upload-artifact@v4' (SHA:aeb6043f…)` and `'actions/download-artifact@v4' (SHA:373906d2…)`. `BF76 ALL SUMS OK` covers the 20,000,000 B file, the nested file and the empty file. BF-75's REST zip of the same artifact (302, then 200, 20,006,465 B) passes `unzip -t`, and its three SHA-256s equal the job's. `retention-days: 2` reads as `expires_at - created_at` = 2 days. No `GHESNotSupportedError` |
| Unapproved pin fails at GHES's step with GHES's lines | **PASS** | Run 1972518, policy `{tags: false, pin_map: null}`: Failure. Set up job downloads `actions/upload-artifact@ea165f8d…` at `SHA:ea165f8d…` (as written). The step `Run actions/upload-artifact@ea165f8d…` logs `##[warning]Artifact upload failed with error: GHESNotSupportedError: …` and then `##[error]@actions/artifact v2.0.0+, upload-artifact@v4+ and download-artifact@v4+ are not currently supported on GHES.` (`run/pin-job-pinned-1972519.log`, lines 16, 28 and 33) |
| Check run lists Graftyard's annotation (link and fork SHA), REST and GraphQL | **FAIL** | Check run 1972519 is `completed`/`failure` with `annotations_count` 2: the runner's warning (line 13) and its failure (line 18), with GHES's message, both served by REST `check-runs/{id}/annotations` (`pin-1.log`; the second pin run overwrote `run/pin-annotations.json` and `run/pin.json` with its own). **Graftyard's annotation is missing.** The GraphQL check never ran in that run (see the second bug below) |
| After approval the same workflow passes | **PASS** | `{pin_map: 5277fe93…}` answered 200; a digest the install does not hold got 422. Run 1972521: Success. `Download action repository 'actions/upload-artifact@ea165f8d…' (SHA:aeb6043f…)`, with no `GHESNotSupportedError`. The REST zip's `x.bin` SHA-256 equals the job's |

**Why the annotation is missing [measured, then read in code].** `pinmap::policy` (`workers/edge/src/actions/pinmap.rs`) returns `Policy { tags: false, approved: None, known: None }` as soon as the org has neither tags nor an approved map, so it never reads `actions/pin-maps/current`. `pin_map::substitute` finds the pin only in `known`, so with `known` None it answers `AsWritten`, not `Unapproved`, and `pinmap::annotations` builds nothing. That is exactly the gate's state, "no approval". Plant258's tests cover `substitute` with `known` supplied, and the edgeboot never commits `completejob` under a policy, so neither reached this return. Fix: read `current` whenever the job ran a forked action, whatever the policy, and add an edge-level test that fails with the early return in place.

**Second bug: an approval cannot be withdrawn [measured].** `PUT …/substitution {tags: false, pin_map: null}` answered 200, but the GET still read `pin_map: 5277fe93…`. The edge builds `SetActionsSubstitution { pin_map: Some(None) }`, and `commit_as` sends it to the site as JSON. serde writes `Some(None)` as `null` and reads it back as `None`, which means "keep". A scratch round-trip of the op printed `{"SetActionsSubstitution":{"tags":false,"pin_map":null}}`, which read back as `pin_map: None`. The approval from step 6 therefore stayed. The second pin run (`pin-2-tags.log`, run 1972524, policy `{tags: true}`) found the map still approved and passed, so it is not a gate result. The fix belongs in the op's JSON shape, for example `#[serde(with = ...)]` that keeps `Some(None)` distinct. The canon bytes (`e.u8(39)`) do not change, and no wire tag is added.

**GHES gap found on the way:** `GET /repos/{o}/{r}/pulls` (list) answers 404 on the edge. Only `pulls/{n}` and its sub-routes are served. The driver opens the pull request by GraphQL and keeps its number.

**Teardown.**
- `m3org/bf76-r1` deleted (204, then an anonymous read gets 404). Tag substitution is off.
- Because `null` withdraws nothing, the driver approved an empty map signed the same way (`43dd75a6…`, which approves no pin) and stored the real map again as `current`. m3org's policy reads `{tags: false, pin_map: 43dd75a6…}`.
- `teardown.py --sweep --shardkill` tore down 5 shards (all 200), found no shard container running, and deleted shardkill.
- The forks and the `actions/*` mirrors stay on m4a for the rerun.

**Cost and sheds.** `acctcost.py` from 07:11:13Z to 07:39:42Z: graft-m4a **$0.30** (containers $0.14 of it), against the $3 stop. The account's $2.88 total is mostly `graft-m6a-site-a-gitshard` ($2.54), which is not this row's. `tieroff503.py` over the same window: elect 0, tierOff 0, edge 503 0. The new repository's 503 retries: none.

**Rerun needs:** the two fixes above as one head (edge, and the site for the op's JSON). Then a site day and an edge day from that main, and `row.sh r2 pin` then `approve` on a fresh repository; `setup` and `tag` need not repeat.

## The r1 fixes (branch `bf76-fix`, from main 321af57d, $0)

- **The annotation reads the current map whatever the policy.** `pinmap::annotations` calls `policy_from(.., known: true)`, which reads `actions/pin-maps/current` even with tags off and no approval, so an unapproved pin is `Unapproved` and Graftyard's annotation comes before the runner's. The launch service passes `known: false` and reads as before. The reads sit behind `MapReads` (the edge's `Live` and a test fake). Test: `workers/edge/src/actions/pinmap.rs`, `an_unapproved_pin_gets_the_pin_map_annotation_with_tags_off_and_nothing_approved`. On main's early return it fails with "the current map is read with tags off and nothing approved: left 0, right 1".
- **`{pin_map: null}` withdraws.** `SetActionsSubstitution.pin_map`'s JSON form (`authz.rs`, `pin_map_change`) writes a withdraw as `"withdraw"`, reads `null` or an absent field as keep, and reads a digest as approve. Binary formats keep the derived form. Test: `crates/identity/tests/agents.rs`, `actions_substitution_withdraw_survives_the_edge_to_site_json`, which round-trips keep, withdraw and approve through JSON and the raft codec, and pins postcard's `27 00 00` (the wire-tag golden's sample), `27 00 01 00` and `27 00 01 01 <digest>`. On main's shape it fails: `{"pin_map":null}` read back as `pin_map: None`.
- **Plant274:** 8 of 8 caught.
- The `GET /repos/{o}/{r}/pulls` list 404 from r1 is M4-WP4's to file and fix (plant 276), not this head's.

**r2, pre-GO'd by the lead, $3 stop:** a site day and an edge day from the main that carries the fix, then `row.sh r2 withdraw` (m3org from r1's empty-map approval to none, by `null`), `FROM=r1 row.sh r2 pin` and `FROM=r1 row.sh r2 approve`, then `teardown`.
