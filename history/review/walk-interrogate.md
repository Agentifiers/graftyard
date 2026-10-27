# Implementability walk: triage and act-on items

Two fresh read-only agents walked the restructured folder as implementers on 2026-10-26:

- `history/review/walk-base.md` (WalkBase) covers Milestones 0 to 3. Its findings are cited here as WB-1 to WB-20.
- `history/review/walk-ops-agents.md` (WalkOpsAgents) covers Milestones 5 and 6 and AM1 to AM3. Its findings are cited here as WO-1 to WO-20.

The build plan writer also reported five sequencing gaps, R4 to R8, in `decisions/OPEN-QUESTIONS.md`. Both walkers ran on anthropic/claude-opus-5-5, and neither wrote any of the text it walked.

The lead read all 40 findings. Every finding is acted on, because each names a behavior two implementers would build differently. Where a walker offered a choice, the lead's decision is stated in that item.

**Line caps.** The caps rise to 1,800 lines for `design/DESIGN.md` and 1,000 for `design/AGENT-LAYER.md`, in `scripts/check_design.py`. Completeness for implementers outranks length now.

## Ownership rules for the four writers

The four writers are P, M, R, and A. Each slice owns the regions named in its heading. To change another slice's region, send the exact text to that writer.

New gate rows are never written into the Build order tables. Each writer lists them in its report as `Milestone | Measurement | Pass | Fail`, and writer B adds them afterward. Every addition keeps the evidence tags, and values that are made up are tagged [guess].

## Slice P: protocol and storage

**Regions owned.**

- `design/DESIGN.md`, Shape: "Sites and placement", "Chains", "Leadership", "The write path", "The read path", "Packs", and "Git cache shards (derived)".
- `design/DESIGN.md`, "Commit and read paths": the intro and journeys 1 and 5.
- `design/SHARED.md`: "Refs and pushes", "Multi-chain operations", the snapshot paragraph of "Folds", and "Record formats and the two-release rule".

**Items.**

- **WK-P1 (WB-1).**
  - Add a placement record per owner and per system chain kind, with `PlacementName`, `home`, and generation. State where it lives.
  - Bootstrap rule. A group's first entry is `Config(Normal(placement))`, built only from that record. A member with no committed config never serves or votes.
  - `NotLeader` and config replies carry the membership.
- **WK-P2 (WB-3).**
  - Inline packs are promoted to a `DurablePack` before their range is archived, and an entry records the promotion.
  - Each shard advances a per-network ingest cursor through `Refs` and `Merged` entries.
  - Define rehydration: stored packs, plus inline packs after the last promotion.
  - `Compacted` names promoted packs.
- **WK-P3 (WB-6).**
  - Entries carry `(RequestId, event range)`.
  - The edge mints a `RequestId` per client request.
  - A per-group request table has a stated retention and answers duplicates from the stored events.
  - Every ticket, refusals included, resolves only when its entry commits.
  - A hedge is a `suspect` report and never a forwarded command.
- **WK-P4 (WB-7).**
  - Commands carry `credential()` naming the token, `jti`, or SSH key shard.
  - `deps` takes the fold.
  - Owner and enterprise ids are stored in the repository core tier, so a transfer updates them by entry.
  - Fix the inline capacity of `deps` to 3.
- **WK-P5 (WB-8).**
  - Member-to-member and edge-to-member messages follow the two-release rule. A new command or field is sent only after the receiving release is enabled.
  - An unknown variant returns a typed `Unsupported`, which the edge renders as a retryable 503.
- **WK-P6 (WB-9).**
  - Name the snapshot writer, for example one follower chosen by index, and the cadence.
  - Define a chunked row-stream snapshot format stamped with one index, produced without blocking apply.
  - Add an install-snapshot step for a member whose next entry is archived.
- **WK-P7 (WB-10).** Cap the encoded size of one command below the entry limit. An atomic push over the cap stores its pack instead of inlining it, and is refused whole, with a GitHub-style message, when its ref updates alone exceed the cap.
- **WK-P8 (WB-11).** A network-level fold table, or a create-only marker per committed pack written after `Acked`, maps each committed `PackId`. The orphan sweeper and the third-bucket filler read it.
- **WK-P9 (WB-12).** The placement round-trip check runs at join, and daily only for groups that served requests that day. Measured round trips live outside `GroupConfig`.
- **WK-P10 (WB-19).** Member object names are `{chain}#{generation}#{kind}{slot}`.

