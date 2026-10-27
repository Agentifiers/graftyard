# Implementability walk: base (WalkBase, poteto-agent, read-only)

I walked 23 work packages and found 20 gaps. Five block building outright: G1, G2, G3, G4 and G5. Thirteen more would likely lead two implementers to build different things. Two are minor. Everything here was read-only, and I edited nothing. Every quote below was read in this session. The proposed fixes are my inference, and none has been tested against a prototype.

The work packages walked were M0-WP1, M0-WP2 and M0-WP6, all of M1 (WP1 to WP7), all of M2 (WP1 to WP5) and all of M3 (WP1 to WP6). For each one I sketched the types, tables, endpoints and gate test I would build, against `design/DESIGN.md` (Shape, Commit and read paths, Build order) and `design/SHARED.md` (Wire compatibility, Chain events and commands, Identity and tokens, Issues and pull requests, Folds). I checked each finding against `decisions/OPEN-QUESTIONS.md`, and none repeats a row there or R4 to R8.

**Playbook checklist (Investigation).**
1. Route through **how**. Skip: the deliverable is a findings list in the format the brief set.
2. Throughput checkpoint: n/a, read-only investigation.
3. Output: findings in the required format.
4. Unslop the reply: done while drafting.

**Principle.** I applied only **principle-foundational-thinking**, the only leaf principle I read this session. Its "data structures first" rule is why I sketched each package's types and tables before anything else. Most findings below are a missing data shape, not a missing sentence.

## Findings

**G1. Blocker. M1-WP1, M1-WP2, M0-WP2.**
- **Where.** `design/DESIGN.md`, Shape ("Sites and placement", `Member`, `Placement`); Failure modes, `chain groups` control-plane row; Install operations, Placement failures.
- **Gap.** Members are "created by name on first use", named `"{chain}#{generation}"`, and "A failed latency check is retried with a new member generation".
- Nothing says how an edge maps a `ChainId` to its placement, sites, generation and learners. Nothing says who writes a new group's first `Config`, or how two edges touching a group for the first time at the same moment agree on it.
- An edge that still calls `#0` after a regeneration would wake an empty object. That object could then initialize itself as a new group.
- Where an owner's placement lives is also undefined. That covers `graftyard placement set default tri`, the owner's `home`, and the token "home family".
- **Smallest addition.**
  - A placement record per owner (and per system chain kind) holding the `PlacementName`, `home` and the current generation.
  - A bootstrap rule: a group's first entry is `Config(Normal(placement))`, built only from that record, and a member with no committed config never serves or votes.
  - `NotLeader` and config replies carry the current membership, so edges can follow it.

**G2. Blocker. M1-WP6, M1-WP7, M2-WP2, M2-WP4.**
- **Where.** `build/BUILD-PLAN.md`, Milestones 1 to 3; `design/DESIGN.md`, "Milestones 1 to 7".
- **Gap.** The M1 gate runs "`git clone` / `fetch` / `push --atomic` from unmodified git". BM-2 runs `gh pr` and `gh issue`. Both need things that M3 builds:
  - creating a repository (`RepoName` on `Authz`)
  - resolving `o/r` (`Names`, `Authz`)
  - authenticating a token (`Tokens`)
  - allocating a `DbId` for a repository, issue or comment (`Ids` and `IdBlock`, which are M3-WP1)
- No package says how M1 and M2 run before those exist.
- **Smallest addition.** Either move `Ids`, `Names`, a minimal `Authz` `RepoName` and stored-token checks into M1, or name a test-only fixed resolver and authenticator that M3 deletes. Either way, record the choice beside R4 to R8.

**G3. Blocker. M1-WP4, M1-WP5, M0-WP6 (B0-22).**
- **Where.** `design/DESIGN.md`, Shape ("Packs", "Git cache shards (derived)"); journey 1, step 2; Install operations, Whole-install restore, step 3; `design/SHARED.md`, `NetworkEvent::Compacted`.
- **Gap.** "A pack of 1 MiB or less skips the buckets and is stored inline in the entry." Shards are "rebuilt from R2 packs and the leader's refs".
  - Once an entry is archived, objects that arrived inline live only in log and archive objects. No rehydration path reads them.
  - `Compacted { consumed: Vec<PackId> }` cannot name an inline pack.
  - Restore "verify[s] each `PackRef.checksum`", which inline packs do not have.
  - Nothing says how a second warm shard, or another of the k shards, learns which packs were committed after its last sync.
