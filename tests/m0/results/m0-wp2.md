# M0-WP2 and M0-WP3: Raft prototype runs (2026-10-28)

The prototype is `src/raft/` and runs as `graft-raft-a/b/c` and `graft-raft-edge`. These scripts are separate from the `graft-site-*` bed, so B0-15 was not touched. Groups use `Tri` hints (enam, weur, apac), and groups named `na3-*` use `Na3` (enam, wnam, weur). Raw runs are in `results/raw/`.

| Row | Result | Gate | Verdict so far |
| --- | --- | --- | --- |
| B0-01 | Leader commit p99 was 399 to 690 ms over 15 one-minute runs at 81 to 100 commands/s; 14 of the 15 runs were over 400 ms. Median p99 491 ms, p50 172 to 370 ms (`results/raw/b0-01-*.json`). Rerun on the final member code: p99 433, 468, and 481 ms (`results/raw/b0-01-final*.json`). Over all 23 one-entry runs: p99 399 to 690 ms, median 491 ms, 19 at or under 600 ms | Pass at most 600 ms, fail over 1 s (restated by the operator, 2026-10-28; was 150 ms and 400 ms) | **Pass in 19 of 23 runs, none failed.** Under the old gate it failed, and flip condition 1's old 400 ms clause fired (below) |
| B0-02 | At 300 commands/s offered: 280/s carried, p99 749 ms. Offered 600: 481/s, p99 1,978 ms. Offered 1,000: 573/s, p99 2,460 ms, 441 commands per entry. No run failed a command (`results/raw/b0-02-w5-*.json`). Rerun at 300 offered against the restated 600 ms line: 159, 276, and 266/s carried, p99 959, 750, and 1,288 ms, 60 to 82 commands per entry, 0 failed (`results/raw/b0-02-g600-*.json`) | At least 300/s at p99 at most 600 ms (was 150 ms); fail under 100/s | **Between pass and fail.** Throughput is well above the fail line; at 300/s the entries grow to about 80 commands and p99 passes 600 ms |
| B0-03 | 13 runs per mode at 99 to 106 ops/s after the harness fix (see "B0-03"). Failing calls: write and read RTO 11.0 to 12.9 s, one run over 12 s. Blackholed calls: 11.0 to 12.5 s, four runs over 12 s. 0 failed operations in all 26 runs (`results/raw/b0-03-*.json` from 07:26). Rerun on the final member code (after the B0-06 fixes), three runs per mode: failing calls 11.1 to 11.9 s, blackholed 11.3 to 12.2 s, 0 failed (`results/raw/b0-03-*-20261028T1125.json` to `T1130`) | At most 13 s at p99 in both modes (restated from 12 s by the operator, 2026-10-28); fail over 60 s | **Pass**: the worst run is 12.9 s |
| B0-11 | **48 hours on the fixed code: Ok** on 5,742,736 operations with intervals widened 100 ms for clock offset between client colos, under 5,729 resets and 2,342 transfers; on the recorded clocks 72 cross-colo reads fail by 27 to 94 ms, all consistent with skew, none same-colo, and no early ack on one clock (see "B0-11: the 48-hour run"). Before the fix, **4 hours on Cloudflare: Illegal** on 475,301 operations, from a ticket-reuse bug after a leader reset, now fixed in the design and the prototype (see "B0-11: the 4-hour run found a linearizability violation"). Earlier, 2 hours on Cloudflare: Porcupine Ok on 270,040 operations, 0 failed, from load clients pinned in ATL, LHR, and SIN (12 logical clients) through a service binding, while the orb ran 243 member resets (half on the leader), 186 transfers (54 abandoned mid-way), 5% dropped AppendEntries replies, and 5% failed member-to-member calls. 89 terms, 52,370 entries (`results/raw/raft-chaos-cf-long-1.json`; the fault plan was replaced for B0-03 and B0-06 runs during it, and one deploy reset every member, as logged in `results/raw/raft-chaos-cf-long-1.started`). Locally, five more runs of 240 to 300 s with learners and gray faults: Ok on 28,133 to 46,706 operations each. A planted mutant serving stale reads was rejected | 48 h, zero violations | **Pass** on the fixed code, with 100 ms clock allowance across client colos; fault coverage on C thin (F36) |
| B0-26 | Every member of 4 groups reset at random inside 60 s under 100 writes/s, with `suspect` reports labelled from two colos: every group committed a new write within 0.2 to 0.9 s of its own last reset, 0 failed operations. 8 groups: within 1.2 s. All 12 members reset inside 0.9 s: within 1.1 s. Rerun on the final member code (after the B0-06 fixes): within 0.2 to 0.4 s (`results/raw/b0-26-*.json`) | Every group within 12 s of the last reset | **Pass**, four runs |
| B0-06 (M0-WP3) | 30% of member calls to and from site A failing for 120 s: every group's leader on A tripped `STEP_DOWN` on its per-follower error rate and transferred within 1.5 to 2.7 s, one leader change each, 0 failed operations. 30% of A's storage writes failing (each resets the member): every group moved within 11.7 to 13.1 s by election after the resets; one group changed twice. Failing only the edge's calls to A (a gray region the leader cannot see): every group moved within 0.6 to 12.2 s, one change each, after three fixes to the two-colo `suspect` path. A rerun of the first two modes on the final code: 2.4 to 5.0 s and 14.1 s, one change each (`results/raw/b0-06-*.json`) | Every group moves within 20 s, at most two changes an hour | **Pass** in all three modes, 2 minutes each, not an hour |
| B0-18 (M0-WP3) | 16 learners at 1,000 reads/s each, 16,000 reads/s, 0 failed, three runs: leader at 171 to 180 req/s, learner read p50 153 to 173 ms and p99 683 to 743 ms (`results/raw/b0-18-cf*.json`) | Leader under 700 req/s, learner p99 at most 1 s (restated by the operator, 2026-10-28; was 150 ms) | **Pass.** Under the old gate it failed on learner p99; the leader-load half passes with 4x margin |
| B0-25 | With the edge held to the design's budget (each commit-path call fails at most 1 + `COMMIT_RETRIES` times): 97.0% and 96.3% of 300 pushes succeeded within three attempts before `STEP_DOWN` existed. With `STEP_DOWN` (B0-06), 14 runs gave 79.3% to 98.3%, because leaders moved under faults that hit every site alike. With the hold described below, nine runs gave 92.3% to 98.7%, median 95.0%, 61 to 73% on the first attempt (`results/raw/b0-25-*.json`) | At least 92% (restated from 96% by the operator, 2026-10-28); fail under 84% | **Pass**: all nine runs on the final code are at 92.3% or more |