## Slice M: model

**Regions owned.**

- `design/SHARED.md`: "Wire compatibility", "Chain events and commands" except the parts slice P owns, "Identity and tokens", "Issues and pull requests", and "Event fan-out".
- `design/DESIGN.md`, Shape: "Merge queue" and "Identity", except the token-shard functions `jti_shard`, `token_shard`, and `fingerprint_shard`, which do not change. Also the list "Reused with the additions below".

**Items.**

- **WK-M1 (WB-4).**
  - Add a `QueueMerge { plans, entry }` command whose `decide` is all-or-nothing.
  - Define the `QueueEvent` variants: enqueue, group built, ejected, and landed.
  - Add a `MergeQueue` consumer that builds groups, creates `gh-readonly-queue/*` refs, reacts to `RequiredCheck` on queue refs, ejects failures, and submits `QueueMerge`.
- **WK-M2 (WB-5).**
  - Check runs, check suites, and statuses become metadata-tier `RepoEvent`s, pruned after their retention. `RequiredCheck` stays core.
  - Add `TestMerged { n, base, head, result, pack }`.
  - Remove SHARED's "off the chain" wording for check runs.
- **WK-M3 (WB-13).**
  - Define the blob store as `R2Key::Blob { repo, oid }`, placed in two buckets the way packs are.
  - Add the `Release`, `ReleaseAsset`, and `LfsObject` repository events.
  - Archive caches are derived and live in the clone-cache bucket.
- **WK-M4 (WB-14).** `TokensEvent::Stored` gains these optional fields: `app`, `client`, `permissions`, `resource_owner`, `repos`, and `paired`.
- **WK-M5 (WB-15).** Add `AppRecord`, which holds keys, permissions, events, the webhook URL, and the secret hash, and `OAuthAppRecord`, which holds the client id, callbacks, and the secret hash. Both sit in the core tier of the owner's `Authz` chain. `Names` keeps only the route from app to owner. `Identity` stops holding client secrets.
- **WK-M6 (WB-16).**
  - Device codes, user codes, authorization codes, and the sign-in link are TTL rows on `Tokens` shards, keyed by the code's hash.
  - `InstanceAuth` is an install-scope entry. Name its chain.
- **WK-M7 (WB-17).**
  - Outside collaborators are `Collaborator` entries on the repository chain. Org members and teams are `Grant` entries on `Authz`. The two never overlap.
  - Add `Invitation` events with accept and decline on the chain that owns the fact.
- **WK-M8 (WB-18).** The edge writes the `Identity` side of a new credential right after the `Tokens` commit. `Audit` only finishes operations that nobody touched. Make SHARED and DESIGN say the same.
- **WK-M9 (WB-20).**
  - Point every citation at SHARED's real headings.
  - Define `CommentEvent`, `ReviewEvent`, `LabelEvent`, `BranchRule`, `RepoSettings`, `IssueCapture`, and `CheckRunUpdate` as stored formats in SHARED.
  - The DESIGN Milestone 2 row must cite `design/SHARED.md`, not `history/`. Ask writer B to change it.

## Slice R: release and operations

**Regions owned.** All of these are in `design/DESIGN.md`:

- Shape: "Release group", "SSH front", and "Lifeboat feed";
- "Usage", the operator block;
- "Install operations";
- the `release control`, `vault`, and `lifeboat` rows of "Failure modes";
- the artifact paragraph of "Deployment modes".

**Items.**

- **WK-R1 (WO-1).**
  - Add `IntentStep { intent, step }`, `IntentDone { intent }`, and `SiteRelease { site, release }`, signed by a new narrow `Keys.executor` key held by `graft-ops`, which can sign these three entries and nothing else.
  - Add one paragraph that maps `graftyard release roll` to its ordered intents.
