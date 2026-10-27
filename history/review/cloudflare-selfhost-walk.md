# Self-host walk of `cloudflare/DESIGN.md` (panel seat C)

Persona: a self-hoster with three Linux hosts in three racks of one building, no Cloudflare account, holding only `cloudflare/DESIGN.md`. Second persona: a BYOC customer whose developers use the lifeboat during a Cloudflare-wide outage. workerd facts come from `cloudflare/workerd` main at commit `ec3982b2` (2026-10-26), read in `src/workerd/server/workerd.capnp`, `src/workerd/server/AGENTS.md`, `src/workerd/server/factory/worker-factory-actor.c++`, `src/workerd/io/actor-sqlite-test.c++`, `src/workerd/util/sqlite.c++`, `build/BUILD.sqlite3`, and `README.md`.

## 1. Verdict

Change the self-hosted mode; the base holds with changes. The Cloudflare-hosted modes survive this walk, and nothing here argues for dropping the Raft-over-Durable-Objects base. The self-hosted mode at lines 535 to 561 is not real. It rests on two workerd features that workerd itself labels experimental or for development only [measured]. It runs every member, edge, and git-cache object of a host on one event-loop thread [measured]. It needs four engines the design never specifies (blob store, queue, workflows, cron) [measured]. It cannot pass its own placement rule [measured], and it has no install, backup, or restore text [measured]. The frame defines self-hosted as customer-operated in the customer's own cloud account, not on the customer's hardware (`FRAME.md` line 17; `DESIGN.md` line 945) [measured]. So the fix is to redefine the Cloudflare self-hosted mode as "the customer operates the same artifact in its own Cloudflare account, with no vendor token". Running on the customer's own hardware should be stated as unsupported for production and allowed for evaluation only. workerd stays in the design only for the lifeboat. The walk also exposes one foundation issue that affects every mode: each site deploy resets every member on that site, which stalls writes on about a third of all groups (finding 6) [inferred]. The lifeboat has three concrete breaks (findings 9 to 11).

## 2. Findings

### F1. Blocker. The self-hosted mode rests on workerd features that Cloudflare marks experimental or for development only, yet the design promises "the same quorum durability"

- **Rests on:** lines 7, 88, 353, 535 to 542, 555 to 561, 655.
- **Walk:** At install, the self-hoster configures `durableObjectStorage = (localDisk = ...)`. The schema marks it "** EXPERIMENTAL; SUBJECT TO BACKWARDS-INCOMPATIBLE CHANGE **" (`workerd.capnp` lines 807 to 808) [measured]. For git caches they set `containerEngine = (localDocker = ...)`, which the schema documents as "Only used for local development and testing purposes" (`workerd.capnp` lines 836 to 838) [measured]. The README warns that "`workerd` is not a hardened sandbox" (README line 32) [measured]. Durable Objects "are always local to one instance of the runtime" (`workerd.capnp` lines 819 to 820) [measured]. The stack table still lists workerd as "GA [inference]" (line 88), and line 561 promises "the same quorum durability and the same wire behavior" [measured].
- **Research the design already had:** `research/cloudflare-git.md` conclusion 8 says self-hosting "would have to supply its own durable DO storage (celld-like) and S3-compatible backends, which makes it a second storage engine" [measured]. The design ships that second engine as `trib-blob` (line 536) with no specification.
- **Frame:** `FRAME.md` line 17 defines self-hosted as "The customer | The customer's AWS account", and the AWS design runs self-hosted in the "Customer's dedicated account" (`DESIGN.md` lines 944 to 945) [measured]. The Cloudflare frame asks for the modes "as far as Cloudflare allows them" (`FRAME.md` line 39) [measured]. A mode built on features Cloudflare documents as experimental or for development only is not one it allows for production [inferred].
- **Fix:** Rewrite the Self-hosted column (lines 533 to 542) and the requirements (lines 555 to 561) as follows. "Self-hosted means the customer operates the same artifact in its own Cloudflare account, with no vendor token. It is the BYOC column with release intents signed only by the customer (line 541)." Add one line: "Running on the customer's own hardware with workerd is unsupported for production. It is allowed for evaluation under the `evaluation` flag, as `Solo` is." Change line 88 to "workerd | runtime GA; `localDisk` experimental, `localDocker` development only | lifeboat and evaluation". Change line 353 and line 655 to match. If the operator insists on on-premises production, F2 to F8 become required work, and the design must budget a storage engine for it.

