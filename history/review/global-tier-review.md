I found 8 gaps in the round-3 global tier: one blocker and seven majors. Four of them break guarantees the design states outright: writes keep working after a Region loss, "no" is never wrong, a revocation sticks, and `reconstitute` recovers a lost geography. I read the documents only and edited nothing. Each claim is labelled measured (taken from the text), inferred, or guess.

### 1. [blocker] A lost Region's board makes every survivor "lag" forever, so writes fail closed everywhere
**Step**: us-west-2 is lost for good, as me-south-1 was (DESIGN.md:7). us-west-2 is auth-na's second Region and hosts front and data cells. The same applies when a Paired BYOC install loses Region a.
**Location**: DESIGN.md:205-208, 216-217, 272, 356, 517-518, 1044, 1049, 1117, 1153, 1172; arena/round3/candidate-2/DESIGN.md:108-111, 190
**Gap**:
- **What the text says (measured).** Every member reads every Region's board, and "One that cannot read them for MAX_LAG lags" (208). NS-ACT-1 repeats it: "a member that cannot read another Region's board counts as lagging" (1153). The only exception is a Region whose geography is `Quiet` (216-217, 272). North America never turns Quiet, because us-east-2 writers keep renewing their leases (356). Nothing in the document ever removes a dead Region from the board set.
- **Managed fleet (inferred from 206-207 and 272).** 30 s after the loss, every data cell on every continent fails closed its write-class grants (pushes, merges, API writes), and edges answer 503.
- **Side effects (measured).** The fast "no" path needs every Region's board read within 5 s (216-217), so it is off fleet-wide. `revoke` waits on "every Region's board" (517-518), so it can never return `covered` again.
- **Paired BYOC (inferred).** There is one board per cell Region (1044) and one geography, which cannot turn Quiet while Region b keeps renewing. Losing Region a stops every write in the install after 30 s.
- **Contradictions (measured).** This breaks "Region-loss recovery with no human action" (1172) and Milestone 4's gate that traffic recovers "by health checks alone" (1117). Candidate 2's board is a 2-of-3 bucket quorum, so losing one Region leaves it working (c2:108-111, 190).

**Suggestion**:
- A member whose own watermark is younger than MAX_LAG is never lagging; its snapshot time proves it is current. Compare against boards only when the member cannot read the authority.
- For the "no" fast path, excuse a Region by lease, not by geography. Each board member holds a short board lease and stops answering "no" or granting writes when it cannot renew. Survivors excuse a Region once all its members' leases have expired.
- Add a Region-loss failure row and a drill with a permanently dead Region, for both managed and Paired.

### 2. [major] The board fast path answers "no" about things committed just before the request
**Step**: In Journey 2 step 2, bob forks and immediately pushes through a front that has not applied R2. In the Milestone 2 gate, `gh api user` runs 100 ms after `gh auth login`, through the other Region's front.
**Location**: DESIGN.md:11, 92, 208, 210, 216-218, 272, 527, 1113, 1176; c2:148
**Gap**:
- **The rule (measured).** `absent_allowed` allows `Absent` when every board was read within 5 s and "no row shows a newer read" (216-217). Rows are written once a second (208). `BoardRow` has no field showing the row is newer than `asked_at` (210).
- **The failure (inferred).** A commit made in the second or so before `asked_at` that nobody has posted yet passes the test. Bob gets "Repository not found", and the gate gets 401.
- **What it contradicts.** Line 92 says the front "does not trust the miss". Line 527 says a first token use "waits about 1 s". It also breaks the 1113 gate and the claim that a replica "never answers 'no'… about a fact it may not have seen" (272). The reason given, "no newer fact exists to miss", holds only if every member's read is newer than `asked_at`.
- **Candidate 2 avoids it (measured).** There, a request that arrives after acknowledgment never gets "no" (c2:148).

**Suggestion**: Pick one of two fixes.
- **Strict:** add `read_at` per partition to `BoardRow`, and require a row with `read_at ≥ asked_at + clock_margin` and `newest_read ≤ mine`. Then restate 11 and 956: during an authority outage, unknown names and tokens wait and then return 503.
- **Keep "no" immediate:** give creates (token creation, signup, fork, repository creation) the same coverage wait as `revoke` (516-519), and weaken the stated guarantee to "never no after acknowledgment".

