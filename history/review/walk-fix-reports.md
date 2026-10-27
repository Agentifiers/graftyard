# Implementability walk: writer reports (slices P, M, R, A)

Lead note: SliceR's WK-R14 text and its Milestone 6 SSH rotation gate row said `/meta` lists both host-key slots. The lead changed the design so `/api/v3/meta` stays GHES-shaped and the keys are served at `/api/graftyard/v1/meta/ssh`; writer B must use that path in the gate row.

## SliceP

All ten WK-P items (P1 to P10) are applied in my owned regions. `check_names` and `check_links` pass. `check_design` still fails on one cost line I did not touch, and it failed the same way when I reran it after a minute.

## Check output (final run)
```
design/DESIGN.md: OK
design/AGENT-LAYER.md: OK
agent block: OK
cost figures:
  design/AGENT-LAYER.md Cost has no row '| **Added total** | **$26** | **$303** | **$3,270** |' from scripts/cost_agents.py
  design/DESIGN.md Cost has no agent layer row ending in $3,270 from scripts/cost_agents.py
check_names: OK, 34 files
check_links: OK, 1233 citations
```
`scripts/cost_agents.py` now prints $3,270 at 2,000 developers, but both Cost sections still say $3,269. I changed no cost text and no script, so another writer's change is the likely cause (inferred). `scripts/__pycache__` is removed.

**Line counts:** `design/DESIGN.md` 1,699 of 1,800, `design/SHARED.md` 875, `design/AGENT-LAYER.md` 938 of 1,000. All three writers' edits are in these numbers.

## What went where, per item

- **WK-P1 (placement record and bootstrap).** In DESIGN Shape, "Sites and placement":
  - New `PlacementRecord { name, sites, home, generation, moving_from, at }` and `SystemChain`.
  - An owner's record is a `NamesEvent::Placement` row on `account_shard(owner)`. SliceM adds that variant, plus `RepoOwner` and `NameKey::Repo`.
  - A system chain's record is in the signed genesis as `Genesis.placements`. SliceR added that field.
  - A lookup rule: cached config first, then names built from the record, then the site's `MemberRegistry` (SliceR's), then `moving_from`.
  - `ReplyMeta`, and `bootstrap()`: the first entry is `Config(Normal(from_record))` at term 1, index 1, with deterministic bytes. A member with no committed config serves no read and grants no vote. It stores entry 1 only after it checks the record at its source itself.
  - `NotLeader` now carries `Option<(Index, GroupConfig)>`, in "The write path".
- **WK-P2 (inline packs).** In DESIGN Packs: the `Promoted` entry (also added to `EntryBody`), and the rule that `Archived` follows `Promoted`. `rehydrate` is defined, `Compacted.consumed` names only stored and promoted packs, and inline packs are verified through the entry digest chain. In "Git cache shards (derived)": `IngestCursor`. In SHARED "Refs and pushes": the Compaction bullet and a new promotion bullet.
- **WK-P3 (request ids).**
  - In DESIGN "Chains": `LogEntry.requests` becomes `(RequestId, Range<u32>)`.
  - In "The write path":
    - The edge mints one `RequestId` per client request.
    - A core-tier `request` table is kept for `REQUEST_RETENTION` (48 h [guess]).
    - `Command::replay` answers duplicates, and a duplicate still in flight joins the existing ticket.
    - Every ticket, refusals included, resolves only when its entry commits.
    - A hedge is only a `suspect` report.
  - Also updated: the "Commit and read paths" intro, journey 5, and the SHARED "Idempotent retries" bullet.
- **WK-P4 (credentials and deps).**
  - In "The write path": `CredentialShard` and `CredentialKey`, `fn credential()` built from the command's `Principal`, and `deps(&self, fold) -> SmallVec<[ChainId; 3]>` reading `RepoSettings { owner, enterprise }`. SliceM adds both fields to `RepoSettings`.
  - In SHARED: `Push.actor` becomes `Principal`. "Multi-chain operations" gets the updated Transfer bullet and a new Enterprise change bullet.
  - Journey 1, step 3 is updated.
- **WK-P5 (two-release rule for messages).** DESIGN write path: `Unsupported`, `CallError`, and `commit` returning `CallError`. The edge renders `Unsupported` as a 503 with `Retry-After: 5` [guess]. SHARED "Record formats" now covers messages and has a paragraph on them.
- **WK-P6 (snapshots).** In DESIGN Packs:
  - The writer is the Full follower at `to / SNAPSHOT_EVERY` mod the number of Full followers.
  - The cadence uses `SNAPSHOT_EVERY` and `SNAPSHOT_BYTES` [guess], plus at least one a day on a day the group commits.
  - Snapshots are taken at one index without pausing `apply`: rows carry the index that last wrote them, and changed rows are copied first into `snap_undo`.
  - The format is `SnapshotFrame` and `SnapshotTrailer`, written as multipart parts to two buckets.
  - A new `InstallSnapshot` step.
  - The SHARED "Folds" snapshot paragraph is rewritten to match.
