I judged the three designs on the rubric and recommend **Candidate A (network-placed cells with a replicated global tier)** as the base. It is the only one that keeps git, merges and API reads free of a synchronous dependency on another store when the global tier or another cell is down, and that matters most for the rubric's base rule. I made no edits. I checked the AWS and GitHub facts against live documentation (links under "Fact flags").

## Scores (1 to 5)

| Criterion | A (`candidate-1`) | B (`candidate-2`) | C (`candidate-3`) |
| --- | --- | --- | --- |
| 1. Names are correct | 5 | 4 | 4 |
| 2. No synchronous global dependency on the hot path | 5 | 2 | 2 |
| 3. Cross-owner features keep the core invariants | 4 | 4 | 4 |
| 4. Identity at global scale | 4 | 3 | 2 |
| 5. One artifact and operability | 4 | 4 | 5 |
| 6. Simplicity and subtraction | 3 | 3 | 2 |

Evidence, one sentence per score. Line references are into each candidate's `DESIGN.md`. All of it is measured from the text unless marked.

**A**
- **A1.** A login claim, its account row and the first membership commit in one `global` transaction keyed by primary key (138, 48). Repository renames leave an `Alias` and owner renames leave `Former` (140). The retirement rule includes the Marketplace clause (141). If the cell crashes after the claim, the applier creates the missing rows (39).
- **A2.** "No request reads `global`" (9), routing is "a pure function over a local file" (254), and the operations that need `global` are listed (421, 329).
- **A3.** The merge holds for "every PR, including every cross-owner PR" (44). Star and watch events are last-writer-wins by `version` (290). Notifications to a cell that is down for more than 3 days are lost (300, 429).
- **A4.** Front cells authenticate from a local projection (178). `revoke` returns once every follower covers the change, within 5 s (185-188). Read load on `global` is fixed by fleet size (203). However, sign-in is a write to `global` and fails while it is down (194).
- **A5.** Single-cell installs run the same applier against a `global` schema in `core` (368, 378). `move_network`'s Flip step is fenced (236-239). A weekly drill cuts the canary cell's path to `global` (414). Moving an account's index cell gets one clause (297).
- **A6.** It adds `global`, front-cell projections, a full `g_*` replica and a full `repo_card` replica in every cell, plus inter-cell logs. The design itself concedes "N-fold storage and apply writes" (424). It does delete the obsolete parts cleanly (382-392).

**B**
- **B1.** A claim is a conditional put on a strongly consistent multi-Region DynamoDB table (MRSC), and it is the commit point of a pending, claim, activate saga with a sweeper (54-55, 147). The retirement rule omits GitHub's Marketplace clause (147).
- **B2.** "A user whose home is down and who has no cached passport cannot authenticate anywhere" (201, 411). Cold routes to forks also go through the owner's home cell (44, 271).
- **B3.** Forks join their network's slot, so `merge()` "runs unchanged" (46). The `OrgAuthority` mirror's lag on member removal is "unbounded while the hosting cell is unreachable" (216). Mirror applies are idempotent on `entity_version` (292-294).
- **B4.** There is no global auth store (149). But a 15-minute passport cache per front-door node is a TTL-driven read on the home cell (42, 167), which is the `research/uptime.md#47` pattern scoped to one cell [inference].
- **B5.** One cell holds every slot in single-cell installs (354). `migrate_slot` uses a CAS on the slot map's generation (125, 264). There is a weekly drill that cuts off a canary cell's home API and a registry-Region drill using AWS Fault Injection Service (404). However, the `Registry` has two backends (359, 369).
- **B6.** It adds a new store type (DynamoDB), the RouteBook, passport caches, revocation feeds and eight mirror relations (205-214). Self-hosted listings "now read mirrors that lag by seconds" (369).