### 3. [major] Quiet and `reconstitute` bring back revocations acknowledged just before a continent cut
**Step**: An EU user revokes a leaked PAT at t0 − 0.5 s, and Europe is cut off at t0. Cross-ocean apply lag is p99 1.5 s, plus 1 s at fronts (904), so outside followers have not applied the revocation.
**Location**: DESIGN.md:205, 216-217, 272, 348, 517-518, 530, 959, 1179; review/round3-judge.md:42; c2:37
**Gap**:
- **At Quiet (inferred).** EU's Regions are dropped from the board check (216-217). "Newest watermark on the boards" (205) becomes the survivors' own, lag drops to zero, and EU-dependent write grants reopen on a view that lacks the revocation.
- **Two answers (measured).** Line 272 also says a cell fails closed what it "cannot see", so the document answers this two ways.
- **After reconstitute (inferred).** `reconstitute` rebuilds from the most advanced survivor's replica (348), so the revocation is gone for good. The leaked PAT works at every non-EU front until it expires.
- **Not stated (measured).** The continent RPO is described only as lost facts (959, 1179), never as access coming back. Candidate 2 acknowledges a write only once it is on two continents (c2:37).

**Suggestion**: Make revocation-class writes durable on a second continent before they return: one conditional MRSC put (for example a `ClaimKey::Revocation` item), which appliers and `reconstitute` apply. Revocations are rare, so the cost is small. Also state that Quiet keeps grants for the Quiet geography's principals and roots failed closed until `reconstitute` finishes.

### 4. [major] Nothing fences the old authority from `reconstitute` when a cut heals
**Step**: Europe is cut off, not lost. The operator runs `reconstitute(Eu, Na, …)` after Quiet, and the link heals hours later.
**Location**: DESIGN.md:280, 315, 329-330, 337-339, 344, 347-349, 356, 996, 1211
**Gap**:
- **No seal (measured).** `reconstitute` writes rows into auth-na "at version + 1" and moves claim homes (348), but nothing seals auth-eu or its `Lease{Eu,*}` items.
- **Two writers (inferred).** When the link heals, EU writers renew through registry quorum (337-339, 356). `AuthTx` checks home against auth-eu's own rows, which still say Eu (329-330). EU resumes writing memberships, grants, tokens, and sessions for the same roots.
- **Interleaved state (inferred).** Appliers order by `version` only (280, 344). EU's unreplicated commits already sit above the survivors' version + 1, so the authorization graph becomes a per-row mix of two writers, with exact ties possible. Only claim operations fail, with `Stale` (315, measured).
- **The design knows a human can't tell (measured).** It says "A human cannot tell a cut from a loss" (1211), and the yearly drill rebuilds into a shadow authority only (996).

**Suggestion**:
- Make `reconstitute`'s first step a registry transition of every `Lease{lost,*}` item, plus a new `Geo{lost}` item, to `Sealed`. Renewal and lease creation then fail for good.
- Order after-images by `(geo_epoch, version)`.
- Define a rejoin procedure that treats the healed authority's unreplicated commits as stated loss, and drill the heal-after-reconstitute case.

### 5. [major] `reconstitute`'s inputs miss token hashes, org secrets, and index cells
**Step**: Rebuild Europe's `Open` roots into auth-na after a real continent loss.
**Location**: DESIGN.md:227-228, 341, 348, 473-474, 481-485, 525, 528, 734, 976, 1067
**Gap**: The inputs are a data cell's replica plus the escrow (348, measured). Three things are missing:
- **Token hashes (measured).** "The replica keeps the metadata without the hash" (473), and token hashes are not in `HomeOnly` (341). No input holds any EU PAT, OAuth, or refresh-token hash, so every EU user's tools stop authenticating for good. Only front projections hold them (227-228), and they are not an input.
- **Org Actions secrets and org webhooks (measured; loss inferred).** They are authority ciphertext (525). The replica holds "org policies without secrets" (1067), and they are not escrowed, so they are lost.
- **Index cells (inferred).** Copied account rows still name dead EU index cells (734). `kid_allowed` requires the index cell to mint (481-485, 528), so App tokens for EU-homed orgs can never be minted again. That makes 976's outage permanent.

**Suggestion**:
- Give every authority table a recovery class in CI (Replicated, Projected, Escrowed, or Lost), and build `reconstitute`'s input set from it.
- Escrow credential hashes and secret ciphertext, or read hashes from a surviving publisher's verified snapshot.
- Have `reconstitute` reassign index cells and record the old cell as the previous index cell for `kid_allowed`.