## B0-01: why commit p99 is about 450 ms, and what is fixed

The p99 is set by the network, not by this prototype's code. Three measurements show it.

1. **The prototype adds nothing measurable around the round.** Each entry's timeline has creation to send at 0 ms (p99 0) and quorum ack to commit at 0 ms (p99 0). The leader sends AppendEntries before its own write and writes in the next macrotask, so its own confirmation (about 21 ms) overlaps the round. A follower's `serveMs`, which includes its own confirmed write, is p50 4 to 52 ms.
2. **One round to the nearest follower is p50 107 to 189 ms and p99 283 to 473 ms under load.** An idle, sequential ping from the leader's colo to the weur member measured p50 135 ms with p99 151 to 216 ms and max 162 to 498 ms. The same ping to apac measured p50 258 ms and p99 1.5 to 1.8 s (`results/raw/raft-ping-tail-*.json`). The design assumed 80 ms [guess] for one enam to weur binding call. Members land in IAD, EWR, ORD, ATL, or MIA and in CDG, AMS, LHR, MAD, or LIS, so the pair varies by group.
3. **One entry in flight makes commit latency about 1.5 rounds.** A command that arrives while an entry is in flight waits for that round and then its own. With load paced at 10 commands/s, commit was p50 209 ms against a round of p50 109 ms. A simulation that replays the measured ping distribution with one entry in flight at 100 commands/s gives p50 204 ms and p99 302 ms with zero storage time. That is twice the 150 ms gate before any write cost.

`Na3` (enam, wnam, weur) did not help: p99 419, 1,471, and 722 ms over three runs, because the fastest quorum still includes a follower 100 to 120 ms away (`results/raw/b0-01-na3-*.json`).