**C**
- **C1.** `NameEpoch` checks plus "quarantine 1 h > cache 50 min" prove that no node misroutes a name (93, 145). But a released login is held for an hour, where GitHub frees it immediately (92, 394).
- **C2.** "One `graph` cluster that every request depends on for first-use authentication and unseen names" (401). Writes survive on cached auth for 15 minutes and reads for 2 hours (315).
- **C3.** "The merge-in-one-transaction invariant holds for every cross-owner PR," but the merge reads a cached `Grant` "at most 5 minutes" stale (41). Stars use `ON CONFLICT DO NOTHING` (56).
- **C4.** Installation tokens are minted through `graph`, "as on GitHub" (64), which is the shared-cluster App-token failure in `research/uptime.md#44`. Authorization is cache-and-confirm against `graph` (169-176).
- **C5.** Single-cell installs collapse `graph`, `net` and `inbox` onto `core` while keeping the typed `Tx<Graph>` and `Tx<Net>` code (349). `move_slot` is fenced (222-248). A weekly drill blocks `graph` from a canary cell (397).
- **C6.** It adds `graph`, a shadow copy of `graph` fed by change data capture (CDC), three `inbox` clusters and network stripes. Cells "hold no canonical row" (216), which rewrites `DESIGN.md`'s cell model rather than keeping it.

## Two strongest ideas and two most serious flaws per candidate

**A**
- (+) The `Lookup::Behind` rule (125-129, 139, 197): a replica never answers "no such name" or "not allowed" until its watermark passes `asked_at`. Denials wait and grants race. This turns a staleness bug into a type error a maintainer cannot write by accident [inference].
- (+) The authorization graph is replicated into each cell's `core`, so `authorize` reads permissions in the same snapshot as the refs (180, 44). This keeps `DESIGN.md:379`'s rule. Read load on `global` is a fixed set of followers (203), which answers `#47` and `#44` structurally.
- (−) Every data cell holds all `g_*` rows and every repository's `repo_card` (285-288, 301, 424). Cell storage and apply writes then grow with the whole service, and a bulk change hits every cell's `core` at once (338) [inference: the opposite of cell independence].
- (−) Its gap-free log and its 50 ms clock margin rest on DSQL detecting write-write conflicts on the partition head rows, which is unverified (260-262, 442-444). Those head rows are also a throughput ceiling.
- Coverage gap: residency places "all its networks, forks of them included" (317), but it never says where a resident org's fork of an outside network goes. B (256, 315) and C (258) both answer this.

**B**
- (+) A minimal global key set (102, 149): logins, the enterprise-managed-user (EMU) shortcode that reserves a whole login suffix, app slugs, SSH fingerprints and verified emails. Claims are commit points with sweepers (147).
- (+) Ids carry their slot bits, so ids route by arithmetic (86-89, 222). `apply_delta` refuses route updates that remove more than 0.1% of entries or move more than 64 slots without a signed override (142-144).
- (−) Authentication needs the caller's home cell (199-201). A push to `c7` fails while `c3` is down for any front-door node without a cached passport.
- (−) `OrgAuthority` is a mirror that authorizes, with unbounded lag while the hosting cell cannot reach the home (216). Org-owned forks route through the owner's home on a cold cache (44).

**C**
- (+) The name-epoch scheme (95-120, 145, 49): a cached route is checked against `repo_head.name_epoch` inside the stripe read the request already makes. This is the most rigorous proof against misrouting after a rename in any of the three.
- (+) `Tx<'_, K>` binds each transaction to one cluster at compile time (231-243). Its honest scale table (9-16) and the CDC-fed shadow of `graph` for canarying schema changes (315) are reusable.
- (−) `graph` is a synchronous singleton for cold authentication, token minting, listings and stars (32, 64, 315, 401). This is GitHub's shared-store shape with a bounded refresh budget.
- (−) Push and merge authorization moves out of the merge snapshot into a cache that can be 5 to 15 minutes stale (41, 171, 402). That is a weaker revocation bound than A's 1 s to 5 s.

