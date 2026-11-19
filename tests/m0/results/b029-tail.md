# b029-tail: B0-29's commit tail, the freeze options, and the merge's slow tier read (2026-11-19)

Code only, no hosted run, no deploy. Branch `b029-tail` from main abc13a0a. The traces are B0-29 run 7
(`tsave-m2bb29h`, a1 `mwmg22a2`, load 18:24:33–18:34:33Z, 2026-11-18), A-B-A row 1 (`tsave-m2bb29j`, a1 `mwmsyyry`,
00:26:30–00:31:30Z) and the within-network control (`tsave-m2bb29n`, a1 `mwmxbbb8`, 02:27:16–02:37:16Z), all in
`tests/fanout/raw/` and `tests/am1/raw/` (`tests/am1/results/b029-m2b.md`). Gate line: commit p99 600 ms, 660 at 90%;
`/stats` measured 817, 717 and 781.

## 1. Where the commit p99 goes [measured]

`tests/m0/cuttrace/bycut.py TSAVE FROM TO` matches each `/commit` arrival (trace kind 25, route 3) to the first cut at or
after it (kind 29), that cut's entry (the first append, kind 15, at or after it) and the entry's apply (kind 4), on the
leader's raw clock. `tests/m0/cuttrace/attr.py` is the cruder lower bound (arrival to the next entry appended after it).

| | run 7 | row 1 | control |
|---|---|---|---|
| modelled commit p99 (`/stats`) | 747 (817) | 710 (717) | 734 (781) |
| cuts ending `Empty` | all | all | 2,755 of 2,755 (`cuts.py`) |
| tail share: waiting for the next freeze | 45% (mean 473 ms) | 37% (331) | 39% (332) |
| tail share: own round | 27% | 40% | 26% |
| tail share: past the apply (deps read, decide) | 15% | 8% | 21% |
| tail share: frozen batch held behind the entry in flight | 13% | 16% | 14% |
| two rounds alone (in flight + own), p99 | 587 | 603 | 570 |
| commit minus two rounds, p99 | 350 | 310 | 345 |

- The 30–70 ms between the model and `/stats` is the admission path before the trace sees a command.
- No queue: every cut took the whole queue, and `queue:slot` p99 is 427–489 ms, about one cycle. B0-33 m54 is a real
  backlog (`queue:slot` p99 4,023 ms, `tests/am1/results/b033-m54.md`), so a fix to the leader's admission queue does
  not reach B0-29's tail, which is the freeze schedule plus two rounds.
- The far voter does not matter: the near voter made quorum on 2,806 of 2,807 of the control's entries.
- The tail's own entries are larger (bytes p50 34–91 KB against 28 KB for all), and their rounds are modestly longer
  (p50 174–259 against 160 ms). Their leader confirm is p50 50 ms, normal.
- Colos (LAX only for the sites), leader changes (0) and loop lag (0) are ruled out for these rows.

## 2. Item 4: a later freeze against a late join

The invariant both must keep (`design/DESIGN.md`, "Shape", "Invariants", and the write path): one entry in flight;
`decide` sees the fold plus the earlier commands of its own entry and the `deps` folds the leader read once through
`fresh`, after the commands were admitted (`decisions/DECISIONS.md`, "`read-ahead`", the flip).

How the leader runs a frozen batch today (`crates/raft/src/member/leader.rs`): `cut_early` cuts it, `hold_batch` keeps
it in `Lead::deciding`, and `ask_deps` asks its reads. Nothing is decided when a read answers (`deps_read` only pumps).
When the entry in flight commits and applies, `decide_held` recomputes the batch's chains on the new fold, asks a re-read
for any new chain, and `decide` runs each command in order on that fold and appends the entry.

### (a) A later freeze: it is already the code, and no reachable timing moves p99

`cut_early` fires the first frozen batch at the in-flight entry's append + `round_ms` − `deps_ms`. `round_ms` is the
moving average of append to commit (it is sampled at `advance_commit`), so that time is already the expected apply less
the measured read. What (a) could still change is the estimate: a quantile instead of the mean, the up-to-100 ms lag of
the ticker that evaluates it, or anchoring a second frozen batch on the first one's expected apply.

The replay (`tests/m0/cuttrace/freezesim.py`, measured arrivals, rounds and read-plus-decide samples; three seeds) on the
three traces:

| freeze rule | run 7 p99 | row 1 p99 | control p99 |
|---|---|---|---|
| main: append + mean round − mean read | 722 | 839 | 725 |
| 0.6 or 0.8 × mean round − mean read (earlier) | 743–749 | 872–891 | 739–751 |
| mean round − 1.5 or 2 × mean read, or at the append | 748–755 | 866–904 | 738–749 |
| a read every 25–75 ms of the flight, the latest answered used (5–8 reads an entry) | 677–693 | 746–748 | 673–689 |
| oracle: apply − this entry's own read (unreachable) | 673 | 745 | 660 |

