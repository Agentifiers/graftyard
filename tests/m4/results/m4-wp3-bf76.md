# BF-76 (`upload-artifact@v4` and `download-artifact@v4` through the forks and the pin map) on graft-m4a

Status (2026-11-26): **heads 1 to 3 built at $0, not run hosted.** The row runs only on the lead's GO. Stop $3.

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
