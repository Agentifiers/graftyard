# Implementability walk: operations and agents (WalkOpsAgents, poteto-agent, read-only)

I walked all 30 work packages in scope and found 20 gaps that would force an implementer to guess. Six are blockers. Most of them sit in the release-control layer (the `Release` group, signed intents, pointers, and `graft-ops`) and in how the agents account connects to the install. Every quote below is text I read in this session (measured). Everything I call "smallest addition" is my own proposal (inferred). Each finding lists its id, severity, work package, location, the gap with a quote, and the smallest addition that would settle it. Findings run deepest first.

## Findings

**G1. Blocker. M5-WP1, M5-WP3, M5-WP4, and `release roll`.**
- **Where.** `design/DESIGN.md`, Shape, "Release group", and "Failure modes", `release control`.
- **Gap.** The `Release` group has no entry that records an intent's progress, its finish, or a site's current release. No key could sign such an entry either. The design says an undo is valid "only while its parent intent is the current one (the latest committed intent not yet finished or aborted)". It also says "`graft-ops` is stateless and resumes each intent from the group". But `ReleaseEntry` has no step, finish, or site-release variant. The only key `graft-ops` holds is the "drain-only key that can sign `SiteMode` and nothing else". "Only a passed day lets the next site roll" and the canary that "reports the site's release" both need a recorded per-site release. One `SignedIntent` holds one `Intent`, so the text never says how `graftyard release roll 2…
- **Smallest addition.** Add `IntentStep { intent, step }`, `IntentDone { intent }`, and `SiteRelease { site, release }`, signed by a new narrow `Keys.executor` key. Add one paragraph that maps `release roll` to an ordered list of intents.

**G2. Blocker. M5-WP1, M5-WP3, and `release roll`.**
- **Where.** Shape, "Release group". `design/SHARED.md`, "Release registry and signing".
- **Gap.** `decide` must refuse "a `SiteDay`, a pointer, or an `Abort` whose target release's signed record does not read every enabled format". `Abort` targets "the newest uploaded release". But no entry brings release records, or the uploads' Cloudflare `VersionId`s, into the fold. A `VersionId` exists only after `graft-ops` uploads a bundle, yet the intent that names it is signed by a person. SHARED's release record lists no set of readable formats. None of `Genesis`, `InstallId`, `VersionId`, `ReleaseId`, `Format`, `Scope`, `SecretName`, `ImageRef`, or `SessionId` is defined anywhere in `design/` (measured with grep).
- **Smallest addition.** Add `ReleaseUploaded { release, record, versions: [(script, VersionId)], images, reads: FormatSet }`, committed by `graft-ops` after the release signature verifies. Define the missing types.

**G3. Blocker. M5-WP2 and AM2-WP3.**
- **Where.** "Commit and read paths", intro and journey 7. Shape, `SignedIntent` and `POINTER_TTL`.
- **Gap.** The text says `graft-front` "forwards it to the edge version named by its cached copy of the `Release` group's `EdgePointer`", and edge calls to `graft-ops` carry "a version override from the signed `OpsPointer`". So every call is pinned. Yet "the edge ships by gradual deployment, by percentage", and `OpsRoll` and `McpRoll` "run at 1% for a day". A call pinned by an override never takes the deployment's percentage split. `EdgePointer { version }` holds one version and no percentage. Only the pre-signed undo pointers have a signer. No key is named for the forward pointer move.
- **Smallest addition.** Make each pointer `Option<VersionId>`, where `None` means follow the deployment and `Some` means pin for swap-back. The alternative is `{ stable, canary, percent }` with `graft-front` doing the split. Either way, name the key that signs forward pointers.

**G4. Blocker. M5-WP1 and the CLI commands `install`, `sites add`, and `sites add-spare`.**
- **Where.** "Usage", operator block. "Install operations", Genesis and Keys.
- **Gap.** The usage example runs `graftyard install ... --genesis genesis.signed` before `sites add` and `sites add-spare`. Yet the genesis "carries ... the sites, and the spares", and the `Release` group sits on `Tri`, so its sites must already exist. No command produces `genesis.signed`. The genesis must also carry the drain-only key, held by `graft-ops`, and the ring keys before those scripts exist. Nothing says how an empty `Release` group chooses which first genesis to accept. Whoever reaches the edge first could plant theirs (inferred).
- **Smallest addition.** Order sites before genesis. Add a `graftyard genesis` command that creates the drain key inside `graft-ops`'s Secrets Store and emits an unsigned genesis for offline signing. Add a trust anchor: the install id and the customer public key, set as script variables when Wrangler creates the scripts.

**G5. Blocker. AM3-WP1 and LM-3's "a bad controller at 1% swaps back".**
- **Where.** `design/AGENT-LAYER.md`, A8; "Failure modes", controller rows; "GHES compatibility", Controller row.
- **Gap.** The controller is reached at "a webhook hostname in the agents account's own zone". `graft-front` lives in the install account and routes only the host and `mcp.<host>`. The design says "`ControllerPointer` swap-back is a signed entry its `graft-ops` reads". But `graft-ops` reading the pointer can only call the deployments API, and the swap-back is supposed to work with the API blocked. No component pins controller traffic by version. A8 says the agents `graft-ops` reads intents "over HTTPS from the install's `Release` group". No endpoint, authentication, or trust root for that read is given.
- **Smallest addition.** Add a front Worker in the agents account that pins by `ControllerPointer` with a version override. Add a `Release` read endpoint whose entries are verified against an install id and customer key that `agents init` provisions.

**G6. Blocker. AM3-WP3, AM3-WP1, and LM-3's "A3 end to end in both modes".**
- **Where.** Agent block, `AgentTask` and the `graftyard_task` rule. A3, steps 3, 5, and 9. The L0 token-ceilings row.
- **Gap.** The rule requires a live task "with `task.app == jwt.app` whose repository is among the token's `repositories`". In fork mode the push goes to `<org>-agents/<repo>`. That is a different owner, so a separate installation and a separate token, and A3 gives the fork to `graftyard-workspace` while `task.app` is `graftyard-agent`. Yet the L0 row requires that "a fork-mode token pushes to its fork after its task is cancelled" is refused, which needs the task claim on the fork token. In-repo mode gives `graftyard-agent` "`contents: write` on the base". Fork mode, though, relies on that same App having "only `contents: read` on the base", backed by the quote that an installation token "cannot be granted permissions that the app was not granted". One App r…
- **Smallest addition.** Allow the claim when the token's repository is the task's repository or its workspace fork. Name the App that pushes to the fork. Give each of the four manifests its permissions and events.

**G7. Major. M5-WP1, M5-WP6, and every CLI command that signs.**
- **Where.** "Install operations", Keys and Revocation runbook. Shape, `Bound` and `LogEntry`.
- **Gap.** The group refuses "any entry not bound to this install and the group's last digest", while the customer intent key "stays with the customer offline". Other entries land on the `Release` log while a person signs: `EntryBody::Noop` ("first entry of every term", and a site day's drain forces a new term), drain-key `SiteMode`, undos submitted by edge isolates, and lifeboat reports. The text never says whether `prev` means `LogEntry.prev`, which counts Noops, or the last signed entry. Signed entries are submitted "through the edge" by no named endpoint, and no refusal shape is given for a stale digest.
- **Smallest addition.** Define `prev` as the digest of the last committed signed `ReleaseEntry`, excluding Noop and Config. Add `POST /api/graftyard/v1/release/entries`, which answers 409 with the current digest when `prev` is stale.

**G8. Major. M5-WP1, M5-WP3, BF-7, and BM-5.**
- **Where.** Shape, "Release group" (the `Abort` comment). "Build order", final-pass rows.
- **Gap.** The design says "`Abort` moves every site and pointer to the newest uploaded release that reads every enabled format", so its target is always valid. BF-7 expects that, after `FormatsEnable`, an `Abort` is "refused, and ... page[s]". BM-5 expects "`Abort` returns every site to N". Two implementers would build different `decide` logic: one retargets the abort forward, the other refuses it.
- **Smallest addition.** Give `Abort` an explicit target release, refused when that release cannot read an enabled format. Or define the computed target and state when an abort is refused.

**G9. Major. M6-WP1 and BM-6.**
- **Where.** Journey 5. Shape, `Intent` and `ReleaseEntry`. "Scale", Workflow starts.
- **Gap.** "`ReplaceMember` ... runs only on a signed `SiteLost` intent or after 24 h of zero acks". But `SiteLost` is a `ReleaseEntry` signed by the customer key, not an intent. `Intent::ReplaceMember` and `Intent::RestorePlacement` have no named signer. The automatic 24-hour path has no key that `graft-ops` holds. Nothing says who measures "zero acks", who starts the Workflow instances, or what "after A recovers" means. The Workflow must replace "every group with a member on a lost site". But groups are "created by name on first use", and BM-6 revokes every API token. The design names no source for that list of groups.
- **Smallest addition.** Write a trigger state machine with named signers. Name the enumeration source, for example walking the `Names` shards plus the fixed shard sets, or a per-site member registry.

**G10. Major. M5-WP5, M6-WP3, `vault init`, and `lifeboat init` and `enroll`.**
- **Where.** "Install operations", Vault and Lifeboat. Shape, "Lifeboat feed".
- **Gap.** The vault "pulls the change feed and packs from the install's edge over HTTPS with an install-scoped export token signed by the customer key". The lifeboat "reads the export bucket with an R2 Object Read only token" and "pulls log tails from group leaders through the edge". `ChangeBatch` holds only `(ChainId, Index)`. The design gives no endpoints, no export-token claims, no encoding for entries, no answer to which bucket holds `R2Key::Export`, and no path for entry bodies to reach R2. It also defines no endpoint for minting `lifeboat:read` tokens, and no submit path for `LifeboatDecoder` reports.
- **Smallest addition.** Specify export endpoints for batches, chain entries from an index, and packs. Add the token claims. Say whether batches carry entry bytes. Add an enroll endpoint.

**G11. Major. AM2-WP1, AM3-WP1, and `agents init`.**
- **Where.** A8. `design/AGENT-LAYER.md`, "Deployment modes", Enable agents, step 1.
- **Gap.** `agents init` "writes their keys and the client secret into that Secrets Store", meaning the agents account's. But `graft-mcp` runs in the install account and needs the `Graftyard MCP` client secret to redeem codes. The insights consumer runs in the site scripts and needs the `graftyard-insights` key. A8 also says "a self-hoster may skip it and still get MCP ... check runs". AM2 comes before AM3, yet only `agents init` (AM3-WP1) registers the `Graftyard MCP` App.
- **Smallest addition.** Split out `graftyard apps init` in the install account for `Graftyard MCP` and `graftyard-insights`, with their keys in the install's Secrets Store, and place it in AM2.

**G12. Major. AM2-WP1.**
- **Where.** Shape, `graft-mcp` (`McpGrant`, `McpRefresh`, `WriteScope`). A7, steps 5 to 7.
- **Gap.** `graft-mcp` "calls the edge only". Yet it must commit `IdentityEvent::McpGrant`, check and bump `generation` on every refresh, revoke on replay, list grants at `/settings`, and have "`token/scoped` for `Graftyard MCP` store[] the grant id and its `WriteScope` on the token's `Tokens` row". GHES's `token/scoped` takes neither field. No internal endpoint or caller authentication is defined.
- **Smallest addition.** Add grant endpoints under `/api/graftyard/v1/mcp/grants`, authenticated by the App's client credentials. Add the additive `token/scoped` fields.

**G13. Major. AM1-WP3, AM1-WP4, AM2-WP2, and AM3-WP3.**
- **Where.** "Tasks and the activity feed"; "Leases"; "GHES compatibility", Task API, Leases, Activity feed, and Plan-block rows.
- **Gap.** These are contracts clients code against, and only their paths are named. Missing:
  - Request and response bodies for tasks and leases, including `issue`, `prompt`, and the `session` and `requester` of an `External` task.
  - Field names and casing.
  - How `FeedCursor { group, index }` is encoded in `?cursor=`, the units and maximum of `wait`, and the list syntax of `paths`.
  - Pagination for the list endpoints.
  - Bodies for the 409 and 410 errors.
  - The format of `SessionId`. Session pages use `{uuid}`, but `External` sessions are named by the App and become `refs/heads/graftyard/<session>/`.
  - The syntax inside a `graftyard-plan` block.
  - The event name, payload, and signing secret of the first-party task delivery.
- **Smallest addition.** An OpenAPI fragment for `/api/graftyard/v1`. A `SessionId` rule, for example a UUID minted by the host for Graftyard-run tasks, and a ref-safe id unique per repository and App for external ones. The plan-block grammar.

**G14. Major. M5-WP6 and BM-5.**
- **Where.** "Install operations", Revocation runbook.
- **Gap.** After `Halt`, `Keys { vendor: None }`, and then `Resume`, the resumed intent and its pre-signed undos are signed by the vendor. Yet "from then the group and `graft-ops` refuse every vendor-signed entry". Nothing says whether a key is checked at commit time or at execution time. So nobody can tell whether a gate breach after `Resume` can still roll back with no human signature.
- **Smallest addition.** Have `Resume` carry undos re-signed by the customer. Or validate undos against the `Keys` that were current at the parent intent's index.

**G15. Major. M5-WP4.**
- **Where.** "Install operations", Site day. Journeys 5 and 7. Shape, `POINTER_TTL`.
- **Gap.** Each step gates on "apply lag, apply and verify exceptions, and ack rate against the same site's N members". No breach threshold is given, and no dwell time for the 1%, 10%, or 50% steps. Automatic drain has a start rule but no undrain rule. Members must also act on `SiteMode`, the first-day 1% lead limit, and "`Release` records a rollback", but isolates cache only "the four pointers".
- **Smallest addition.** A table of compiled gate and dwell constants. An undrain rule signed by the drain key. A statement that site isolates cache the full `Release` view with the same TTL.

**G16. Major. AM1-WP2, AM1-WP3, and the L0 lease-limits row.**
- **Where.** Shape, `AgentApp` ("Install setting, an `Authz` entry"). "Deployment modes", Enable agents, steps 4, 5, and 7.
- **Gap.** `Authz` is per account (`Authz(AccountId)`). The design names no account whose chain holds install-wide `AgentApp` rows, nor how a repository's `decide` reads them. No REST or admin surface sets `AgentApp`, `AgentPolicy`, `agents_enabled`, or the per-org choice of in-repo mode. The gate requires "Uninstall ... cancels every task". Lease release is derived, but no mechanism is stated for cancelling tasks across every repository group.
- **Smallest addition.** Name the chain and read it as a `dep`. Define the settings endpoints. State that an uninstalled App's tasks read as cancelled.

**G17. Major. M5-WP3 and M5-WP6.**
- **Where.** "Install operations", Rotation runbook. "Deployment modes", BYOC token permissions. Shape, `Intent::Rotate`.
- **Gap.** The runbook says "create the new value, write it to Secrets Store". But the deploy token has "exactly Workers Scripts Edit and Containers Edit". That is not enough to write Secrets Store or create customer tokens. The design never says what `Rotate { secret }` makes `graft-ops` do for each secret, or who signs the intent.
- **Smallest addition.** A table with one row per secret: who acts, where the new value comes from, how it reaches Secrets Store, and which steps `graft-ops` runs.

**G18. Major. M6-WP1 and M5-WP5.**
- **Where.** "Deployment modes", the artifact list. Shape, `Intent`. "Failure modes", `vault` bad-deploy.
- **Gap.** The artifact holds "`graft-front`, the edge, the three site script bundles, and `graft-ops`". The Workflows script and the vault Worker are missing from that list, and no `Intent` rolls the Workflows script. The vault has a "bad vault release" failure row, but by design the install's tokens cannot reach the vault account.
- **Smallest addition.** Add a `WorkflowsRoll` intent, or ship the Workflows inside `graft-ops`. Add a vault upgrade command that runs under the vault account's token.

**G19. Major. M5-WP1 (`StaleMode` and `stale`).**
- **Where.** Shape, the read path and `ReleaseEntry`. "Tradeoffs accepted".
- **Gap.** "Stale mode starts only by a signed `StaleMode` flag in the `Release` group". The key lists name no signer for `StaleMode`, and `Scope` is undefined. The `Release` group is itself a quorum group, so it may be unable to commit in exactly the partition or outage where stale mode is needed (inferred). Members are not said to read the flag.
- **Smallest addition.** Name the key. Define `Scope`. State how an isolate accepts a signed flag that has not committed, for example by verifying it against the cached `Keys`.

**G20. Major. M6-WP2.**
- **Where.** `design/SHARED.md`, `git-ssh` row ("One service host key set"). Shape, "SSH front". Rotation runbook.
- **Gap.** The design says nothing about where the SSH host keys live, how stateless container replicas on three sites get them, how they rotate, or how `/meta` publishes them. Developers pin host keys, so two builds with different custody would show different fingerprints (inferred).
- **Smallest addition.** Keep the host keys in Secrets Store, deliver them to containers through the outbound handler, add them to the rotation runbook, and publish them in `/meta`.

## Work packages walked

- **M5-WP1.** G1, G2, G4, G7, G8, G19.
- **M5-WP2.** G3.
- **M5-WP3.** G1, G2, G8, G17.
- **M5-WP4.** G1, G15.
- **M5-WP5.** G10, G18.
- **M5-WP6.** G7, G14, G17.
- **M5-WP7.** Gate only. It inherits G1, G8, and G14.
- **M6-WP1.** G9, G18.
- **M6-WP2.** G20.
- **M6-WP3.** G10.
- **M6-WP4.** Gate only. It inherits G9 and G10.
- **AM1-WP1.** Walked clean.
- **AM1-WP2.** G16.
- **AM1-WP3.** G13, G16.
- **AM1-WP4.** G13.
- **AM1-WP5.** Walked clean except a minor gap below the cut. The exact set of invisible and bidirectional characters, and the resources that get `graftyard_trust`, are not listed.
- **AM1-WP6.** Gate only. It inherits G13 and G16.
- **AM2-WP1.** G11, G12.
- **AM2-WP2.** G13.
- **AM2-WP3.** G3.
- **AM2-WP4.** Gate only. It inherits G3 and G12.
- **AM3-WP1.** G5, G6, G11.
- **AM3-WP2.** Walked clean. Minting tokens across two owners falls under G6.
- **AM3-WP3.** G6, G13.
- **AM3-WP4.** One gap below the cut. In fork mode the log is pushed to the fork, while the session URL in the trailer names a repository the text does not pin down.
- **AM3-WP5.** Gate only. It inherits G5 and G6.

**CLI commands.**
- `install`, `sites add`, and `sites add-spare` are blocked by G4.
- `release roll` is blocked by G1, G2, and G7.
- `vault init` is blocked by G10.
- `lifeboat init` and `lifeboat enroll` are blocked by G10.
- `agents init` is blocked by G5, G6, and G11.
- None of these can be built from the text alone without guessing.

## How I worked

Throughput checkpoint: n/a, read-only investigation.

I followed the Investigation playbook.
- **Route through how.** Skipped. The brief fixed the method (a module skeleton per package), so I did not read the how skill.
- **Throughput checkpoint.** Done, one line.
- **Output.** Findings in the format the brief asked for.
- **Unslop the reply.** Done.

I did not read `history/`, and I re-reported nothing from R4 to R8.

Foundational Thinking shaped the method. I named each package's types and tables before its logic, and that is how G1 and G2 surfaced: entry kinds the executor needs, with no data shape to hold them. Its concurrency corollary ("what happens if another actor modifies this concurrently?") produced G7, about offline signatures racing the other writers of the `Release` log.

## Open decisions for you

- **Pointer meaning (G3).** "Pointer as pin-only override" or "pointer carries the canary split" is a design call. It decides how `graft-front` is built.
- **App split (G6, G11).** Splitting the four first-party Apps across the install and agents accounts changes what an operator runs and in which milestone.

[Some lines truncated to 768 chars. Use agent://WalkOpsAgents:raw:7-7 to read line 7 whole; cut lines run to line 32, each read the same way]