### F2. Major. The placement model cannot express three racks in one building, so the install refuses at its first step

- **Rests on:** lines 39, 110, 114, 131 to 134, 186, 274, 535.
- **Walk:** The self-hoster runs `tributary placement set default tri --sites A,B,C`. That command "refuses two sites in one region" (line 39) [measured]. `Region` has only Cloudflare hint names (line 114), and `Placement::new` "refuses two voters in one region" (line 131) [measured]. `QuorumProof.regions` must be at least 2 unless the group is `Degraded` or `Solo` (line 186) [measured]. Three racks in one building are one region. The self-hoster must either lie (label racks `enam`, `weur`, `apac`) or fall back to `Solo`, which needs the evaluation flag. Join-time placement checks use `/cdn-cgi/trace` and the R2 get-bucket `location` (line 110), and the daily Cron check repeats them (line 274). Neither exists off Cloudflare [inferred].
- **Fix:** If on-premises survives F1, add `enum FailureDomain { Region(Region), Declared(Box<str>) }` to `Site`. `Placement::new` then refuses two voters in one failure domain, and `QuorumProof` counts failure domains. The placement check for `Declared` is an operator-signed attestation plus a host-id check. If F1's fix is taken, line 535 drops workerd and this finding falls away.

### F3. Major. The on-premises install has no text, and four runtime services it needs do not exist in workerd

- **Rests on:** lines 34 to 44, 76 to 78, 235 to 237, 318, 329, 538 to 540, 556 to 559.
- **Walk, install:** Every command shown takes `--account` or a Cloudflare script name (lines 35 to 44) [measured]. Nothing tells the self-hoster how three hosts learn each other's addresses and Ed25519 keys, how the edge runs on each host, or what the load balancer health-checks [measured].
- **Missing services:**
  - Cron Triggers drive the pack sweeper, GC, placement checks, and fold audit (line 78). `workerd.capnp` has no cron or scheduled trigger [measured: grep for "cron" and "scheduled" returns nothing]. Alarms do exist: `alarm-scheduler.c++` keeps them in SQLite [measured].
  - `ReplaceMember` is a Workflow (lines 77, 329). workerd's `workflowsEngine` is "the externally-supplied service responsible for running Workflows" (`workerd.capnp` lines 274 to 305, 874 to 875) [measured].
  - A queue binding is "implemented by the named service" (`workerd.capnp` lines 476 to 479) [measured]. Line 539's "in-process queue" is therefore Tributary code.
  - Analytics Engine bindings require the `--experimental` flag (`workerd.capnp` line 490) [measured], and learner placement reads them (line 146).
- **Walk, SSH:** Line 538 says "sshd listens directly". But `trib-sshd` reaches groups only through the container outbound handler (lines 235 to 237, 318) [measured]. workerd can do both halves:
  - A `tcp` socket hands raw connections to a `connect()` handler (`workerd.capnp` lines 149 to 154, 185 to 194) [measured].
  - The `containerEgressInterceptorImage` sidecar routes container egress "through workerd for egress mappings (setEgressHttp bindings)" (lines 845 to 849) [measured].

  The design states neither half.