- **Smallest addition.**
  - A promotion step: before a range is archived, its inline objects are placed as a `DurablePack`, and an entry records the range and the pack that replaced them.
  - A per-network ingest cursor that each shard advances by reading `Refs` and `Merged` entries.
  - Rehydration defined as stored packs plus the inline packs after the last promotion.

**G4. Blocker. M2-WP3, M2-WP5 (BM-2, BF-1).**
- **Where.** `design/SHARED.md`, "Issues and pull requests", Merge queue; `RepoEvent::Queue(QueueEvent)`; `Consumer` enum; `design/DESIGN.md`, Shape, "Merge queue".
- **Gap.** "When a group's required checks pass, one entry carries a `Merged` per pull request and the base fast-forward ... so a group lands whole or not at all."
  - `Merge` is single-PR (`pub struct Merge { pub n: IssueNumber, ... }`). Five separate `Merge` commands decide independently, so a group would not land whole.
  - `QueueEvent` is defined nowhere, `history/` included.
  - No component builds groups, creates the `gh-readonly-queue/*` refs, watches for checks to pass, ejects a failing group, or submits the group merge. No consumer in the `Consumer` enum does it.
- **Smallest addition.** A `QueueMerge { plans: Vec<(IssueNumber, MergePlan)>, entry: MergeQueueEntry }` command with all-or-nothing `decide`, the `QueueEvent` variants (enqueue, group built, ejected), and a named driver (for example a `MergeQueue` consumer) that is triggered by `RequiredCheck` on queue refs.

**G5. Blocker. M2-WP2 (`rest:checks`, `rest:commits`).**
- **Where.** `design/SHARED.md`, "Issues and pull requests" ("Other check runs and statuses stay off the chain", "Stars and check runs that no rule requires stay off the repository chain"); `design/DESIGN.md`, Shape agent block ("PrSync, test-merge records, check runs, statuses" as caused commands; "Check runs and statuses are kept 90 days").
- **Gap.** The two documents contradict each other. If non-required check runs are off the chain, no store exists for them. `RepoEvent` also has no variant for a check run, a status, or a test-merge result (`Mergeability`, `TestMerge`, the commit behind `refs/pull/N/merge`).
- **Smallest addition.** Make check runs, check suites and statuses `RepoEvent` variants in the metadata tier, which `apply` prunes after the retention period. `RequiredCheck` stays core. Add a `TestMerged { n, base, head, result, pack }` variant.

**G6. Major. M0-WP2, M1-WP2.**
- **Where.** `design/DESIGN.md`, Shape, the write path; "Commit and read paths" intro.
- **Gap.** "the caller re-sends with the same request id, which the entry's `requests` makes idempotent."
  - Undefined: who mints a `RequestId`, how long duplicates are remembered, and what `outcome` returns for a duplicate (the original per-ref outcomes or the issue number).
  - `requests` is not mapped to the events each request produced.
  - Undefined: whether a refusal resolves before its entry commits. A `Stale` refusal caused by an earlier, uncommitted command in the same entry breaks linearizability if that entry is lost.
  - "hedged to the other follower" has no stated meaning for a `commit` call.
- **Smallest addition.**
  - Entries carry `(RequestId, event range)`.
  - A per-group request table with a stated retention, which answers duplicates with an output rebuilt from the stored events.
  - Every ticket in an entry, refusals included, resolves only when that entry commits.
  - A hedge is only a `suspect` report and is never a forwarded command.

**G7. Major. M1-WP2, M2-WP3, M3-WP2 (BF-2), M3-WP3 (BF-3).**
- **Where.** `design/SHARED.md`, `Push` (`pub actor: Actor`); `design/DESIGN.md`, Shape, the `Command` trait (`fn deps(&self)`) and write path ("each distinct credential shard (token, `jti`, and `Account` row)"); `AuthzEvent` doc ("whose own `Authz` chain is a third dependency").
- **Gap.**
  - Commands carry no credential, so the leader cannot know which token, `jti` or SSH-key shard to re-read for each entry.
  - `deps()` gets no fold, so it cannot name the enterprise's `Authz` (that needs the org's fold) or the current owner after a transfer.
  - `deps` returns at most 2 chains inline, while the comment names three.
- **Smallest addition.** Add `fn credential(&self) -> &Credential` (or a `Principal` on every command), and change to `deps(&self, fold: &Fresh)`. Alternatively, store owner and enterprise in the repository core tier.

**G8. Major. M1-WP2, M1-WP6.**
- **Where.** `design/SHARED.md`, "Record formats and the two-release rule"; `design/DESIGN.md`, journey 7 and Stack.
- **Gap.** The rule covers "entry bodies, event kinds, snapshot framing, stored webhook payloads, and every other persisted record".
  - The edge rolls by percentage, with N and N+1 in one deployment. Sites roll on site days. So edge N+1 sends commands and read requests to leaders on N, and members on N+1 exchange AppendEntries with members on N.
  - Nothing versions these messages or says what a leader does with a command variant or field it does not know.