- **WK-P7 (command size cap).** `COMMAND_MAX` is 1,200,000 bytes [guess], in "The write path". SHARED "Refs and pushes" gets a "Size" paragraph: a push that would go over the cap stores its pack instead of inlining it, and a push whose ref updates alone exceed the cap is refused whole with an `ng` line on every ref [guess wording]. Journey 1, step 2 is updated.
- **WK-P8 (committed-pack markers).** I chose the create-only marker, `R2Key::Committed { of }`, which also covers `Blob`. If the leader dies before writing a marker, the sweeper checks the object's `RequestId` against the chain's `request` table and writes the missing marker. `PACK_COMMIT_WINDOW` is 1 h [guess], so a pack that has not committed by then never will. The SHARED "Orphan sweep" bullet and journey 1 are updated. At SliceM's request, `Blob` and `ArchiveCache` were added to `R2Key`.
- **WK-P9 (round-trip check).** Measurements move out of `Member` into a per-member `Rtt` table. The check runs when a member joins, then at most once a day and only on a day the group served a request. This is in the placement paragraph and the Shape block.
- **WK-P10 (member names).** `do_name` is `{chain}#{generation}#{kind}{slot}`, and `Member` gains `slot` and `generation`. Any member added after bootstrap takes one above `Placement.generation`.

## Decisions beyond the item text
1. Changing an owner's record bumps its `generation` and keeps the old record in `moving_from`. A Workflow then moves each group through `Joint`.
2. Term 1 holds only the bootstrap entry. The first real leader is elected at term 2.
3. An entry whose commands were all refused is still appended, with no events.
4. A refused command leaves no `request` row, so a re-send is decided again.
5. `replay` reports a ref missing from the stored events as `Stale`.
6. A transfer to an owner with a different placement moves the group before `Live` commits.

## Gate rows
| Milestone | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| 1 | 1,000 chain creations with two edges at once, plus 100 during an owner record change | One group per chain. Members without a committed config serve no read and grant no vote | Any chain with two groups, or any read or vote served without a config |
| 1 | Edges with cold caches after a member retry, `ReplaceMember`, and an owner move | 100% of requests find the group | Any request that finds no group |
| 1 | Leader killed after quorum and before the reply, 10,000 times. The edge re-sends with the same `RequestId` | Original outcomes returned, zero double applies | Any double apply or changed outcome |
| 1 | Leader lost before commit, on entries with a `Stale` refusal caused by an earlier command in the same entry | Zero refusals returned from uncommitted entries (Porcupine) | Any |
| 1 | Receipts that miss `CALL_DEADLINE` | Zero commands forwarded to a second member | Any forwarded |
| 1 (3 for full identity) | Revoke a token, a `jti`, and an SSH key, and suspend an account, with 1,000 commands queued | 100% decided afterwards get 401 | Any accepted |
| 1 | Edge on N+1 with a new message format that is not enabled, plus one forced unknown variant | Nothing sent early. Unknown variant gives 503 with `Retry-After` and no state change | Any other rendering or any state change |
| 1 | Atomic push of 10,000 refs with a 1 MiB pack. A push whose updates exceed `COMMAND_MAX` | First: pack stored and push commits. Second: refused whole, no entry | Partial push, or an entry over 1.5 MB |
| 1 | Archive a range of inline packs, kill all shards, rehydrate. A second shard behind its `IngestCursor` | `git fsck` clean, every ref clones, second shard serves the newer pushes | Any missing object |
| 1 | 1,000 failed placements, plus 1,000 commits whose marker write is killed | Failed ones deleted after 24 h, zero committed ones deleted, missing markers repaired | Any committed pack deleted |
| 0 (B0-15) | 1,000 idle groups for 48 h | Zero duration billed, zero `Rtt` checks | Any wake |
| 1 | A learner on its voter's site, and two learners on one site | Distinct objects | A name collision |
| 2 | Snapshot of a 4 GB [guess] metadata tier under 300 commands/s | Apply p99 within 10% [guess]. Snapshot digest equals a refold at `to`. No request-limit errors | Otherwise |
| 3 | Follower offline past the archive boundary, then rejoins | Catches up by `InstallSnapshot` with matching fold digests | Stuck or divergent |
| 3 | Transfer across enterprises and placements, plus an org changing enterprise | Both routes reach the group throughout. `deps` names the new owner after `Settings` | Any lost route, or a stale owner after `Settings` |

## Open questions and text outside my regions
- **Untested values.** `COMMAND_MAX`, `REQUEST_RETENTION`, `PACK_COMMIT_WINDOW`, `SNAPSHOT_EVERY`, `SNAPSHOT_BYTES`, and `Retry-After: 5` are all [guess]. The write cost of the `snap_undo` copies is [unverified].
- **Ref-count wording.** GitHub's refusal text for too many refs in one push was not found. GitHub documents only an optional per-repository push policy.
- **Extra lookup.** A cold-cache route by `/repositories/{id}` costs one more `Names` read, through the `RepoOwner` row.
- **Stale text I don't own.** These still use the old wording and need their owners:
  - DESIGN Invariants, "Placement is checked by latency": still says "At join and daily".
  - DESIGN Usage call-site comment: "hedge to the other follower".
  - DESIGN Failure modes, `chain groups` provider-wide row: "hedged to both followers".
  - `build/BUILD-PLAN.md` M0 text: still cites `Member.rtt_ms`, which is gone.
  - `build/BUILD-PLAN.md` M1 text: still uses the `{chain}#{generation}` name form.
- **Install operations.** I sent SliceR the new name format for the "Placement failures" bullet.

## SliceM

I applied all nine WK-M items, plus four related items I describe below. `check_names` and `check_links` pass. `check_design` still fails on two cost figures outside my regions. I ran it again after a minute and got the same result.

## Per item