- **Walk, TLS:** The workerd keypair is static PEM in the config (`workerd.capnp` lines 1095 to 1108), with no ACME [measured]. Renewing it means reloading the config, and that restarts every member on the host (F6) [inferred].
- **Walk, clocks:** The lease assumes drift under 500 ms (lines 168, 663), and nothing requires NTP on the customer's hosts [measured].
- **Fix:** Only if on-premises survives F1, add an "On-premises install" subsection with these pieces:
  - the exact `tributary selfhost init --hosts h1,h2,h3 --domain-labels rack1,rack2,rack3` flow;
  - cron as a per-site alarm DO;
  - a named Workflows engine (Miniflare's DO-based engine, vendored and pinned);
  - the queue service;
  - SSH as a workerd `tcp` socket on port 22 to the SSH front DO's container through the egress interceptor;
  - certificate rotation by rolling host restarts after F6's leader drain;
  - chrony required, with members refusing lease reads when measured peer skew exceeds 100 ms.

### F4. Major. `trib-blob` has no durability contract, and the acknowledgment invariant borrows R2's

- **Rests on:** lines 213 to 215, 269, 291, 440, 536.
- **What the design claims:** The acknowledgment invariant says pack bytes are "each persisted before its 200", and it cites the R2 durability page (line 269) [measured]. On premises the bytes go to `trib-blob`, specified only as "same 2-of-3 rule, served through an R2-shaped binding" (line 536) [measured].
- **The SQLite side holds:** workerd opens each object in WAL mode (`worker-factory-actor.c++` line 290) and never sets `synchronous`. The build defines no `SQLITE_DEFAULT_*SYNCHRONOUS` (`build/BUILD.sqlite3` lines 13 to 23), so SQLite's default FULL applies [measured]. The VFS `xSync` calls `file->sync()` or `datasync()` (`sqlite.c++` lines 2239 to 2244) [measured]. workerd treats the object as "durably stored after the synchronous sqlite commit() call returns" (`actor-sqlite-test.c++` lines 402 to 404) [measured].
- **Failure path:** The building loses power during a 50 MiB push. The entry commits on A and B, and their SQLite files are fsynced. If `trib-blob` answered 200 before it fsynced the pack file and its directory, the pack can vanish on both A and B. A committed `Stored` `PackRef` then points at missing bytes, which is a lost acknowledged push [inferred]. The same contract has to cover `If-None-Match: *` atomicity and bit rot, because no scrub is stated [measured].
- **Fix:** Add a `trib-blob` contract that holds whatever F1 decides, since the lifeboat stores packs too:
  - write to a temp file, `fsync` it, `link()` it to the final key (which fails if the key exists, giving `If-None-Match: *`), and `fsync` the directory, all before any 200;
  - verify `PackRef.checksum` on every read;
  - run a weekly scrub that repairs from a sibling copy;
  - require disks that honor cache flushes.

### F5. Major. At 2,000 developers one host runs every member, edge request, and byte stream on one thread

- **Rests on:** lines 361 to 373, 535 to 538, 144 to 147.
- **Threading:** workerd's server runs "One thread, one loop" (`src/workerd/server/AGENTS.md` line 106) [measured]. Its objects are "always local to one instance of the runtime" (`workerd.capnp` line 819) [measured]. So each site's members must live in one process. Every group has a voter on every host, so learners (line 144) add no capacity on premises [inferred].
- **Load:** From `cost.py`'s `workload("large")`:
  - 145,647 GB of git bytes a month, or 56.8 MB/s on average [measured: script run];
  - about 570 MB/s at the design's 10x peak (line 372) [inferred];
  - all of it proxied by the edge, which "tees" container output to the client and the clone cache (line 278).

  The SQLite commit and its fsync run synchronously on that same loop (F4 evidence), so each commit's fsync stalls every request on the host [inferred].
- **Per-host disk:**
  - 700 GB of member SQLite (`core_gb` 200 plus `aux_gb` 500) [measured: script run];
  - about 4 TB of packs, because the sweeper fills the third bucket (line 291) [inferred];
  - about 10 TB of the 30 TB of Actions blobs, held on the job leader's site (line 445) [inferred].

  That is about 15 TB per host, and the design states no host sizing [measured].
- **Fix:** Under F1's fix this is moot. If on-premises stays, run N workerd processes per host, each owning a hash range of chains, with the edge routing by range. State host sizing (cores, about 15 TB of disk, 10 GbE at 2,000 developers). Add a Milestone 7 gate: 2,000-developer synthetic load on three hosts with loop-thread p99 latency under 50 ms.

### F6. Major, all modes. Every site deploy or host restart resets every member on that site, so about a third of groups stall writes for 10 to 15 s

- **Rests on:** lines 41, 99, 323, 398, 422, 427, 455, 484.
- **Walk:** The self-hoster upgrades host A on Monday by restarting workerd. Every member on A restarts, and every group led from A loses its lease. "a leader reset loses its lease and a follower wins within about 10 s" (line 455) [measured].
- **The same happens on Cloudflare:** A site deploy reassigns the version, and a Durable Object "will only be reset when it is assigned a different version" (lines 99, 398). The gradual deployments page says "only one version of each Durable Object can run at a time" (https://developers.cloudflare.com/workers/versions-and-deployments/gradual-deployments/index.md, "Durable Objects") [measured]. Line 398 counts this as a feature ("Every site-day deploy exercises it").
- **Arithmetic** [inferred, assumes leaders spread evenly across sites]:
  - Writes to a third of groups fail for 15 s, which is 1/3 x 15/60 = 8.3% of a minute's writes. That is above the 5% bad-minute line (line 422).
  - So each site deploy costs about one bad minute. A weekly release is three site deploys, about 13 a month, or about 13 of the 21.6-minute write budget (line 427).
  - Before any incident, that is 60%.
- **Fix:**
  - Before a site deploys or a host restarts, the release tool appends a `Drain { site }` intent to the `Release` group. Each leader on that site performs Raft leadership transfer (TimeoutNow) to the follower with the most recent log, and the drained site's members refuse candidacy until the drain ends. The deploy proceeds only when no group is led from the site.
  - On premises, the drain also waits for in-flight clone streams, up to the 20-minute bound at line 369.
  - Add a Milestone 0 row: "Site deploy under 100 writes/s: zero failed writes, p99 commit at most 1 s during the deploy."
  - Rewrite line 398 to stop describing elections as the deploy mechanism.

### F7. Major. A workerd upgrade can make a host's member files unreadable, and the release rules do not cover the runtime's on-disk format

- **Rests on:** lines 262, 348, 516, 529, 541.
- **Walk:** The self-hoster upgrades the workerd binary on host B. `localDisk` is "SUBJECT TO BACKWARDS-INCOMPATIBLE CHANGE" (`workerd.capnp` line 808) [measured]. The README's "will never break your JavaScript code" promise (README line 28) covers the JavaScript API, not stored data [measured]. If the format changes, every member on B fails to open. That is a full host loss, followed by a rebuild of 700 GB of SQLite plus the packs (F5) [inferred]. The two-release format rule (lines 262, 348) covers Tributary's entry formats only [measured].
- **Second trap:** `durableObjectUniqueKeyModifier` and each namespace's `uniqueKey` name the storage directories. The schema says "DO NOT LOSE this value, otherwise it may be difficult or impossible to recover stored data" (`workerd.capnp` lines 783 to 788) [measured]. The design never says these config values are state that must be backed up [measured].
- **Fix:**
  - Pin the workerd binary version inside the signed artifact (line 529 lists only "a workerd config plus the same images").
  - Treat a workerd version change as `ReplaceMember` onto a fresh directory, one host per day, never as an in-place restart.
  - List `uniqueKey` and the modifier as backed-up install state.

  This applies to the lifeboat whatever F1 decides.

### F8. Major. On-premises installs have no off-site copy and no restore procedure

- **Rests on:** lines 439, 442 to 447, 542, 561, 636.
- **Walk:** The building floods. All three hosts and their disks are gone.
  - The design gives self-hosted no point-in-time restore. DO PITR is a Cloudflare feature (lines 439, 443).
  - The lifeboat is "Not needed, because the install already runs off Cloudflare. A second site can play the same role" (line 542) [measured]. A "second site" in the same building is not off-site [inferred].
  - `RestoredTo` (line 442) recovers one repository from its own log and cannot help when every log is gone [inferred].
  - The one whole-install recovery row says "Rebuild sites from the lifeboat export" (line 447), with no procedure and no gate [measured].
  - Milestone 7's gate powers one host off (line 636) and never drills a restore [measured].
- **Fix:**
  - Make the lifeboat feed mandatory for any install off Cloudflare, sent to a host in another building, with RPO equal to the feed lag (F11).
  - Write the restore as a numbered procedure: rebuild members from the export's snapshots and log tails, verify every `PackRef.checksum`, then open writes.
  - Add a Milestone 7 gate: restore a three-host install from its lifeboat into new hosts with zero lost entries up to the export cursor.

  Under F1's fix, the same gate applies to BYOC installs (Cloudflare-wide data loss, line 447).

### F9. Major. The lifeboat trails site releases by a week, but formats are enabled after the sites alone, so its feed stalls exactly when it may be needed

- **Rests on:** lines 262, 348, 393, 509, 516.
- **Walk:** Release N+1 rolls to sites A, B, and C on Monday to Wednesday. `formats:enable` "requires every site's reported release" (line 509) [measured], so a new entry format can be enabled on Thursday. The lifeboat "releases trail site releases by a week" (line 516) [measured]. Until the following Monday it runs N and meets entries it cannot decode.
- **Effect:** The feed stalls, or it skips entries, and the design says neither [measured]. A Cloudflare-wide outage that week finds the lifeboat days behind, not 30 s [inferred]. This is the same unreadable-shape incident class that line 393 removes for the sites.
- **Fix:** Lifeboats report their decoder release to the `Release` group, and `formats:enable` requires every registered lifeboat to report N+1 as well. Alternatively, split the lifeboat bundle into a decoder that rolls with the sites and a serving binary that trails by a week. Either way, a lifeboat that hits an undecodable entry halts and alarms; it never skips.

### F10. Major. During a Cloudflare-wide outage, the documented client steps fail on authentication

- **Rests on:** lines 25 to 30, 240 to 243, 338, 517.
- **Walk:** The BYOC developer runs line 28's `git -c url."https://lifeboat.acme.example/".insteadOf=...`. git now asks its credential helper for `lifeboat.acme.example`, a host it has never stored [inferred]. GCM would start a device flow, and creating a token is a write, which the lifeboat answers with 503 (line 29) [inferred]. `GH_HOST=lifeboat.acme.example gh pr list` (line 29) fails too. gh keeps only authenticated hosts and reads enterprise tokens from `GH_ENTERPRISE_TOKEN` or per-host login (`research/clients.md` finding 5) [measured], and `gh auth login` for that host is again a write [inferred].
- **Token scope:** For the lifeboat to accept existing tokens, it must hold the `Tokens` shards. Those are sharded by secret hash per home family (lines 240 to 243), not by org, so an "org-signed export token" (line 517) cannot select them [inferred]. That works for a single-tenant BYOC install that exports everything. A managed Enterprise lifeboat (line 542) would receive other customers' token hashes [inferred].
- **TLS:** The lifeboat host needs a certificate that Cloudflare's edge does not provide. The design names no issuer or renewal schedule [measured].
- **Fix:**
  - Add `tributary lifeboat enroll`, run by each developer while Cloudflare is up. It writes the existing token into gh's hosts file for the lifeboat host and adds `credential.https://lifeboat.acme.example.helper` delegating to the main host's credential. Lines 28 and 29 then show the full snippet.
  - State that a BYOC lifeboat exports every `Tokens` and `Identity` shard. A managed lifeboat accepts only tokens minted for it, by org.
  - State the certificate: ACME DNS-01 against the zone, renewed 30 days before expiry.

### F11. Minor. "At most about 30 s behind" is not a bound

- **Rests on:** lines 28, 76, 250 to 251, 338, 414, 447.
- **Walk:** The feed is a Queue consumer that writes `ChangeBatch` objects to R2 every 10 s (lines 76, 250) [measured]. On 2025-06-12, Queues "were unavailable" for 2 h 28 min while DO commits mostly succeeded (`research/cloudflare-uptime.md` finding 9) [measured]. On 2025-03-21, R2 writes failed 100% for 67 min (finding 8) [measured], while inline pushes kept committing (line 408).
- **Effect:** Either event leaves the lifeboat an hour or more behind while writes are acknowledged [inferred]. A provider-wide outage that follows, or a recovery from line 447, sees that lag, not 30 s.
- **Fix:**
  - Restate "typically about 30 s; lag is published".
  - Have the lifeboat serve its export cursor age in `/api/v3/meta` and in an `X-Tributary-Lifeboat-Lag` header.
  - Alarm when the lag passes 5 min.
  - Have the lifeboat fall back to pulling log tails from group leaders through the edge when the R2 feed is stale.

### F12. Minor. The "only Cloudflare technologies" question is asked for the managed lifeboat only, but it applies to every off-Cloudflare part

- **Rests on:** lines 7, 542, 556 to 559, 714.
- **Walk:** The on-premises mode needs Linux, Docker, a load balancer, SMTP, and an ACME or uploaded certificate (lines 556 to 559). It also needs Tributary-written replacements for R2, Queues, Workflows, and Cron (F3, F4) [measured]. The BYOC lifeboat also runs on customer hardware with Docker (line 542) [measured]. Line 714 puts only "a managed lifeboat on vendor-owned hardware" to the operator [measured].
- **My reading:** The operator wrote "entirely cloudflare native. only cloudflare technologies" (`FRAME.md` line 39) [measured]. workerd is a Cloudflare technology, so the words do not exclude it outright [inferred]. They do exclude a production mode whose storage, queueing, workflow, and blob layers are not Cloudflare's [inferred].
- **Fix:** Widen line 714 to one operator question covering every lifeboat and any on-premises mode: "May Tributary run workerd plus Docker on non-Cloudflare hardware, for (a) a read-only lifeboat, (b) evaluation, (c) production?" Recommend yes, yes, no, and record the answer in `FRAME.md`.

## 3. Checked and found sound

- Quorum durability on a local workerd member holds on a single host: WAL with default `synchronous` FULL, `xSync` calling `fsync`, and the commit treated as durable only when SQLite's commit returns (`worker-factory-actor.c++` line 290, `BUILD.sqlite3` lines 13 to 23, `sqlite.c++` lines 2239 to 2244, `actor-sqlite-test.c++` lines 402 to 404) [measured]. So an entry acknowledged by two of three hosts survives one host's power loss [inferred].
- Journey 5's single-host-loss logic carries over to hosts. An entry that reached only the dead leader was never acknowledged, and one that reached a follower survives (lines 323 to 325) [inferred from the protocol].
- workerd can reproduce the SSH outbound-handler shape, using the `tcp` socket and the egress interceptor (`workerd.capnp` lines 149 to 154, 845 to 849) [measured]. It persists DO alarms in SQLite, so cron can become alarms (`alarm-scheduler.c++` lines 51 to 55) [measured].
- Members over Ed25519-signed HTTPS (`SiteLink::Https`, lines 120 to 123) are the right link for hosts that cannot share bindings, because workerd objects never cross instances [inferred].
- The read-only lifeboat with no promotion (lines 667, 695) is correct. It cannot fork a chain [inferred].
- `cost.py` runs and reproduces the table at lines 574 to 576 ($168, $938, $5,845 for the default) [measured: script run].