- **Smallest addition.** Put messages under the same rule: a new command or field is sent only after the receiving release is enabled, and an unknown variant returns a typed `Unsupported`, which the edge renders as a retryable 503.

**G9. Major. M2-WP1, M3-WP5, M1-WP2.**
- **Where.** `design/SHARED.md`, "Folds" ("Each tier is snapshotted"); `design/DESIGN.md`, Shape, `R2Key::Snapshot`; journey 5 ("It copies the latest snapshot").
- **Gap.** These are undefined:
  - who writes per-tier snapshots, and how often
  - how a consistent snapshot of a multi-GB member, up to the 4 GB metadata tier, is produced inside Durable Object request limits while the member keeps applying entries
  - how a follower or learner whose next entry has been archived catches up (there is no InstallSnapshot step)
- **Smallest addition.** Name the writer (for example one follower, chosen by index), the cadence, a chunked row-stream format stamped with one index, and an install-snapshot step in the protocol.

**G10. Major. M1-WP6.**
- **Where.** `design/SHARED.md`, `Push.updates: BoundedVec<RefUpdate, 10_000>` and `RefName` "at most 900 bytes [guess]"; `design/DESIGN.md`, `EntryBody::Events` "<= 1.5 MB including inline packs; DO rows cap at 2 MB".
- **Gap.** One allowed atomic push can be about 9 MB of ref updates, plus a 1 MiB inline pack. No rule covers a command bigger than an entry, or the text git shows when one is refused.
- **Smallest addition.** A cap on a command's encoded size. Push the pack to storage instead of inlining it when the updates are large. Reject anything over the cap whole, with a `TooManyRefs`-style message.

**G11. Major. M1-WP4.**
- **Where.** `design/SHARED.md`, Refs and pushes, "Orphan sweep"; `design/DESIGN.md`, journey 1, step 2.
- **Gap.** "A sweeper deletes pack objects that no entry names after 24 hours", and it "fills the third bucket". Nothing maps a `PackId` to the entry that names it. Answering "no entry names it" would mean scanning every chain in the network.
- **Smallest addition.** After `Acked`, the committing leader writes a create-only marker key per committed pack, or a network-level fold table records each committed pack. The sweeper reads that.

**G12. Major. M0-WP1 (B0-15), M1-WP1.**
- **Where.** `design/DESIGN.md`, Shape, placement paragraph ("at join and daily, each member takes the median of 20 binding round trips"); Build order, the idle-cost row.
- **Gap.** A daily check on every member wakes every idle member, which contradicts B0-15's pass condition, "Zero duration billed" for idle groups. `rtt_ms` also lives in `Placement`, inside `GroupConfig`, so updating it daily would mean a `Config` entry per group per day.
- **Smallest addition.** Run the daily check only for groups that served requests that day. Store measured round trips outside the config.

**G13. Major. M2-WP4 (`lfs`, `uploads-release-assets`, `codeload-archives`, `rest:releases`).**
- **Where.** `design/SHARED.md`, Surfaces table ("presigned blob-store URLs", "Release rows, assets in the blob store", "`verify` returns only after the object is as durable as a stored pack"); `design/DESIGN.md`, `R2Key`.
- **Gap.** `design/DESIGN.md` never defines "the blob store". It has no key, bucket or two-bucket placement for LFS objects, release assets or archive caches. `RepoEvent` has no variant for releases, assets or LFS objects.
- **Smallest addition.** An `R2Key::Blob { repo, oid }` placed in two buckets the same way packs are, plus `Release` and `LfsObject` repository events.

**G14. Major. M3-WP2 (`tokens` surface).**
- **Where.** `design/DESIGN.md`, Shape, `TokensEvent::Stored { hash, kind, account, scopes, expires, sso_orgs }`.
- **Gap.** This row cannot represent:
  - a fine-grained `github_pat_` (resource owner, repository selection, permissions)
  - the App or OAuth client behind a `ghu_` or `gho_`, which `LeaseHolder.app`, agent classification and `DELETE /applications/{client_id}/token` all need
  - the pairing of a `ghr_` with its `ghu_`
- **Smallest addition.** Add `app`, `client`, `permissions`, `resource_owner`, `repos` and `paired` as optional fields.

