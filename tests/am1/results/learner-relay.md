# Learner replication off the leader: priced at $0 (2026-11-23)

The lead asked for this after lever A's split (`seal-bytes-design.md`, section 8) found that learner answers are most of
the copies leaving a repository leader. The question is what moving learner replication to the followers would save the
leader per entry, what it would cost in learner lag, and which invariants and design sections it touches. There are two
options: followers relay held AppendEntries to learners, or learners pull entries from a follower. Nothing is built and
no hosted row ran. Tags follow `design/SHARED.md`, "Read this first".

## 1. What learners cost the leader now

[measured, `tests/fanout/learnercost.py` on the committed leader traces and Workers Logs, output in
`tests/fanout/raw/learnercost.out`] A learner's `/raft` call to the leader is priced by each row's plain-slice fit, as
`flushslice.py` does it: `a + b` per call plus `k` per answer KB. A call either carries read-index asks or not (then it
carries the learner's AppendEntries replies), and its answer carries held AppendEntries and the rest. Learner regions
follow `place_learners`' round-robin over the row's `LEARNERS` [inference from `crates/types/src/placement.rs` and the
drivers]: 192 and 195 are enam, 193 weur, 194 apac on mrB's four, and 196 weur, 197 apac, 198 and 199 enam on B0-32's
eight. The read-index round trips agree: enam p50 31 to 167 ms, weur 121 to 266, apac 194 to 332.

Leader ms per entry replicated:

| Row (entries, leader CPU per entry on all routes) | Read-index calls | Append-reply calls | Held entries in answers | Other answer bytes | All learners | Share of leader CPU |
|---|---|---|---|---|---|---|
| mrB, B0-33 (591, 136 ms) | 5.1 | 1.6 | 4.1 (574 KB) | 1.1 | **12.0** | 9% |
| m3fsf2, B0-32 52.5 (20, 128 ms) | 14.7 | 4.5 | 5.1 (684 KB) | 1.1 | **25.4** | 20% |
| m3fg3, B0-32 55.8 (14, 131 ms) | 16.5 | 7.1 | 8.2 (1,012 KB) | 2.2 | **34.0** | 26% |

By region (all learners in it, ms an entry: read-index + replies + held entries):
- mrB: enam (2) 2.5 + 0.7 + 1.9; weur (1) 1.3 + 0.4 + 1.1; apac (1) 1.4 + 0.5 + 1.2.
- m3fsf2: enam (4) 8.0 + 2.0 + 2.5; weur (2) 3.6 + 1.1 + 1.2; apac (2) 3.1 + 1.4 + 1.4.
- m3fg3: enam (4) 8.6 + 2.7 + 3.7; weur (2) 4.2 + 1.3 + 1.6; apac (2) 3.8 + 3.1 + 2.9.

- **The largest part is read-index calls**: 43 to 58% of the learner bill. They are per call: 0.31 ms a call on mrB, 0.44
  on m3fsf2, 0.58 on m3fg3. The entries themselves (held bytes) are 24 to 34%.
- **Half the bill is the enam learners**, on the leader's own region (site 0 is enam on graft-m2b).
- The fit spreads per-call and per-KB cost over every slice that answered learner calls, apply slices included. So
  these are estimates of the learner share, not a measured split of each turn [inference].

## 2. The two options

Both keep a learner's read-index asks on the leader. A read is granted only at an index R that the leader confirmed
after the batch arrived (`fresh`, `read-newest`), and a follower has no lease to confirm one. A learner already counts R
as committed by digest from the answer (`narrow-reads-3`, "Where a far learner's wait goes"), so it needs only the
entries through R from somewhere, not a commit index from the leader.

**Option 1, followers relay.** The leader stops replicating to learners in a region with a voter (weur and apac on
graft-m2b). That voter forwards each AppendEntries it steps to its region's learners, as calls (a learner never calls
it, so `QUIET` doesn't bind), before its own storage confirm (a learner counts toward no quorum). The learner verifies
by `prev_digest` as now and answers the relay, which tracks its next index. The leader no longer holds entries for
those learners, and `append_for_read` stops for them.

**Option 2, learners pull.** A learner in a voter's region sends its `/raft` calls for entries to that voter, which
answers with what it holds past the learner's index. Read-index asks still go to the leader. A variant forwards the
asks through the follower in its own batches; that saves little (below).

**What each saves the leader** (ms an entry, then share of leader CPU; enam learners stay on the leader, because
graft-m2b has no enam follower and a transatlantic relay would add a round trip to the leader's nearest learners):

| Row | Option 1 or 2: held entries + append replies of weur and apac learners | Same, if enam learners also moved (with the lag cost below) | Option 2 variant, read-index forwarded at the learner floor (50 ms) |
|---|---|---|---|
| mrB | 3.2 (2.4%) | 5.8 (4.3%) | +0 (weur and apac ask 11.5 and 12.3 batches a second, under one 20 a second floor) |
| m3fsf2 | 5.1 (4.0%) | 9.6 (7.5%) | +1.5 (27 and 24 a second, held to 20) [inference] |
| m3fg3 | 8.9 (6.8%) | 15.3 (11.7%) | +0 (16.6 and 15.2 a second) |

- **As commit scale:** x ≈ 0.96 to 0.93 on m3fsf2 and 0.93 to 0.88 on m3fg3. That is m3fsf2's line about 284 to 288
  against ×0.63 for 450, and m3fg3 flat (it needs ×0.2) [inference from `b032share.py`'s marks]. On B0-33, mrB's
  leader gains 2 to 4%.
- **Options 1 and 2 save the leader the same.** They differ in the followers' cost and the learners' calls. In option 2
  a learner adds calls to its follower beside its read-index calls to the leader. In option 1 the follower adds one
  call per learner per append, which it already does to no one.
- **The followers can take it.** mrB's weur follower ran 0.11 vCPU over the window (`/raft` 12.9 s, `/entries` 4.2 s of
  23.3 s) against the leader's 0.38. A follower would pay about what the leader pays now for the entries of the learners it
  takes: 1.2 to 3.0 ms an entry each.
- **Neither moves the read-index calls**, which are the largest part. `learner-cadence` cut them from 56.1 to 15.6
  calls a second and was reversed: read wait p99 rose from 1.6 to 4.0 s, and edge 503s rose 19 times (A39).

## 3. What it costs in learner lag

[measured unless tagged] Today a learner gets its entries in the answer to its own next call to the leader (held to
taken p50 0 ms, p90 0 to 117 ms). Each learner steps an AppendEntries every 187 to 284 ms at the median (p90 334 to
1,110), and its read-index calls take p50 31 to 332 ms. `append_for_read` sends the entries through R ahead of each
read answer, so a learner is never behind a voter.

Under either option, a weur or apac learner gets its entries only once its follower has them:
- **The usual case is no worse** [inference]. The leader sends an entry to both followers at the cut. A follower one
  region-hop from its learner hands it over in an in-region call (tens of ms), about when the leader's answer would
  have reached that learner.
- **The tail follows the slower follower.** The followers' AppendEntries calls took p50 286 and 403 ms on mrB, and p90
  847 ms on m3fsf2 and 1,659 to 1,795 ms on m3fg3 (they include the follower's confirm). Commit needs one follower, so
  a learner behind the other one lags with it, by up to the commit-path p90. Today `append_for_read` lets a learner run
  ahead of a lagging follower.
- **Feed polls.** A learner holds polls only while it applied an entry within FEED_BATCH (1 s) of the leader's `at`
  (`design/AGENT-LAYER.md`, "Activity feed"). A learner behind a follower at m3fg3's p90 would stop following and
  refuse its polls, moving them to other learners or the 429 last resort. LF-12 r19 shows what learners lagging together
  cost: 25,350 last-resort polls (`tests/m0/results/m0-wp12.md`).
- **The read-index floor.** A read waits for R's entries after its answer. A follower tail of 0.8 to 1.8 s lands on
  the learner gate (1 s p99, `decisions/DECISIONS.md`, "Measured latency is accepted, and the build continues") and on
  the edge's `ROUTE_LATE` (1 s), which then routes reads away from those learners.
- **A fallback is needed either way** [inference]. When the relay's follower is behind R by more than a bound, or is
  down, the leader replicates to that learner again, as now. That is the code path today, so it costs nothing new, but
  the switch has to be tested.

## 4. What it touches

**Invariants** (`AGENTS.md`):
- *One entry in flight; a follower verifies k+1 only after it applies k.* Not changed, but a relayed learner must hold
  to it from a source that is not the leader: the relay forwards only entries it holds, with the leader's term, and the
  learner checks `prev_digest` as now. A relay of entries the leader later truncated is the case to model. A learner
  applies only through R or a commit it learned, never past it.
- *An event exists iff its change committed.* A learner applies only committed entries; the relay's uncommitted
  entries must not be applied or fed past R. This is the same rule, from a new source.
- *Acknowledge only after the quorum rule; `Acked` only inside `outcome`.* Not changed: learners never count, and a
  relay's acks never reach the leader's `matched`.
- *Read-index linearizability* (`fresh`, `read-newest`). Unchanged in options 1 and 2, since R still comes from the
  leader. The forwarding variant must carry the leader's answer per batch, not the follower's commit, and keep
  `read-newest`'s rule that an answer grants only its own and earlier batches.
- *A site-script change ships through a site day*, and the change sits in `crates/raft/src` (learner progress, relay
  state): the long sweep and Porcupine, with learners modeled in the simulator.

**Design sections to edit first:**
- `design/DESIGN.md`, "Shape", the write path ("sends to both followers at once (and to learners, which never count)")
  and "The read path" (learner read-index batching).
- `design/DESIGN.md`, "Commit and read paths", "Follower read", and "Scale", the "Hot group reads, learners" row: the
  `QUIET` rule and the call-depth climb now apply to follower and learner pairs too.
- `design/DESIGN.md`, "Failure modes", chain groups (instance, region, noisy-tenant): a follower's loss or slowness now
  lags its learners until the fallback.
- `design/AGENT-LAYER.md`, "Activity feed": what "following" means for a relayed learner. That block sits outside the
  shared agent block.
- `decisions/DECISIONS.md`: "Not adopted: pipelining AppendEntries to learners", `learner-hold`, `read-newest`,
  `learner-cadence` (reversed) and `narrow-reads-3`'s `append_for_read`. None has a flip that covers relay, so it needs
  its own evidence. F97 (far learners) and A39 stay open beside it.

## 5. Reading

Relay or pull saves a slow leader 4 to 12% of its CPU an entry, and 2 to 4% on B0-33's mrB. That is the same order as
lever A, for a larger change: a raft change, a new replication source, and a lag tail tied to the slower follower that
the feed and read gates feel. It does not reach ×0.63 alone or with lever A. The read-index calls, 43 to 58% of the
learner bill, stay on the leader in every form, because only the leader can confirm R.

The cheaper lever in the same numbers is the enam learners [inference]: half the leader's learner bill on every row
comes from the 2 or 4 learners on the leader's own region. On m3fsf2 each costs about 3.3 ms an entry. Their number
comes from the row's `LEARNERS` setting (`enam:4000` places four), which is a placement and driver choice, not a
replication design. Whether B0-32's read load needs four learners there is a question for its owner.