**WK-M1, merge queue.**
- DESIGN Shape, "Merge queue": `MergeQueueEntry` now names its queue ref, `refs/heads/gh-readonly-queue/{base}/pr-{n}-{parent_tip}`. Which SHA GitHub puts in that name is tagged [unverified].
- New `QueueEvent` with four variants: `Enqueued`, `GroupBuilt`, `Ejected`, `Landed`. Each says which webhook it renders (`merge_group.*`, `pull_request.enqueued` and `pull_request.dequeued`).
- New `EjectReason`.
- New `QueueOp` command with `Enqueue`, `Dequeue`, `Build`, and `Eject`.
- New `QueueMerge { plans, entry, actor }`. Its `decide` is all or nothing. It checks the `parent_tip` chain and the per-PR head and protection, and reads required checks on the queue refs under `AllGreen` or `HeadGreen`.
- The `MergeQueue` consumer is described in the same block: it builds groups, ejects failures, lands groups, and rebuilds after an outside base move.
- SHARED: `Consumer` gains `MergeQueue`. The "Merge queue" and "Other consumers" text now describe it.

**WK-M2, check runs.**
- SHARED `RepoEvent` gains `CheckRun`, `CheckSuite`, `Status`, and `TestMerged { n, base, head, result, pack }`. The first three are metadata tier and are pruned by `RepoSettings.check_retention_days`.
- `RequiredCheck` stays core. Its `conclusion` becomes `Option<CheckConclusion>`, so a re-run blocks a merge again.
- I removed both "off the chain" sentences. `record_check` now commits the run, with the `RequiredCheck` in the same entry when a rule requires it.
- The `rest:checks` and `rest:commits` rows are updated, and `refs/pull/N/merge` now moves by `TestMerged`.

**WK-M3, blob store.**
- SliceP added `R2Key::Blob { repo, oid }` and `R2Key::ArchiveCache` to "Packs" using my text, and its sweep markers cover `Blob` keys.
- SHARED: new `BlobRef` and `DurableBlob`, plus `Release`, `ReleaseAsset`, and `LfsObject` events with `ReleaseRecord` and `AssetRecord`.
- The `lfs`, `rest:releases`, `codeload-archives`, and `uploads-release-assets` rows are updated. Archive caches are derived and sit beside the clone cache.

**WK-M4, token fields.** `TokensEvent::Stored` gains the six optional fields: `app`, `client`, `permissions`, `resource_owner`, `repos`, and `paired`. Two more come from SliceA's WK-A4 request: `grant` and `write`. A doc comment says which token kinds set which fields. The `ghr_` secret is redrawn until it lands on its `ghu_`'s shard, so both rows commit in one entry.

**WK-M5, App records.**
- SHARED: new `ClientId`, `SecretHash`, `AppRecord`, `AppKey`, and `OAuthAppRecord`. `AuthzEvent` gains `App` and `OAuthApp`.
- DESIGN: `NameKey` gains `AppSlug` and `Client`. `NamesEvent` gains `AppSlug` and `ClientRegistered`, which are routes to the owner only. Registration order is stated: the slug claim is the commit point, and `Audit` finishes a registration nobody finished.
- "Who owns which fact" is updated.

**WK-M6, short-lived codes.**
- `TokensEvent::Code`, `CodeApproved`, `CodeUsed`, and `CodeKind` are TTL rows keyed by the code's hash. A device code is redrawn until it lands on its user code's shard, so both commit in one entry.
- Lifetimes: 900 s for a device code and 10 minutes for an authorization code, tagged [unverified for GHES 3.22]. The sign-in link keeps its 15 minutes [guess].
- `AuthzEvent::InstanceAuth` lives on the install's own `Authz` chain, the account that `Genesis` names (SliceR's field). Only a `SiteAdmin` writes it.

**WK-M7, collaborators and invitations.**
- New bullet "Collaborators and grants never overlap", applying the lead's rule: outside collaborators are `Collaborator` entries, org members and teams are `Grant` entries. `decide` refuses a `Collaborator` write for a current member.
- New `InvitationEvent`, `Invitee`, and `InvitationGrant`, with `Invitation` variants on `RepoEvent` and `AuthzEvent`. Accepting appends the grant in the same entry. The 7-day expiry is tagged [unverified].
- The `rest:collaborators` and `rest:orgs` rows are updated.

**WK-M8, credential write order.** DESIGN Identity now says the edge appends the `Identity` side right after the `Tokens` commit and before it answers. `Audit` appends it only if nobody did within a minute. "Event fan-out" says the same. SliceP rewrote the "Multi-chain operations" bullets to match.