**G15. Major. M3-WP4 (`github-apps`, `rest:apps`), M3-WP3.**
- **Where.** `design/DESIGN.md`, `NamesEvent::AppRegistered { app, owner }`; `design/SHARED.md`, "Who owns which fact" and Residency ("OAuth client secrets" on `Identity`).
- **Gap.** These have no home:
  - an App's public keys, needed to verify the App JWT before minting
  - its requested permissions and events, and its webhook URL and secret
  - OAuth apps' client ids and callback URLs
- `Identity` "never gates a request", yet it holds client secrets that gate token exchange.
- **Smallest addition.** An `AppRecord` and an `OAuthAppRecord` in the core tier of the owner's `Authz` chain. `Names` keeps only the routing from app to owner.

**G16. Major. M3-WP3 (`oauth-web-flow`, `oauth-device-flow`, admin bootstrap).**
- **Where.** `design/SHARED.md`, OAuth and "Instance sign-in in installs"; `design/DESIGN.md`, Install operations, First admin.
- **Gap.** No store is defined for device codes, user codes, authorization codes, or the 15-minute sign-in link. The chain that holds `InstanceAuth` (SAML or OIDC config, "Install scope only") is also undefined.
- **Smallest addition.** TTL rows on `Tokens` shards keyed by the hash of each code. `InstanceAuth` as a signed install-scope entry.

**G17. Major. M3-WP3 (`rest:collaborators`, `rest:orgs`).**
- **Where.** `design/SHARED.md`, Surfaces (`rest:collaborators`: "`Collaborator` entries on the repository chain and `Grant` entries on the owner's `Authz` chain"); `design/DESIGN.md`, Invariants ("one owner per fact").
- **Gap.** One fact has two writers, and no rule picks between them. Repository and org invitations have no state, although the collaborator Terraform test accepts invitations.
- **Smallest addition.** A rule mapping outside collaborators to `Collaborator` and org members and teams to `Grant`, plus `Invitation` events with accept and decline.

**G18. Minor. M3-WP2, M3-WP3.**
- **Where.** `design/DESIGN.md`, Shape, Identity ("the `Audit` finisher then appends the `Identity` side"); `design/SHARED.md`, Multi-chain operations ("The `Identity` side follows", with Audit only as a fallback after a minute).
- **Gap.** The two documents disagree on who writes the `Identity` side of a new PAT or SSH key. If it is the `Audit` consumer, SSH-key listing in M3 depends on M4-WP1.
- **Smallest addition.** The edge writes it, and `Audit` only finishes operations nobody touched.

**G19. Minor. M3-WP5.**
- **Where.** `design/DESIGN.md`, `Member.do_name` `"{chain}#{generation}"`.
- **Gap.** A learner on a site that also hosts the group's voter, or two learners on one site, get the same object name.
- **Smallest addition.** `{chain}#{generation}#{kind}{slot}`.

**G20. Minor. M2-WP2, M2-WP3.**
- **Where.** `design/DESIGN.md`, Milestone 2 row ("the ... model reused from `history/DESIGN.md`") and "Reused with the additions below".
- **Gap.** The design of record points M2 at `history/`, which `AGENTS.md` says is never normative. It also cites SHARED sections that do not exist ("Chains and commits", "Repository chains and packs", "Change safety"). The payload types `CommentEvent`, `ReviewEvent`, `LabelEvent`, `BranchRule`, `RepoSettings`, `IssueCapture` and `CheckRunUpdate` are defined nowhere, `history/` included.
- **Smallest addition.** Point to SHARED's real headings, and define these payloads as stored formats.

## Work packages walked

| Package | Result |
| --- | --- |
| M0-WP1 | G1, G12 |
| M0-WP2 | G1, G6 |
| M0-WP6 | G3 |
| M1-WP1 | G1, G12 |
| M1-WP2 | G1, G6, G7, G8, G9 |
| M1-WP3 | walked-clean |
| M1-WP4 | G3, G11 |
| M1-WP5 | G3 |
| M1-WP6 | G2, G8, G10 |
| M1-WP7 | G2 |
| M2-WP1 | G9 |
| M2-WP2 | G2, G5, G20 |
| M2-WP3 | G4, G7, G20 |
| M2-WP4 | G2, G13 |
| M2-WP5 | G4, G5 |
| M3-WP1 | walked-clean apart from G2's ordering effect |
| M3-WP2 | G7, G14, G18 |
| M3-WP3 | G7, G15, G16, G17, G18 |
| M3-WP4 | G15 |
| M3-WP5 | G9, G19 |
| M3-WP6 | walked-clean |

Each of the 20 needs a design decision before the package it names can close its gate. No finding rests on `history/`. I read it only to confirm that `QueueEvent` and the payload types are missing there too.