- No freeze rule that waits for one read beats main. The unreachable oracle takes 49–94 ms off, and the rolling read,
  which costs 3 to 7 times the dependency reads, takes 29–93.
- The replay overshoots row 1 (839 against `/stats` 717; it is the 5-minute row, and its first 30 s carried 21 of its 23
  tail entries), so read row 1's column for direction only.
- Cost under B0-33's backlog: none, because nothing changes. Today a full queue already takes one capped slice at the
  same time (`cut_early` has no backlog branch, and the cap binds either way), and the cycle is set by apply and
  `decide_held`, not by the freeze.

So (a) as a timing change is not worth a `crates/raft` change: merge-tail found the same on run 5 (no reachable rule
more than 14 ms off p99, `tests/m0/results/merge-tail.md`).

### (b) A late join: only with the joiner's own read, and worth about 60–100 ms

- **Who may join.** A command admitted after a batch froze may join that batch until `decide_held` runs, only the newest
  (back) frozen batch, so a later-admitted command never goes ahead of a newer batch. It goes through the same lanes,
  tokens, ENTRY_SOFT and ENTRY_MAX as a cut, counted on top of the batch's bytes; a batch already cut at a cap (`Soft`,
  `EntryMax`, `AgentTokens`) takes no joiner.
- **Its read.** A joiner whose `deps` name any chain asks its own `DepsWanted` at admission, for its chains. The batch's
  read was asked before the joiner was admitted, and a revocation acknowledged in between would not bind it: that is
  BF-15/BF-27's "100% of commands decided afterwards get 401", DESIGN's "after the batch's commands were admitted", the
  read-ahead flip, and the test `a_command_admitted_after_the_early_cut_waits_for_the_next_entry`
  (`crates/raft/tests/deps.rs`). So a joiner that only reuses the batch's read cannot join. A joiner with no deps needs no
  read; B0-29 has few, since pushes, issue commands and merges all have deps.
- **When its decide runs.** In `decide_held`, after the entry in flight applied and after every earlier command of the
  batch, on that fold, as any command of the batch. Each chain passes `decide` the newest read the batch holds for it.
  That read was asked after every command of the batch was admitted, so it is valid for all of them, and the entry
  still sees one fold per chain.
- **It never delays the append.** If a joiner's read has not answered when the batch would otherwise be decided, the
  joiner leaves the batch and stays first in the queue for the next cut. A joiner that gains a chain on the post-apply
  fold is treated the same way, rather than holding the batch for a re-read.
- **What changes in the code.** `Deciding` tracks several outstanding reads and each command's admission time against
  them (today one `id`, overwritten by `ask_deps_for`). `CutNote` gets a join note, and `CutEnd` a new value appended at
  the end (append-only). The simulator's `deps_out` assertion narrows from "every request admitted before the entry's
  first read" to "every request admitted before each read it was decided on".

The replay (`tests/m0/cuttrace/latejoin.py`, freezesim's loader; a joiner's read is a sample of the same read times):

| | run 7 p99 | row 1 p99 | control p99 | joined |
|---|---|---|---|---|
| main (replay) | 722 | 839 | 725 | 0 |
| (b), each joiner's own read | 663 | 732 | 643 | 14–15% |
| (b), one re-read per batch, 25–100 ms after the freeze | 684–706 | 794–843 | 683–696 | 4–5% |
| (b), each joiner's own read, with reads halved | 576 | 669 | 551 | 10–11% |

- Predicted `/stats` commit p99 with (b), `/stats` less the replay's change: run 7 817 − 59 = 758; row 1 717 − 107 = 610;
  control 781 − 82 = 699 [inference]. That is under the 660 line on row 1 only.
- Cost: one dependency read per joiner with deps, about 0.15 more reads a command on B0-29 [inference]. That is load on
  each owner's `Authz` leader, which the 2026-11-12 decision's flip names as the thing to watch.
- Cost under B0-33's backlog: about none [inference]. A full queue cuts at a cap, and a capped batch takes no joiner,
  so no joins, no extra reads, and no longer cycle.

### What to build, and how it is checked

Neither option alone takes run 7 or the control under 660 by the replay. (a) gives nothing measurable. (b) gives
59–107 ms for one more dependency read per joiner and a change to the simulator's admitted-before-read check. The rest
of the tail is the two rounds (p99 570–603) and the read past the apply. If (b) is approved, the plan:

- Design first: DESIGN's write path ("A command joins the next entry ...") and journey 1 gain the join rule and the
  joiner's own read. A DECISIONS entry records the decision, why, what it beat ((a), the shared re-read, reusing the
  batch's read), the evidence (this file) and the flip (a hosted B0-29 whose commit p99 does not fall by about 60 ms, or
  `Authz` dep read p99 rising more than that).
- Tests in `crates/raft/tests/deps.rs`:
  - a joiner with its own read joins and decides after the batch's earlier commands;
  - a joiner whose read answers after the apply rides the next entry;
  - the existing test, which stays: a joiner without its own read never joins.