**Fact flags**
- B line 9 is correct. MRSC can span "any three-Region configuration across the 15 supported AWS Regions… span North America, Europe, and Asia Pacific" ([AWS What's New, 2026-09-23](https://aws.amazon.com/about-aws/whats-new/2026/09/dynamodb-mrsc-additional-regions/)). DSQL "doesn't support cross-continent multi-Region clusters" ([DSQL user guide](https://docs.aws.amazon.com/aurora-dsql/latest/userguide/what-is-aurora-dsql.html)).
- B's reasoning error: it infers that the namespace authority must span continents (9, 415). A (203) and C (68) both survive a Region loss with a DSQL cluster inside the North American Region set. Nothing in the task requires surviving a continent loss, so the case for DynamoDB as a new store type is overstated.
- A line 441 is answerable now. us-east-1, us-east-2 and us-west-2 are all in DSQL's North American set; that the witness may be any of them is [inference]. Cross-continent pairs are not supported (same page).
- C lines 11-13 match the [DSQL quotas page](https://docs.aws.amazon.com/aurora-dsql/latest/userguide/CHAP_quotas.html): 10 TiB default storage, 256 TiB on request, 100 connections per second not configurable, burst 1,000. C's DSQL paper citation resolves to "Aurora DSQL: Scalable, Multi-Region OLTP".
- B omits the Marketplace retirement criterion. A and C match [GitHub's username reference](https://docs.github.com/en/account-and-profile/reference/username-reference).
- A (434) and C (412) both reject an "owner-home" alternative that puts forks in the fork owner's cell. B's actual design puts forks with their network (B 267). All three converge on that, so the merge invariant does not separate them.
- Research citations `uptime.md#26`, `#27`, `#29`, `#44`, `#47` and implication 22 are quoted correctly by all three (measured).

## Base
A has the highest total and the only top score on the criterion that matters most. Its invariants (`Behind`, a single writer to `global`, the network as the fence) are local rules that a maintainer can extend without new protocols. B's passport coupling and C's `graph` dependency both put a remote store back on the request path. On tie-breaks A is not the smallest, but neither alternative earns the tie.

## Grafts into A
1. **From B, enforcing that residency is not silently bypassed.** A resident org's fork of an outside network is created detached, as a new network inside its residency with objects copied, and a pull request back upstream gets an explanatory 422 (B 256-257, 315).
2. **From B or C, slot bits in ids.** Route `RepoId` by arithmetic to a slot (C uses 4,096 slots, B 65,536). The projection then only needs names plus a slot map, not every repository mapped to a network. Move slots instead of single networks.
3. **From C, `NameEpoch`.** Check it in the data cell's snapshot so a stale positive route after a rename or reclaim is caught inside the serving cell (C 95-120, 145).
4. **From C, the shadow `global`.** Feed it by CDC with replayed production reads, as the schema canary (C 315).
5. **From C, takedowns without CloudFront invalidations.** Use short-lived signed URLs that carry a `content_epoch` (C 301).
6. **From B, `apply_delta`'s removal-percentage and generation guards** for front projections (B 142-144).
7. **From B, the EMU shortcode claim** that reserves a whole login suffix (B 96-98).

## Drops from A
- Drop the `repo_card` replica in every data cell (285-288, 301). Send card events only to the index cells of accounts that list the repository, the way A already routes star events.
- Drop "a JWS from one cell's key verifies at every front" for installation tokens (71, 427). Accept an installation token only if its `kid` cell is the installation account's index cell. Otherwise a compromised cell can mint tokens for every installation.
- Drop the 3-day retention that loses notifications (300, 429). Park a destination's partition instead of expiring it.

## Flip condition
- **The unknown:** can A's `global` change log (64 partition head rows on a two-Region North American DSQL cluster with a witness) keep appends gap-free in commit order, with p99 apply lag under 1 s, at signup-flood write rates?
  - If OCC on the head rows lets a retried append commit out of order, `Behind` can deny wrongly.
  - If throughput caps at a few hundred appends per second per head, `Behind` turns into routine 503s.
  - Either way the choice flips to B, whose global tier holds claims only. C fails criterion 2 regardless [inference].
- **Cheapest experiment, about one day on a scratch account:**
  1. Create one us-east-2 and us-west-2 cluster with a us-east-1 witness.
  2. Run 200 concurrent writers split across both Regions, each doing batched appends that bump one of 64 head rows.
  3. Run a follower polling every 100 ms that asserts no `seq` gaps and that `at` never decreases.
  4. Report commits per second per head, the `40001` retry rate, p99 commit latency, and follower lag.