- **WK-R2 (WO-2).**
  - Add `ReleaseUploaded { release, record, versions: [(script, VersionId)], images, reads: FormatSet }`, committed by `graft-ops` with the executor key after the release signature verifies. Human-signed intents name a `ReleaseId`. `graft-ops` resolves it to `VersionId`s from this entry.
  - Define `Genesis`, `InstallId`, `VersionId`, `ReleaseId`, `Format`, `FormatSet`, `Scope`, `SecretName`, and `ImageRef`.
- **WK-R3 (WO-3). Lead decision: pointers are pin-only overrides.**
  - Each pointer is `Option<VersionId>`. `None` means the request follows the deployment's percentage split. `Some(v)` pins every request to `v`, which is the swap-back.
  - Only pre-signed undos set `Some`. The next signed intent clears it.
  - No forward pointer move exists, so no key needs to sign one.
- **WK-R4 (WO-4).**
  - Sites are created before genesis.
  - `graftyard genesis` creates the drain and executor keys inside `graft-ops`'s Secrets Store and emits an unsigned genesis for offline signing.
  - The trust anchor is the install id and the customer public key, set as script variables when Wrangler creates the scripts.
  - An empty `Release` group accepts only a genesis that matches the anchor.
  - Fix the Usage block order.
- **WK-R5 (WO-7).**
  - `Bound.prev` is the digest of the last committed signed `ReleaseEntry`, excluding `Noop` and `Config`.
  - Add `POST /api/graftyard/v1/release/entries`, which answers 409 with the current digest when `prev` is stale.
- **WK-R6 (WO-8). Lead decision: `Abort` carries an explicit target.**
  - It is `Abort { intent, target: ReleaseId }`, refused and paged when the target cannot read an enabled format.
  - Make BF-7 and BM-5 agree with this. Ask writer B to edit the rows.
- **WK-R7 (WO-9).**
  - `ReplaceMember` and `RestorePlacement` are `ReleaseEntry` intents. Name their signers, including the automatic path's key, which is the executor key, after 24 h of zero acks.
  - Name who measures zero acks: the other sites' `SiteHealth`.
  - The enumeration source is a per-site member registry kept on the `Release` group or on each site's script. Choose one and state it.
- **WK-R8 (WO-10).**
  - Specify the export endpoints: change batches, chain entries from an index with their bytes, and packs.
  - Specify the export and `lifeboat:read` token claims.
  - Add the enroll endpoint, the `LifeboatDecoder` submit path, and which bucket holds `R2Key::Export`.
- **WK-R9 (WO-14).** Undos are validated against the `Keys` that were current at the parent intent's index. After `Keys { vendor: None }`, a `Resume` must carry undos re-signed by the customer.
- **WK-R10 (WO-15).**
  - Add a table of compiled site-day gate and dwell constants, all [guess].
  - Add an undrain rule signed by the drain key.
  - Site and edge isolates cache the full `Release` view with `POINTER_TTL`.
- **WK-R11 (WO-17).** Add a rotation table with one row per secret: who acts, where the new value comes from, how it reaches Secrets Store, and what `Rotate { secret }` makes `graft-ops` do. Name the signer of `Rotate`.
- **WK-R12 (WO-18).**
  - The Workflows script joins the signed artifact and rolls on site days through `SiteDay` targets.
  - The vault Worker ships in the artifact. `graftyard vault upgrade` runs under the vault account's token.
- **WK-R13 (WO-19).**
  - `StaleMode` takes the customer key.
  - Define `Scope`.
  - Members and isolates accept a signed flag that has not committed, after verifying it against their cached `Keys`, because the `Release` group may be the one that cannot commit.
- **WK-R14 (WO-20).**
  - SSH host keys live in Secrets Store and reach containers through the outbound handler.
  - Add them to the rotation table and publish them in `/meta`.

## Slice A: agents

**Regions owned.**

- All of `design/AGENT-LAYER.md` except "Build order".
- The agent block in `design/DESIGN.md` Shape, which must stay byte-identical.

**Items.**

- **WK-A1 (WO-5).**
  - Add a front Worker in the agents account that pins controller traffic by `ControllerPointer`, following WK-R3's pin-only rule.
  - Add a `Release` read endpoint. Its entries are verified against the install id and the customer key that `graftyard agents init` provisions.