**WK-M9, citations and stored formats.**
- Fixed citations: the "Reused with the additions below" list now points at "Event types", "Refs and pushes", "Record formats and the two-release rule", "Repository repair, tombstones, and retention", and "Event fan-out". The `Inbox` comment now cites "Known deviations from GitHub".
- I asked SliceR to change the "Deployment modes" paragraph's citation to "Release registry and signing".
- Defined as stored formats in SHARED "Issues and pull requests": `IssueCapture`, `CommentEvent` (with `CommentTarget` and `LineAnchor`), `ReviewEvent`, `LabelEvent`, `BranchRule` (with `Protection`, `RequiredChecks`, `RequiredReviews`, and `MergeQueueSettings` from GHES 3.22's `merge_queue` rule), `RepoSettings`, and `CheckRunUpdate` (with `CheckSuiteUpdate`, `StatusUpdate`, and `CheckConclusion`).
- **Writer B**: the Milestone 2 row must cite `design/SHARED.md`, "Issues and pull requests", not `history/DESIGN.md`.

## Decisions beyond the item text
- `IssueChanged` is new. It covers issue close and reopen, title and body edits, draft, and lock. It is needed for `gh issue close` at Milestone 2.
- `PrCapture` gains `issue: IssueCapture`, so a pull request's title and body have a home.
- `RepoSettings` is a whole record with `owner` and `enterprise`, as SliceP's WK-P4 needs. `NamesEvent::Placement`, `RepoOwner`, and `NameKey::Repo` are added for SliceP.
- From SliceA: `Credential::Session` now holds a new `WebSessionId`, and the `InstallationClaims.task` text is replaced.
- Commands carry `actor: Principal`, and `credential()` derives SliceP's `CredentialShard` from it.
- An exchange commits `CodeUsed` before the new token's `Stored`, so one code never yields two tokens.
- Two lead-approved edits outside my regions, in SHARED:
  - The residency bullet in "Public service concerns" drops "OAuth client secrets" and says the secret hashes live on `Authz`.
  - The Retention row for check runs now cites "Issues and pull requests".

## Gate rows (for writer B)
| Milestone | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| 2 | A group of 5 PRs under `HEADGREEN`, the 3rd failing its required check on its queue ref, 100 runs, with the base leader killed mid-landing in 10 | The base moves only by `QueueMerge` entries that land a whole group; the failing PR is ejected and the other 4 land after a rebuild; every base move is a fast-forward | Any partial group, any non-fast-forward base move, or a queued PR lost |
| 2 | `on: merge_group` workflow and go-playground/webhooks parsing `merge_group` and `pull_request.enqueued`/`dequeued` | A run on each built queue ref; every queue ref deleted within 5 min of landing or ejection [guess bound] | A missing delivery, or a queue ref left over |
| 2 | 10,000 non-required check runs and statuses on one repository, then entry time advanced past `check_retention_days` | `gh pr checks` and `GET /commits/{sha}/statuses` read them; core tier bytes unchanged; rows pruned on every member with equal `fold_digest` | A non-required row in the core tier, or members disagreeing |
| 2 | A required check passes, then is re-requested before the merge | The merge is refused until the new run concludes, 100/100 | A merge on the stale success |
| 2 | A head push, then `git ls-remote origin refs/pull/N/merge`; a conflicting push | The `TestMerged` commit within 10 s with a matching `mergeable`; on a conflict the ref is deleted and `mergeable: false` | A stale ref |
| 2 | `git lfs push` and `gh release upload`, then one site bucket made unreadable | `verify` and the 201 come only after two buckets hold the blob; fetch and download succeed 100/100 | Any acknowledged blob unreadable |
| 2 | All `ArchiveCache` objects deleted, then `gh release download --archive=tar.gz` | The bytes match the earlier download | Any difference or error |
| 3 | A `github_pat_` scoped to one repository with `contents: read`; a `ghu_`/`ghr_` refresh; `DELETE /applications/{client_id}/token` | The PAT is refused outside its scope; revoking either paired token refuses the other; the DELETE revokes | Any use after revocation |
| 3 | `@octokit/app` installation-token flow with a newly registered App; OAuth web flow with a registered OAuth app and a wrong secret | The JWT verifies against `AppRecord.keys`; the wrong secret gets 401; a dump of every `Identity` chain shows no client secret | Any other result |
| 3 | `gh auth login` device flow with the code shard's leader killed mid-flow, 100 runs; replay of a used code; a code older than 900 s | Each login completes or restarts cleanly; replay and the expired code fail 100/100 | A token from a reused or expired code |
| 3 | `TestAccGithubRepositoryCollaborator`; an org invitation accepted and one declined; a `Collaborator` write for an org member | Accepting grants in the same entry; declining grants nothing; the member write is refused | Any overlap or lost grant |
| 3 | A new SSH key and PAT, listed at once with no `Audit` consumer deployed | `gh ssh-key list` and `GET /user/keys` show them 100/100 | Any listing that misses them |
| 3 | A site admin switches `InstanceAuth` from Builtin to OIDC | The next sign-in uses OIDC; a non-admin write of it is refused | Any other result |

Also for writer B: `build/BUILD-PLAN.md` line 31 and M2-WP3 need `MergeQueue`, `QueueOp`, `QueueMerge`, and `QueueEvent`. The `MergeQueue` consumer belongs in Milestone 2 with the Queue plumbing (WK-B5).

## Open questions
- `GET /orgs/{org}/outside_collaborators` has no index across an org's repositories.
- These payload types are still undefined: `DeployKeyEvent`, `LfsLockEvent`, `TakedownEvent`, `TeamEvent`, `OrgPolicy`, `AccountCapture`, `CredentialEvent`, `SsoEvent`, `AuditEvent`, milestones, and reactions.
- The agent block's `IssueEdited` and `Assigned` overlap the new base `IssueChanged`.
- Which SHA GitHub puts in the `gh-readonly-queue` ref name is unverified.

## Line counts and checks
- `design/DESIGN.md`: 1,699 lines (cap 1,800).
- `design/AGENT-LAYER.md`: 938 lines (cap 1,000).
- `design/SHARED.md`: 875 lines.
- `check_design`: DESIGN, AGENT-LAYER, and the agent block are OK. Cost figures fail: the agent "Added total" row reads $3,269, but `scripts/cost_agents.py` prints $3,270. The same mismatch shows in DESIGN's agent layer row. Neither the cost sections nor the script is in my regions.
- `check_names`: OK, 34 files.
- `check_links`: OK, 1,233 citations.
- `scripts/__pycache__` removed.

## SliceR

All 14 WK-R items are applied in `design/DESIGN.md`. The lead decisions for WK-R3 and WK-R6 are followed. On the final run all three checks pass and `scripts/__pycache__` is removed.

## Items

**WK-R1 (progress entries and the roll order).**
- Shape, "Release group":
  - `ReleaseEntry` gains `IntentStep { intent, step }`, `IntentDone { intent }`, and `SiteRelease { site, release }`.
  - `Keys` gains `executor`, held by `graft-ops`, plus a signer list.
  - A new fold type, `IntentState { Running, Halted, Undoing, Aborting { target } }`.
  - The `Intent` doc says `graft-ops` commits a step only after its gate passes, and that every step can safely run twice.
- Install operations, new "Release roll" bullet. The order is: member sites (one a day), then spares and residency sites, then `SiteDay { Workflows }`, then `EdgeRoll`, `OpsRoll`, `FrontRoll`, `McpRoll`, then `ContainerDay` per site. It also says which undos each intent carries.

**WK-R2 (uploaded releases and the missing types).**
- New `ReleaseUploaded { release, record, record_sig, versions: SmallVec<[(Script, VersionId); 16]>, images, reads: FormatSet }`, plus `enum Script`.
- Defined `Genesis`, `InstallId`, `VersionId`, `ReleaseId`, `Format`, `FormatSet`, `Scope`, `SecretName`, and `ImageRef`.
- Every intent now names a `ReleaseId`.
- `reads` comes from the bundle's formats file, whose hash the signed record already lists. So `design/SHARED.md` did not need an edit.

**WK-R3 (pointers are pin-only).**
- New fold type `Pointers { edge, ops, mcp, controller: Option<VersionId> }`.
- Each pointer entry is `EdgePointer { pin: ReleaseId }` (and the same for ops, MCP, and controller), accepted only as a pre-signed undo.
- Sent to SliceA, who confirmed they are using it.

**WK-R4 (genesis and the trust anchor).**
- The Genesis bullet is now five steps: `init`, `sites add`, `genesis` (creates the remaining scripts and the drain and executor keys), `sign` offline, then `install --genesis`.
- The trust anchor is two script variables, `GRAFTYARD_INSTALL` and `GRAFTYARD_CUSTOMER_KEY`.
- The Usage operator block is reordered to match.
- `Genesis` carries SliceP's `placements` field and `account`, whose `Authz` chain SliceA and SliceM cite.

**WK-R5 (`prev` and the submit endpoint).**
- `Bound.prev` is defined as the digest of the last committed signed entry, not counting `Noop` and `Config`.
- Added `POST /api/graftyard/v1/release/entries` with 201, 409 (returns the current digest), and 422 bodies.
- The GET side is SliceA's, cited as `design/AGENT-LAYER.md`, "API reference".

**WK-R6 (Abort carries a target).**
- `Abort { intent, target: ReleaseId }` is refused and pages when `target` cannot read an enabled format.
- Updated the Shape doc, the revocation runbook, and the `release control` bad-config row.

**WK-R7 (site loss).**
- `ReplaceMember { lost, spare }` is signed only by the executor key. `decide` accepts it after a customer-signed `SiteLost`, or when the site has been drained for 24 h with zero acks in both other sites' `SiteHealth`.
- `RestorePlacement { site }` takes an intent key and is refused while the site is drained.
- Enumeration source: a `MemberRegistry` per site script (with `RegistryRow`), not the `Release` group. A member writes its row before it stores its first entry. A new "Site loss" bullet also defines when a site counts as recovered.

**WK-R8 (export and lifeboat endpoints).**
- Shape, "Lifeboat feed":
  - `ExportObject` holds entry bytes, so batches carry them.
  - Batches live in a single export bucket in site A's region with a 7-day lifecycle [guess].
  - Four export endpoints: batches, chain entries from an index, snapshots, and packs. Also the chain-id path encoding and the error codes.
  - `ExportClaims` for the export token, revoked by `jti`.
  - The `lifeboat:read` token is stored as a PAT-format token. Minting goes through `POST /api/graftyard/v1/lifeboat/tokens`, and `LifeboatDecoder` is sent through the release-entries POST.
- Updated the Vault and Lifeboat bullets.

**WK-R9 (undo validity).** An undo is accepted only if its bytes match the parent's committed `undo` list, checked against the `Keys` at the parent's index. After `Keys { vendor: None }`, `Resume { intent, undo }` must carry undos re-signed by the customer.

**WK-R10 (gate constants, undrain, cache).**
- A table of 18 compiled constants in Install operations, Site day, all [guess].
- The `SiteMode` doc gives the undrain rule.
- Isolates, members included, now cache the whole `Release` view for `POINTER_TTL`.

**WK-R11 (rotation).** The rotation runbook is now a table of 11 rows, including the App and agents-account rows SliceA asked for. It uses two Secrets Store slots per secret. `Rotate { secret, slot }` is signed by an intent key. The customer or a site admin writes new values with their own Wrangler login.

**WK-R12 (Workflows script and vault).** `SiteDay { target: DayTarget::Workflows }` rolls the Workflows script. The artifact paragraph now lists the Workflows script and the vault Worker, and adds `graftyard vault upgrade`. Its `design/SHARED.md` citation now points at "Release registry and signing", as SliceM asked. The `vault` bad-deploy row is updated.

**WK-R13 (StaleMode).** `StaleMode` takes the customer key. An uncommitted flag is checked against cached `Keys` and spread through the `Release` members' `pending` rows, up to `STALE_MAX`. `Scope` is defined.

**WK-R14 (SSH host keys).** Shape, "SSH front": the keys live only in Secrets Store and reach containers through the outbound handler at `keys.graft`. New `SshHostKeys` type, a rotation row, and the keys are published in `/api/v3/meta`.

## Decisions beyond the item text
1. The next intent that rolls the same script clears its pin, not every next intent. Clearing on any intent would put a bad edge back on its split while the API is down.
2. The group accepts only one current intent at a time. The exception is `ReplaceMember`, which halts the current one. Because each signature binds the latest digest, `release roll` signs one intent at a time and is rerun after each finishes.
3. `Rollback { target, release }` is a new undo-only entry. `FrontRoll` and `ContainerDay` carry no undo, because `graft-ops` returns them to N through the API.
4. The `percent` field is removed from the roll intents. Steps come from `ROLL_STEPS` instead.
5. The executor key also signs `ReleaseUploaded` and `ReplaceMember`, which WK-R2 and WK-R7 require. That widens WK-R1's "these three entries and nothing else".
6. `Keys` gains `agents_executor` at SliceA's request.
7. `graftyard init` mints the install id and creates nothing. `graftyard genesis` creates the non-site scripts, so each one binds every site.
8. `/api/v3/meta` adds github.com's `ssh_keys` and `ssh_key_fingerprints` fields, which GHES 3.22's `/meta` does not have.

## Edits outside my regions (Main approved them)
- Usage call sites: the hedge comment, and the `commit` comment now says it returns `CallError`.
- Shape, "The read path": the `stale()` doc now allows an uncommitted flag.
- Invariants: "Placement is checked by latency" now follows WK-P9.
- Journey 7: the "A site day" and "Record shape the previous release cannot read" bullets use `Rollback` and `Abort { intent, target }`.
- Failure modes: the `chain groups` provider-wide row (hedge wording) and bad-deploy row (`Rollback`).
- Install operations, "Placement failures": the new member name form, as SliceP sent it.
- I left the "Review resolutions" entries alone, because they record past decisions.

## Gate rows (for writer B)
| Milestone | Measurement | Pass | Fail |
|---|---|---|---|
| 5 | Kill `graft-ops` at each step of a `SiteDay` and an `EdgeRoll`, 100 times | It resumes after the last `IntentStep`; no step skipped; one `IntentDone` | A skipped or doubled step |
| 5 | Submit a second intent while one is current, and an intent naming a release with no `ReleaseUploaded` | Both refused | Either committed |
| 5 | Two genesis entries sent to an empty `Release` group: one matching the anchor, one signed by another key | Only the matching one commits | The other commits |
| 5 | Sign an entry offline while `graft-ops` commits an `IntentStep` | 409 with the current digest; the re-signed entry commits | A stale `prev` commits |
| 5 | `EdgePointer` undo under 1% `EdgeRoll` with the API blocked, then the next `EdgeRoll` | Every request pins to N within `POINTER_TTL`; the next `EdgeRoll` sets `None` | Any request off N while pinned, or the pin survives |
| 5 | A vendor-signed undo not in the parent's `undo` list, and a `Resume` after `Keys { vendor: None }` without customer re-signed undos | Both refused | Either accepted |
| 5 | A forged `ReleaseUploaded`, `ReplaceMember`, or pointer signed by the executor key, for a site not drained 24 h | Only a correctly signed `ReleaseUploaded` commits | A pin, or a `ReplaceMember` of an undrained site |
| 5 | `Rotate` of each `SecretName` with live traffic | No new script version; zero failed calls; old value unused after the switch | A new version, or failures |
| 5 | `release roll` end to end, including `SiteDay { Workflows }` and `graftyard vault upgrade` | Order as in Release roll; vault upgraded with no install token | Any other order, or an install token used |
| 5 | Blackhole the `Release` group, then send a customer-signed `StaleMode` | Scoped groups serve `stale()` within `POINTER_TTL`; the flag expires at `until` | No stale reads, or a flag past `STALE_MAX` honored |
| 6 | Lose site A with every API token revoked; drain it 24 h; then recover it | Executor-signed `ReplaceMember` moves every group with a voter on A, using B's and C's registries; `RestorePlacement` refused until undrained | A group left out, or a restore while drained |
| 6 | Rotate SSH host keys | `UpdateHostKeys` clients learn the new set; `/meta` lists both slots for the overlap; same fingerprints on every replica | A mismatch, or a host-key error for an updated client |
| 6 | Lifeboat enroll, a daily `LifeboatDecoder`, and tail and pack pulls with the R2 feed stopped | Token refused by the main edge; no 7-day alarm; tail and packs fetched from followers | Any leader read, or a main-edge accept |
| 5 | Vault pulls every export endpoint; then the export `jti` is revoked | All four endpoints served; refused within `READ_CACHE` after revocation | Any served after `READ_CACHE` |

**BF-7 and BM-5 wording (WK-R6):**
- BF-7: `After FormatsEnable, submit the pre-signed Rollback to N and Abort { intent, target: N } | Both refused, and both page | Either applied`
- BM-5, change only the second sentence: "With the vendor token revoked mid-roll, `Halt`, `Keys { vendor: None }`, then `Abort { intent, target: N }` returns every site and pinned script to N, and a vendor-signed intent submitted after `Keys { vendor: None }` is refused."

## Open questions
- Does a Tunnel token rotation keep the running connectors? [unverified]
- Can a Workflows script be rolled by gradual percentage without touching running instances? [unverified]
- Do OpenSSH clients pick up new host keys through `hostkeys-00@openssh.com`, as the SSH rotation assumes? [inference]
- Should the two extra `/api/v3/meta` fields be checked by the GHES byte-diff gate?
- Every constant I added is [guess]: the gate and dwell table, `STALE_MAX`, the 64 registry shards, the export page size, the export token's 400-day expiry, and the 7-day export lifecycle.

## Stale text outside `design/DESIGN.md`, for writer B
- `README.md` glossary, "pointer" row: should describe a pin set by an undo.
- `build/BUILD-PLAN.md` M5-WP1 and M5-WP2 rows: should name `ReleaseUploaded`, `IntentStep`, `IntentDone`, `SiteRelease`, `Pointers`, `Rollback`, and `Keys.executor`.
- `build/BUILD-PLAN.md` M6-WP1 row: should name `MemberRegistry`.
- `build/BUILD-PLAN.md`, the `graft-ops` component row: should mention the executor key.

## Line counts and checks
`design/DESIGN.md` 1,754 of 1,800, `design/AGENT-LAYER.md` 955 of 1,000, `design/SHARED.md` 875. These include the other writers' edits.
```
design/DESIGN.md: OK
design/AGENT-LAYER.md: OK
agent block: OK
cost figures: OK
check_names: OK, 34 files
check_links: OK, 1241 citations
```

## SliceA

I applied all seven slice A items. All three checks pass and the agent block is still byte-identical in both files. One thing to know first: adding the agents-account front Worker changed the cost model. The 2,000-developer agent total went from $3,269 to $3,270, so I edited cost figures in four files outside my regions. Details are under WK-A1.

## Per item

**WK-A1, controller front and `Release` reads** (measured: written and checked)
- **AGENT-LAYER Shape, controller block.** It adds `AgentsAnchor`: the install id, the customer key and the host, set as script variables by `graftyard agents init`.
- **`AgentsReleaseView`.** It is built from `GET /api/graftyard/v1/release/entries?after=` and checked against that anchor. Isolates cache it for `POINTER_TTL`, never move to a lower index, and keep the last value when a refresh fails.
- **`forward()` for `graft-agents-front`.** It sets a version override only while `Pointers.controller` is `Some(v)`. This follows SliceR's pin-only rule and its types: `ControllerPointer { pin: ReleaseId }`, `ControllerRoll { release }`, `Keys.agents_executor`, and `Script::AgentsFront` and `Script::Controller`.
- **The release comment in the graft-mcp block** now uses `Pointers.mcp` and `Pointers.controller`.
- **Other sections touched:** A3 step 2, A8, the GHES Controller row, the controller Failure-modes rows (description, instance, control-plane, bad-deploy), the controller Deployment-modes row, and Open questions.
- **Cost.** `scripts/cost_agents.py` now counts each webhook twice, because it runs `graft-agents-front` and then `graft-controller`. It also adds a line "Install: `Release` reads by the agents account", at most one read per webhook (`RELEASE_READS_PER_WEBHOOK` [guess]). Running it gives $26, $303 and $3,270. `--a1` is unchanged.
- **Files I edited outside my regions for that $1:**
  - the DESIGN Cost row and paragraph, which `check_design.py` requires;
  - `README.md`;
  - `alternatives/AWS.md`, where $12,686 becomes $12,687;
  - `research/costs.md`, regenerated by `costs_report.py`.
- If you'd rather not have that spread, revert those four files together with the script change.

**WK-A2, task claim and manifests**
- **Agent block, both files.** The claim is valid when the token's App is the task's App or `graftyard-workspace`, and its repositories are the task's repository or that repository's workspace fork.
- **Session refs.** Only `graftyard-workspace` may append under `refs/graftyard/`. It used to say "the controller App".
- **New "First-party Apps" table** with each App's permissions and events. A3 steps 3, 5, 7, 8 and 9 are rewritten to match.
- SliceM applied the matching `InstallationClaims` text in SHARED.

**WK-A3, which init registers which Apps.** Covered by the "First-party Apps" paragraph, A8, the rewritten "Enable agents" steps 1 to 9, and the controller bad-deploy row. The agents account no longer holds the `Graftyard MCP` or `graftyard-insights` keys.

**WK-A4, MCP grants.** `McpGrant` doc, a GHES row for the scoped-token fields, and API reference entries for `/mcp/grants`. The agent block now names `graftyard_grant` and `graftyard_write`. SliceM added `grant` and `write` to `TokensEvent::Stored`.

**WK-A5, API reference**
- **New subsection "### API reference"** under GHES compatibility, as a YAML block. It covers every agent-layer endpoint with bodies, snake_case fields, `Link` pagination, error bodies, and the 409, 410 and 422 cases.
- **It also defines** the `graftyard_task` delivery (header, signing secret, payload) and the `graftyard-plan` grammar.
- **The feed block** defines the cursor encoding, `wait` (whole seconds, clamped at 25) and `paths` as a repeated parameter.
- **The agent block** adds `enum SessionId { Run(Uuid), External { app, name } }`.
- **Name clash.** SHARED already used `SessionId` for web sign-in sessions. SliceM renamed that one to `WebSessionId`.

**WK-A6, install-wide settings**
- The agent block puts `AgentApp` rows on `Authz(Genesis.account)`. Every App command lists that chain in `deps()`.
- `OrgPolicy` gains `agents_workspace: WorkspaceMode { Fork, InRepo }`.
- An uninstalled App's tasks read as cancelled through the same check that releases its leases, so nothing sweeps them.
- Settings endpoints are in the API reference, there is a new Scale row for the install `Authz` leader, and the "Leases" paragraph is updated.

**WK-A7, small gaps**
- `HIDDEN_CHARS` lists the dropped characters. It keeps the emoji selectors U+FE00 to U+FE0F [inference].
- `graftyard_trust` names the resources that get it.
- `SessionLog` and A3 step 7: in fork mode the session URL names the base repository.

Review resolutions now has a "Walk pass" entry with WK-A1 to WK-A7.

## Decisions beyond the item text
1. **Fork creation.** The host creates the workspace fork through `POST /api/graftyard/v1/repos/{owner}/{repo}/workspace`, which only `graftyard-workspace` may call. That way no App needs the `administration` permission.
2. **Who pushes.** Only `graftyard-workspace` pushes, in fork mode and in-repo mode alike. `graftyard-agent` keeps `contents: read`, and is also installed on `<org>-agents` so it can open PRs from the fork.
3. **Auth for `Release` reads.** The read endpoint takes an App JWT of `graftyard-agent`. It returns only the entries the agents account needs.
4. **Webhook retries.** The `graftyard-agent` hook opts into the base's automatic retries.

## Gate rows for writer B
| Milestone | Measurement | Pass | Fail |
| --- | --- | --- | --- |
| AM2 | `graftyard apps init` on an install with no agents account, then the MCP and A5 insights flows | MCP and overlap check runs work. No `Graftyard MCP` or `graftyard-insights` secret exists in any agents account | Either flow needs the agents account |
| AM2 | `token/scoped` from `Graftyard MCP` without `graftyard_grant`, with another user's grant, and with `graftyard_write` above the grant. A stale `generation` on `POST /mcp/grants/{id}/generation`. Grant calls with wrong client credentials | 422 for each bad token request. 409 and the grant revoked for the stale generation. 401 for wrong credentials, 100 of 100 | Any accepted |
| AM3 | Bad `graft-controller` at 1% with the deployments API blocked. Submit the pre-signed `ControllerPointer` undo, then blackhole `GET release/entries` for 10 minutes | Every `graft-agents-front` isolate pins N within `POINTER_TTL` of its next request. Webhooks keep flowing on the last view | Any webhook reaches N+1 after the pin, or blocks on the read |
| AM3 | `release/entries` with an entry signed for another install id, a `Keys` entry not signed by the anchored customer key, and an entry out of index order | All refused by the agents view, 100 of 100 | Any accepted |
| AM3 | Token ceilings by kind: a `graftyard-agent` token pushes to the base and the fork. A `graftyard-workspace` token pushes to the base in fork mode. A `graftyard_task` request from a third App, or for a repository that is neither the base nor its fork | 100 of 100 refused for each | Any accepted |
| AM3 | Fork-mode session URL in a trailer, read by a base reader with no access to the fork | The page and JSON serve the log from the fork | 404, or the URL names the fork |
| AM1 | Uninstall an App with 100 open tasks across 10 repositories | Every task reads `cancelled` at the next read with no sweep command, and its next command records it | Any task reads live |
| AM1 | Unflag an `AgentApp` (`DELETE /agents/apps/{id}`), then push under its old `confine` prefix | The next `decide` after the acknowledgment no longer classes it `ActorKind::Agent` | A decision after the acknowledgment uses the old row |
| AM1 | Feed cursor from another group, `wait=40`, 33 `paths` | 410 with `oldest_cursor`, `wait` clamped to 25, 422 for the 33 paths | Otherwise |
| AM1 | Trust view: one payload per `HIDDEN_CHARS` range in each `graftyard_trust` resource | 100% absent from `visible` and counted | Any present |
| AM1 | `graftyard-plan` blocks: a valid one, a repeated key, a bad glob, a second block, and a block over 4 KiB | One lease from the valid block. None from the others, and the body unchanged | Otherwise |

The AM2 and AM3 Built cells also need to change: AM2 adds `graftyard apps init`. AM3 adds `graft-agents-front`, the `release/entries` GET, the workspace endpoint, and `Keys.agents_executor`.

## Open questions (now in AGENT-LAYER, Open questions)
- Does a `fetch()` version override on `graft-controller` also pin the Workflow instances it starts? [unverified] If not, swap-back covers only new webhook handling.
- Does a changed Secrets Store value reach a bound script without a new script version? [inference] `graftyard apps init` and key rotation in the site scripts rely on it.
- Can `graftyard-agent` open a PR from the `<org>-agents` fork with only `contents: read` installed on the fork? [unverified]

## Line counts and check output
- `design/DESIGN.md`: 1,752 lines (cap 1,800). `design/AGENT-LAYER.md`: 955 lines (cap 1,000).
- `check_design.py`: DESIGN OK, AGENT-LAYER OK, agent block OK, cost figures OK.
- `check_names.py`: OK, 34 files. `check_links.py`: OK, 1,241 citations.
- `scripts/__pycache__` removed.

## Not changed
`design/DESIGN.md` "Review resolutions" has no walk-pass line for slice A, because no slice owns that region. Add one if you want it.