Payload size matters at high load, not at 100 commands/s. An echo between the members cost 96 ms empty, 118 ms at 50 KB, 390 ms at 500 KB, and 533 ms at 1.5 MB enam to weur. At 1,000 commands/s, entries reached p50 480 KB and p99 1.76 MB, which is why B0-02's round grew to 739 ms.

[inference] No implementation of the design as written meets 150 ms p99 on `Tri`, because the gate is less than one round trip at the measured tail. The 400 ms fail line is also below the measured p99 of one entry in flight. What would move it:

- **Pipelining entries.** It would remove the wait for the entry in flight, about 0.5 round at p50. The design rejected it on purpose (`decisions/DECISIONS.md`, "One group per repository, one entry in flight, and group commit"), because `decide` and `verify` would no longer see the fold they check.
- **A lower gate.** About 500 ms p99 for `Tri` matches the measured network. This is a design decision, not a measurement.
- **Candidate 1**, which flip condition 1 names. It acknowledges on the object's own region, so it has no cross-region round on the write path, but it then gives up RPO zero across regions, the reason this base won.

### Two levers tried for B0-01: transport and pipelining

**Transport.** `graft-m0-transport` (`src/transport.ts`, its own script) timed 150 sequential round trips with 2 KB each way between enam and weur or apac objects, by RPC (what members use), by `stub.fetch`, and over one WebSocket opened through `stub.fetch`. Over four pairs the three agreed within 3 ms at p50 and p99, for example ATL to LIS 134, 135, and 132 ms p50, and MIA to LHR 113, 114, and 111 ms (`results/raw/transport-t*.json`). An idle round trip has a tight tail (p99 within 10 ms of p50). The transport is not where the time goes.

**Pipelining.** Outside the design, the prototype lets a group named `plN-...` keep up to N entries in flight, each sent to each follower in its own AppendEntries call. A follower still stores entry k+1 only after k with the same digest, and refuses an out-of-order one, so the leader falls back to its catch-up loop. Porcupine stayed Ok on a 180 s local chaos run at depth 4 (22,309 operations; `results/raw/raft-chaos-local-pl4-1.json`), and the leader did keep four entries in flight. On Cloudflare at 100 commands/s:

| Depth | Runs | Commit p50 | Commit p99 |
| --- | --- | --- | --- |
| 1 (the design) | 3 | 203 to 269 ms | 505 to 677 ms |
| 2 | 1 | 270 ms | 2,228 ms |
| 4 | 3 | 185 to 285 ms | 424 to 578 ms |
| 8 | 1 | 309 ms | 621 ms |

(`results/raw/b0-01-pipe*.json`, `b0-01-pl*-pipe*.json`). Depth 4 is about 70 to 100 ms better at p99 than depth 1 in the same hour, and still over the 400 ms fail line in two of three runs. Deeper pipes make it worse, because more concurrent appends from one object queue behind each other: one invocation runs at most 6 calls at once. The leader's call time minus the follower's own time (`wireMs`, new in `raft-bench.ts`) is p50 92 to 163 ms and p99 153 to 397 ms to the nearest follower under load, against p99 within 10 ms of p50 when idle. The tail is the platform's handling of concurrent calls under load, not the code, the transport, or one entry in flight.

[inference] No change inside the prototype moves `Tri` commit p99 under 400 ms on this platform. Pipelining is not worth reversing the one-entry rule for. What remains is the operator's choice in OPEN-QUESTIONS F2: restate the gate to the measured network, or take candidate 1.

## B0-03

