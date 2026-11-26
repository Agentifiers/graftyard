# BF-76 (`upload-artifact@v4` and `download-artifact@v4` through the forks and the pin map) on graft-m4a

Status (2026-11-26): **heads 1 and 2 built at $0, not run hosted.** Head 3 (Twirp v4 and the block shim, and the forks) is still to build. The row runs only on the lead's GO, after head 3. Stop $3.

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

## The hosted plan (after head 3, on GO)

Install graft-m4a, $3 stop, read by `acctcost.py` before each step.

1. **Site day** from the row's main: the 0x15.10 reader (`ACTIONS_FORMATS`) and the 0x12.15 reader. Every live flag is kept.
2. **Edge day**: `FORMATS_ENABLED` adds 21.10 and 18.15, keeping every value it holds, 21.7, 21.8 and 32.28 among them.
3. **Setup**: the org `graftyard-actions` with the forks `upload-artifact` and `download-artifact`, built by head 3's reproducible build from upstream v4.6.2 and v4.3.0. The upstream tags go in the mirror org (`actions/*` is held Deleted on m4a, so `actions-mirror`, as BF-71 did). A pin map naming both pins, signed with the key in `~/.graft-m4a/release.key`, is uploaded by the site admin.
4. **Tag job**: org policy `{tags: true}`. One workflow uses `actions/upload-artifact@v4` and `actions/download-artifact@v4`. Pass: both jobs succeed; the job log's download line names the fork's SHA; every file's SHA-256 matches.
5. **Unapproved pin**: policy `{tags: false}` and no approval. The workflow pins `actions/upload-artifact@<upstream v4.6.2 sha>`. Pass:
   - the job fails at that step, with `##[warning]Artifact upload failed with error: GHESNotSupportedError: ...` and then `##[error]@actions/artifact v2.0.0+, upload-artifact@v4+ and download-artifact@v4+ are not currently supported on GHES.`;
   - the job's check run lists the step's failure annotation and Graftyard's annotation, which names `/organizations/<org>/settings/actions/pin-map` and `graftyard-actions/upload-artifact@<fork sha>`, in both REST and GraphQL.
6. **Approval**: policy `{pin_map: <digest>}`, then the same workflow re-run. Pass: it succeeds and the files match.
7. **Teardown**: the repositories and runs are deleted, and the policy is withdrawn. No containers.

The sheds are reported apart, as elect and tierOff (`tests/fanout/tieroff503.py`).