### 6. [major] One member's bad board row fails closed every cell worldwide, and boards have no failure rows
**Step**: A wave-1 canary front cell, or one node with a ClockBound or snapshot-time fault (202-203), posts `applied` far ahead of real time.
**Location**: DESIGN.md:205-210, 216-217, 279, 914-994, 971, 1002, 1209, 1230; c2:153, 188-195
**Gap**:
- **No bound (measured).** Lag is the newest watermark on the boards minus one's own (205), with no limit on what other members post.
- **Fleet-wide effect (inferred).** One inflated row makes every member lag past MAX_LAG. Every cell fails closed write grants for that geography and edges return 503. Because boards are read across Regions every second, per-front-cell waves (971) cannot contain it, which breaks "the cell is the blast radius" (279).
- **Missing from the table (measured).** The failure table has no row for boards or control buckets (914-994).
- **Unspecified layout (guess on scale).** Per-member objects read by every member is about N² GETs per second, with "every edge as a member" (1230). That is also the cost the MREC board was rejected on (1209).
- **Candidate 2 contains it (measured).** It has board failure rows, and a faulty member loses only its own lease (c2:188-195, 153).

**Suggestion**:
- Drop rows timestamped after ClockBound `latest()` plus the clock margin.
- Treat a lead over MAX_LAG as real only if two front cells corroborate it, or if the reader finds the named `(part, seq)` in the authority.
- Specify the board layout and its cost, and add board rows (bad-deploy, Region loss, and the rest) to "Failure modes".

### 7. [major] In Regional installs, a dr promotion brings back revoked credentials
**Step**: In a self-hosted Regional install (the default, 1037), an admin revokes a leaked PAT and removes a contractor, and `revoke` returns `covered`. Four minutes later the home Region is lost and the dr Region is promoted.
**Location**: DESIGN.md:516-519, 530, 918, 1037, 1041-1043, 1049
**Gap**:
- **Async identity (measured).** The authority and claim table live in `core` (1041-1042), so they ride `dr-apply`'s CDC with a "5 minutes [unverified]" RPO (918).
- **Nothing replays revocations (measured).** Promotion runs only the `ref_log` reconcile and the saga sweep (1049).
- **Access comes back (inferred).** Revocations, removals, uninstalls, and suspensions in the RPO window are undone, and the rebuilt projections no longer contain them. This breaks line 530: after `covered`, no edge or cell honors the removed fact.

**Suggestion**: Before returning `covered`, `revoke` also PUTs a create-only record to the dr Region's bucket. Promotion applies every record past `dr-apply`'s position before traffic moves. State the identity RPO in the Region failure row and in "Tradeoffs accepted".

### 8. [major] Paired installs don't need the MRSC registry, and keeping it rejects valid Regions and adds a failure domain
**Step**: A Canadian BYOC customer asks for Paired in ca-central-1 and ca-west-1. Separately, a Paired install where DynamoDB is impaired in two Regions while DSQL is healthy.
**Location**: DESIGN.md:5, 58, 310, 337-339, 1037, 1041-1042, 1051, 1181; review/round3-judge.md:95; research/byoc-grounding.md#8, #34, #36; c2:215
**Gap**:
- **Redundant (measured).** A Paired install has one geography, and its `authority` schema already lives in three-Region DSQL `core` (1037, 1042). Regional installs already run the `core` claim table behind the same trait (310, 1041). The round-3 judge removed DynamoDB only for single-Region installs and gave no reason to keep it for Paired (round3-judge:95).
- **(a) Rejects valid Regions (measured).** The preflight rejects trios outside MRSC's 15 Regions, and "ca-west-1 and ap-east-1 are in DSQL's sets but not in MRSC's list" (1051). The Canadian trio fails because of the registry alone.
- **(b) Adds a failure domain (inferred).** Writer leases renew through MRSC (337-339), so a DynamoDB fault stops sign-in and every identity write while `core` is healthy. A single geography never needs this fence, because DSQL's own quorum already fences a minority Region.
- **(c) Adds pipeline cost (measured).** A create-once table, `prevent_destroy`, and rollback barriers enter the BYOC pipeline (1051; byoc-grounding#34, #36).
- **(d) Leaves a path undescribed (inferred).** Converting Regional to Paired would move claims between two registry implementations, and the design does not describe it; candidate 2 names `reseat` for this (c2:215).

**Suggestion**: Keep MRSC in managed only. Every install, Regional or Paired, uses the `core` claim table, with lease items in `core` (or no leases when there is one geography). Drop the MRSC-list preflight for installs and the DynamoDB statements at lines 5, 58, 1041, 1051, and 1181.