- Plants (mutants the tests must catch): reuse the batch's read; join an earlier frozen batch; use a joiner's late read;
  hold the append for a joiner; join a capped batch.
- The long sweep: `RAFT_HISTORY=... RAFT_SEEDS=2000 RAFT_STEPS=10000 cargo test -p graftyard-raft --release --test sim
  long_sweep -- --ignored --nocapture`, with a joins counter that must be above zero under HARSH. Then
  `tests/m0/porcupine -seg 2000` on the history, `.gray` and `.live`. Both are recorded here, with the full checks.

## 3. Item 5: the merge's slow `tier_ahead` is the edge's leader walk, not the leader [measured]

`tests/m0/cuttrace/tierahead.py A1ROW TSAVE MERGES` pairs a1's `mergeSteps` (`server-timing`) with the edge's merge
requests from Workers Logs (`tests/m0/cuttrace/mergewl.py`, kept in `tests/m0/cuttrace/merges/`; start = end − wall).
It lists the merge-rows `/read`s (query `rows`, code 3) that reached a voter from the edge isolate inside
`tier_ahead`, with their `SentBy` step (kind 27).

| | run 7 | row 1 | control |
|---|---|---|---|
| merges paired | 77 of 78 | 52 of 53 | 70 of 72 |
| `tier_ahead` ≥ 400 ms | 1 (978) | 1 (928) | 3 (1,148, 1,155, 2,168) |
| of those, walked the followers first | 1 | 1 | 3 |
| `tier_ahead` < 400 ms with any walk | 0 of 75 | 0 of 50 | 0 of 67 |
| `tier_ahead` p50 | 58 | 71 | 70 |

- **Each slow merge's walk.** The rows read went to s1 (+117–163 ms) and s2 (+288–362), both `SentBy::Walk`, before it
  reached s0 (+399–634) as `SentBy::Leader`. The 2,168 ms merge walked s1, s2, s0 twice, the second time 1.0 s after the
  first.
- **The leader was fast.** In the control, the leader's own `/read` took at most 45 ms in Workers Logs (wall) around
  each slow merge, and its confirms then were at most 51–58 ms.
- **Not the leader's confirms, a dump rebuild, or LM-1's read gate.** No whole-tier dump hit the leader nearby, and
  no `/suspect` was reported in the control's load. The leader's `/read` gate (`gate`, `READS_MAX`) sheds with an
  answer the trace would show as a retry on the same member, and none appears.
- **Why the isolate walked [inference, open].** `SentBy::Walk` means the isolate held no cached leader for the chain
  (`call_opt`, `lookup`: `Leader` only when `cached_leader` names the member). The isolates had been serving `read_spread`
  reads of this chain for 30 s, mostly to learners, which learn the leader only from the learner's `meta.leader`
  (`learn_leader_from_follower`). That sets `(term, leader)` from any answer at a term at or above the held one, including
  `leader: null`, so one learner answer that names no leader at the current term clears the cache [inference]. The
  trace does not record the learners' `meta.leader`, so this is not shown. Each follower's answer also did not send the
  walk straight to s0, which `lookup` does when a `notLeader` names the leader. Why is open: the edge's lookup notes
  are not traced for reads.
- **Candidate fix, named and not built.** `learn_leader_from_follower` keeps a held leader when a follower's or
  learner's answer at the same term names none. Separately, `tier_rows`' merge read could start at the leader the
  isolate's last commit reached.
- **What it means for the merge figure.** Capped at 400 ms, `tier_ahead` would take merge p99 from 1,782 / 1,596 /
  2,862 to about the resolve-bound 1,393 / 1,342 / 1,759, which is still well over the commit stage (682–924).
- **For LM-1's shed-gate logging.** The 1–2 s is in the edge's lookup, not the leader's gate, so logging the leader's
  sheds will not show it.

## Files

- `tests/m0/cuttrace/bycut.py`, `attr.py`: the attribution (section 1).
- `tests/m0/cuttrace/latejoin.py`: option (b)'s replay (section 2).
- `tests/m0/cuttrace/tierahead.py`, `mergewl.py`, and `merges/merges-m2bb29{h,j,n}.jsonl`: item 5 (section 3).
- The commands, from the repository root:
  - `python3 tests/m0/cuttrace/bycut.py tests/fanout/raw/tsave-m2bb29n.jsonl.xz 2026-11-19T02:27:16Z 2026-11-19T02:37:16Z`
  - `python3 tests/m0/cuttrace/latejoin.py tests/fanout/raw/tsave-m2bb29n.jsonl.xz 1795055236000`
  - `python3 tests/m0/cuttrace/tierahead.py tests/am1/raw/a1-b0-29-mwmxbbb8.jsonl tests/fanout/raw/tsave-m2bb29n.jsonl.xz tests/m0/cuttrace/merges/merges-m2bb29n.jsonl`