Earlier B0-03 runs were not valid: two edge invocations with 20 workers each offered about 40 ops/s, not 100, and one invocation runs at most 6 member calls at once and queues the rest (measured with the edge's `/fanout`: 6 pings finished in 153 to 172 ms, 30 pings in 142 to 1,122 ms), so queued calls missed `CALL_DEADLINE` inside the edge and looked like a failing new leader. The runner now uses 20 callers with 5 workers each, waits for every isolate to drop a cached plan before and after a run, and checks that site A leads before the fault starts. A failing site also now fails calls already in flight when the fault starts (`Faults.cutAt`), as a lost site would.

The member changed in two ways, both checked with Porcupine locally (31,967 operations, Ok):

- A follower that gets `suspect` with under 0.5 s of promise left holds the report and campaigns when the promise lapses, inside the same call.
- A voter whose promise lapses within 0.5 s answers the vote once it lapses, instead of refusing and making the candidate back off.

The design's 11.5 s figure is the promise remaining (up to 10 s), the 1 s deadline, and two rounds. The runs land at 11.0 to 12.9 s because the measured round is p50 110 to 190 ms and the new leader's first entry waits for one, so the pass line has no margin. A promise shorter than `ELECTION_AFTER` (10 s) would move RTO down one for one, and it is the only large term.

## B0-25

The edge used to retry one operation for up to 30 s, which hid the faults. Now `op(..., strict)` counts failed attempts per commit-path call (`commit`, `outcome`, `read`), gives up when one call has failed three times, and follows a named leader for free at most twice. The runner models a push as three edge operations on three groups (token read, `Authz` read, commit), as the design's arithmetic does. The first version counted failures across the whole operation and got 93.7%. The design's budget is per call, so it was changed.

### B0-25 after `STEP_DOWN`

`STEP_DOWN` was added for B0-06 after B0-25 passed, and B0-25 then dropped. Twelve runs on the code from before the fix below gave 79.3% to 98.3% within three attempts (median 93%, 4 under 90%; `results/raw/b0-25-percall-3.json` to `-10.json`, `b0-25-dist-*.json`). Every run under 90% had a leader trip `STEP_DOWN` and move, sometimes twice in 30 s (polled member logs). With 22% of all member calls failing everywhere, a leader sees about 40% errors on both followers. That is the "own site is sick" signal, but every site is equally sick, so the transfer only adds an election's worth of `NotLeader` answers to the commit budget.

Fix in the prototype and the design (`STEP_DOWN` doc comment): before a `STEP_DOWN` on call errors, the leader asks its transfer target to ping the other follower 20 times (`linkErrors`). If that link also fails over 5%, the fault is not the leader's, so it holds for one window instead of transferring. An unreachable target does not hold the step-down. Locally, Porcupine stayed Ok over a 120 s chaos run, in which one step-down was held (`results/raw/raft-chaos-local-sdh1.json`). On Cloudflare, nine runs with the hold (`results/raw/b0-25-hold-*.json`): 92.3% to 98.7%, median 95.0%, and two of them still logged a step-down because the target's 20 pings happened to fail under 5%. B0-06 `calls` still passed with the hold (`results/raw/b0-06-calls-20261028T1236.json`, worst group 11.8 s, one change each), because there the followers' own link is healthy. The remaining gap to 96% is the fault model: an injected write failure resets the member and fails every call in flight on it, so failures are not independent as the design's arithmetic assumes.

### Fault coverage note (2026-10-30)

The fault injector's plan reader could stay stuck on an old plan, or on none, in an isolate whose plan fetch never settled (`tests/m0/results/m0-wp4.md`, "Audit: fault-plan reader"). For B0-25, client errors name injected faults on every site in all 9 hold runs that carry the verdict, but which isolates applied the plan was not recorded. Per-site fault coverage was also not recorded for the 4-hour `cf-long-2` run (whose Illegal verdict stands, because weaker faults cannot create a violation) or for the 48-hour `cf-48h` run, where lost-transfer rates (A 9.4%, B 9.0%, C 4 of 64) fit injection on A and B and are thin on C.

## B0-18: learner reads wait for one round to the leader

A learner answers a `fresh` read only after a read-index request sent after the read arrived, so every learner read pays one round trip to the leader plus up to `LEARNER_GAP` (50 ms) of batching. Learners on the leader's own site (enam, colos ATL, IAD, EWR, ORD, MIA) read at p50 39 to 108 ms and p99 295 to 693 ms; weur learners at p50 135 to 208 ms; apac learners at p50 245 to 451 ms and p99 up to 1.2 s. At 20 reads/s per learner, an enam learner was p99 119 ms and the others 442 and 965 ms. The design's "learner reads add up to 50 ms of read-index wait" counts the batching and not the round trip.

[inference] The 150 ms p99 holds at best for learners in the leader's region at light load. Learners in other regions cannot meet it with read-index as designed. Options: count only learners in the leader's region against the gate, serve learner reads from a lease the leader grants (a follower lease, which moves the stale-read risk onto clock drift as leader leases already do), or relax the gate.

The prototype's learner pulls committed entries inside its read-index call, instead of the leader pushing them, and `selfLoad` generates reads inside the learner, so the learner's own per-object request limit was not exercised. Locally, chaos with half the gets sent to three learners passed Porcupine on 31,411 operations (`results/raw/raft-chaos-local-lrn2.json`).

## B0-06 and `STEP_DOWN`

`STEP_DOWN` (design/DESIGN.md, Shape, `Health`) runs on the leader over a 10 s window: its AppendEntries outcomes per follower, and its own write confirmation time. The error rate must pass 5% for every follower, because a fault at the leader's site breaks its calls to both, while a sick follower breaks only the calls to itself and moving the leader would not help. That per-follower rule is the prototype's reading of the design; the design says only "commit-path error rate". A tripped leader transfers to the most caught-up follower and records `stepped_down_at`, which pre-vote already refuses for `RESET_QUARANTINE`.

A third mode, `edge`, fails only the edge's calls to A, so the leader sees nothing wrong with its followers and only the two-colo `suspect` rule can move it: the followers stop renewing its promise, its lease lapses, and a follower campaigns. On Cloudflare the first two runs **failed** the row: two of four groups took 23 to 25 s, and after a first fix one group changed leader three times in two minutes. Three gaps, each fixed and rechecked with Porcupine locally:

1. **The gray leader won back the election.** Withheld promises depose it, but as a follower it then got `suspect` reports of its own, campaigned, and the other two granted it because its log was as new as theirs. Now a follower that withheld promises from a leader on two colos' reports also refuses that member in pre-vote for the same 20 s, and a higher term does not end the refusal. The design added this sentence to `suspect` and the gray-region journey.
2. **Pre-vote tie-breaks favoured the gray leader.** The "lower site key wins" rule made B and C stand down for A, which the others refused anyway. Now a member only claims the tie-break when it could win, and a member refused as suspected stops campaigning for the window.
3. **Late reports deposed the new leader.** Edges still reported "a" after B led, and B's promises were withheld on those reports. `suspect` now names the member the caller failed to reach, and a report counts only if it names the follower's current leader. The design's `suspect` already carries the term, which does the same job.

After the fixes, two Cloudflare runs moved every group within 0.6 to 12.2 s with one leader change each (`results/raw/b0-06-edge-20261028T1110.json`, `-1113.json`). The prototype has no `SiteHealth` object yet, so automatic drain (B0-07) is not covered.

With storage-write faults, the leader on A did not trip `STEP_DOWN` before the resets ended its term: each failed write resets the member, which loses its leadership, and the other two elect a leader within about 11 s. One group's new leader later tripped `STEP_DOWN` on confirm p99 621 ms and transferred, its second change.

## B0-11: the 4-hour run found a linearizability violation

The second Cloudflare run (`cf-long-2`, 4 hours, 475,301 operations from ATL, LHR, and SIN, 138 resets, 77 transfers, 5% dropped AppendEntries replies, 5% failed member calls) checked **Illegal** (`results/raw/raft-chaos-cf-long-2.json`). The run is the protocol's to fail: B0-11's fail value is any violation.

**What was wrong in the harness, and did not cause it.** The load clients restarted their value counter every 60 s alarm chunk, so 50,813 (key, value) pairs were written more than once and a read could not name one write. Renaming every write uniquely in time order left four of six keys Illegal. Three reads also returned a value with no recorded write, from a client that the orb's restarts interrupted mid-call; adding each as a write with an unknown result left key k5 Illegal. Porcupine passes k5 only if every operation's interval is widened by 50 ms or one read is removed, and no clock offset between the three client colos from -150 to +150 ms makes it pass. The causality bound across colos is under 191 ms, and no read ever returned before the write it saw was called.

**What was wrong in the protocol.** The members' stored logs (read back with a new `/dump` route; all three identical) show `put(k5, 101.60)` in entry 15521, created 84 ms before the reference time. The LHR client's put returned 19 ms after that entry was created, while every other acknowledged write in the window returned 137 ms or more after its entry, one round to a follower. So the edge answered `ok` for a command whose entry could not yet have committed. The cause is the ticket: a ticket was `(term, seq)`, and a leader that resets and resumes `led_term` keeps the term but restarts `seq` at 1. An `outcome` for a ticket issued before the reset then waits on the new instance's command with the same `seq` and returns that command's result. `run/raft-ticket.ts` reproduces it deterministically on the local bed: three of three runs answered the old ticket with another command's result.

**Fix, in the design and the prototype.** `Ticket` carries `boot`, a random value per object instance, and `outcome` answers `NotLeader` for a ticket from another instance, so the edge re-sends with its request id (`design/DESIGN.md`, `commit` and `outcome`). After the fix `run/raft-ticket.ts` answers `NotLeader` in three of three runs, and a 240 s local chaos run with a reset or transfer every 4 to 8 s checked Ok on 5,797 operations (`results/raw/raft-chaos-local-boot1.json`). The 2-hour run `cf-long-1`, rechecked with unique write names, is Ok on every key, so it did not hit this path. The load clients now put the chunk's start time in every value, and the chaos driver refuses to overwrite a finished run and logs events as they happen, after a supervised driver restarted at the end of `cf-long-2` and overwrote its event log. A 10-minute Cloudflare run on the fixed code checked Ok on 19,030 operations with 22 resets and 16 transfers and no repeated values (`results/raw/raft-chaos-cf-boot-smoke.json`). The 48-hour run (`cf-48h`) started at 18:50Z on 2026-10-28; its driver resumes itself after an orb restart, and anything that changes `graft-raft-*` during it is logged in `results/raw/raft-chaos-cf-48h.started`.

## B0-11: the 48-hour run on the fixed `Ticket` code (`cf-48h`, 2026-10-28T18:50Z to 2026-10-30T18:50Z)

**What ran.** Member code with the `Ticket` `boot` fix and the `STEP_DOWN` hold (`results/raw/cf-48h-src/`), never redeployed during the run. Pinned load clients in ATL, LHR, and SIN, 12 logical clients, 40% reads, the rest puts and compare-and-sets over 6 keys. The fault plan was the four 5% rules (dropped AppendEntries replies, failed calls out of each site), set once. The chaos loop made 5,729 member resets (A 2,323, B 2,292, C 1,114) and 2,342 transfers from a leader, 824 of them abandoned or with a lost reply, and the group reached term 16,336. Four stretches of 68 to 115 minutes on 2026-10-28 and 2026-10-29 had no resets or transfers, because the orb stopped the driver before the chaos loop moved to `graft-m0-chaos`. The load and the fault plan ran through them. Every gap and restart is in `results/raw/raft-chaos-cf-48h.started`.

**History.** 5,742,736 operations, 12 with no reply (each a 30 s client give-up). Latency p50 284 ms, p99 1,211 ms, max 29.0 s.

**The check.** Porcupine on the whole file was killed for memory at 12.7 GB. The checker (`porcupine/main.go`) now has `-seg N`: one key's history is cut after an isolated operation, one that every earlier operation returned before and every later one was called after. Each segment starts from the state that operation leaves. No-reply writes are dropped, which can only make the check stricter. On a 30,000-operation sample, `-seg` agrees with the whole-history check, and it rejects a planted read that returns a value overwritten 3.6 s earlier. `run/history-prep.py` adds a write for each of 2 to 6 read values per key whose writer's row was lost, and optionally widens every interval.

| Check | k0 | k1 | k2 | k3 | k4 | k5 |
| --- | --- | --- | --- | --- | --- | --- |
| Recorded clocks | Illegal | Illegal | Illegal | Illegal | Illegal | Illegal |
| Intervals widened 100 ms each side | Ok | Ok | Ok | Ok | Ok | Ok |

Results are in `results/raw/raft-chaos-cf-48h.check.log` and `results/raw/cf-48h-analysis/`.

**Why the recorded clocks fail: 72 cross-colo reads, and clock skew explains all of them.** `run/history-skew.py` finds 72 reads that are stale on the recorded clocks: a later write, W2, had returned before the read was called, but the read returned the older value W.
- **All 72 are across colos.** ATL to LHR 18, LHR to ATL 6, ATL to SIN 12, LHR to SIN 36. None is a same-colo read, where client and writer share one clock, although same-colo reads had 93,289 chances within 100 ms of a write to hit one.
- **The gaps are small.** W2 returned 27 ms (p50) to 94 ms (max) before the read was called.
- **One clock offset per colo explains every one.** For each stale read there is an offset for ATL, LHR, and SIN that also satisfies every bound from the reads around it (each read of a write from another colo bounds the offset from above). There are no opposite-direction pairs within 10 minutes.
- **The members' log orders W before W2 in all 72 cases.** All three members hold identical entries in every window.
- **The early-ack test, on one clock:** the 4-hour bug showed as an ack before commit. Commands from W2's own client object, which share W2's clock, were never called after W2 returned yet committed in W2's entry or an earlier one. That holds for 0 of 72 W2s and 0 of 5,934 control writes. The same object's reads in the 150 ms after W2 returned saw W2 in 14 of 16, and in the other 2 a newer value.
- **Earlier runs set the bound.** The 4-hour run already measured a causality bound across these colos under 191 ms (above).

**Verdict: Pass [measured].** 0 violations on 5,742,736 operations once each interval allows 100 ms of clock offset between client colos, under 5,729 resets and 2,342 transfers. Every interval that fails on the recorded clocks is cross-colo, under 100 ms, and consistent with skew. The single-clock test that caught the 4-hour run's bug finds no early ack. The row's "zero lost acknowledged entries" clause also holds. All three members store the same 984,514 entries (indexes 2 to 984,515, no gap, identical digests in every 5-minute window). Every one of the 1,814,873 acknowledged puts and successful compare-and-sets is in them, no compare-and-set acknowledged as failed is, and the log's own fold never applies a compare-and-set whose expected value differs (`results/raw/cf-48h-analysis/lost-acked.json`, from `lost-acked.py`).

**Caveats.**
- **The 100 ms widening is the run's assumption,** not a measured clock bound. Clients in three colos have no shared clock, and the M1-WP2 rerun should record each client object's offset to the members at the start and end of each chunk.
- **Fault coverage on site C is thin (F36).** C lost 10 of its 111 transfers (9.0%), the same rate as A (117 of 1,130, 10.4%) and B (83 of 1,101, 7.5%). C's longest stretch with no lost transfer was 21.4 h over 45 transfers, about a 10% chance at a true 5% rate. The run's code records no per-isolate fault counters, and `/stats` after the run carries no `InjectedFault` lines, so which isolates injected stays unknown.
- **The driver crashed twice at the end.** It crashed once writing the history as one string, now written in batches (`run/raft-chaos.ts`). The checker was then killed for memory. Neither touched the run.

## Bugs the runs found and fixed

- Split votes when two survivors hedged at once: a deterministic pre-vote tie-break and a random backoff.
- A hung voter blocked a campaign for the full deadline: vote and suspect fan-outs return at the first grant.
- The fault plan's read sat on the measured path: it refreshes in the background, with a `from` time for every isolate.
- Storage-write faults fired at the start of every call: they fire only before a real write.
- The leader wrote its entry before sending it: it now sends first and writes in the next macrotask, and any reply that reveals a commit waits for that write (output gate).
- A reset leader that resumed its term could resend a different entry at the same term and index: followers match entries by digest.
- A transfer left `led_term` set: it is cleared.
- A group could stall forever after resets: the members agreed there was no leader, every promise had lapsed, and nothing campaigns unless asked. The edge now hedges (sends `suspect`) on a `NotLeader` that names no leader, as it does after a missed deadline. Found by a local chaos run with learners, where 30 s give-ups repeated every 30 s.

## Also seen

- "Subrequest depth limit exceeded" on `/probe`, which goes edge to member to peer. It happens on some fresh groups and not others, and never on the commit path so far.
- A pending `FaultControl` plan with `hang` faults left in an isolate's cache hung test-control calls after a run; `/stats` now has a 10 s deadline per member.