- **WK-A2 (WO-6). Lead decision.**
  - A token's `task` claim is valid when its App is the task's App or the install's `graftyard-workspace`, and its repositories are the task's repository or that repository's workspace fork.
  - `graftyard-workspace` pushes to the fork.
  - Give each of the four first-party Apps its manifest: permissions and events.
- **WK-A3 (WO-11). Lead decision.**
  - `graftyard apps init` runs in the install account in AM2. It registers `Graftyard MCP` and `graftyard-insights`, with their keys and the client secret in the install's Secrets Store.
  - `graftyard agents init` registers only `graftyard-agent` and `graftyard-workspace`, in the agents account.
  - A self-hoster without a controller still gets MCP and insights.
- **WK-A4 (WO-12).**
  - Add grant endpoints under `/api/graftyard/v1/mcp/grants`, authenticated by the `Graftyard MCP` App's client credentials.
  - Add the additive `token/scoped` fields for the grant id and `WriteScope`.
- **WK-A5 (WO-13).**
  - Write a compact OpenAPI-style fragment, as a table or a YAML block, for every `/api/graftyard/v1` endpoint, with bodies, field names in snake_case, pagination by GitHub's `Link` header, and error bodies. Cover the 409, 410, and 422 responses.
  - Define the cursor encoding, `wait` in seconds with a maximum of 25, and `paths` as repeated parameters.
  - The `SessionId` rule: a UUID minted by the host for Graftyard-run tasks, and a ref-safe id unique per repository and App for external ones.
  - Define the `graftyard-plan` block grammar.
  - Define the first-party task delivery's event name, payload, and signing secret.
- **WK-A6 (WO-16).**
  - Install-wide `AgentApp` rows live on the install's own `Authz` chain, which `Genesis` creates. `decide` reads it as a dependency.
  - Add settings endpoints for `AgentApp`, `AgentPolicy`, `agents_enabled`, and in-repo mode.
  - Uninstalling an App makes its tasks read as cancelled through the same check that releases its leases. No sweep is needed.
- **WK-A7.** Fix the two minor gaps WalkOpsAgents left below its cut:
  - list the invisible and bidirectional character set and the resources that get `graftyard_trust`;
  - name the repository in a fork-mode session URL. It is the base repository.

## Slice B: build order, after P, M, R, and A finish

**Regions owned.** The "Build order from zero" section of `design/DESIGN.md`, the "Build order" section of `design/AGENT-LAYER.md`, `build/BUILD-PLAN.md`, and `decisions/OPEN-QUESTIONS.md`.

**Items.**

- **WK-B1 (WB-2). Lead decision.** Milestone 1 builds the minimal identity real code needs:
  - the `Ids` group;
  - `Names` login and repository-name claims;
  - an `Authz` owner record with admin grants and `RepoName`;
  - stored-token authentication.
  Milestone 3 completes OAuth, Apps, installation tokens, SSO, and learners. No test-only resolver ships.
- **WK-B2 (R4).** Split the Milestone 3 suspension row. The PAT part stays in Milestone 3, and the SSH part moves to Milestone 6.
- **WK-B3 (R5).** The change feed (`ChangeBatch` and export) moves into Milestone 5 with the vault.
- **WK-B4 (R6).** AM2 and AM3 run after base 5.
- **WK-B5 (R7).** Cursors, the Queue plumbing, and the `PrSync` consumer move into Milestone 2.
- **WK-B6 (R8).**
  - Every T0 surface in SHARED's table gets a milestone.
  - The web app gets a milestone, with a gate that names its oracle.
  - Tag each placement [inference].
- **WK-B7.** Add every gate row that writers P, M, R, and A report, plus the BF-7 and BM-5 changes from WK-R6, and the Milestone 2 citation fix from WK-M9.
- **WK-B8.** Regenerate the work packages and the coverage table in `build/BUILD-PLAN.md`. Update `decisions/OPEN-QUESTIONS.md`: remove R4 to R8 as answered, and add any question the writers report as left open.
