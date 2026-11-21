# inline-cut: the 300 KB cuts, and why `AGENT_INLINE_ENTRY` at 32 KiB waits (2026-11-21, $0)

The orchestrator (T-01a17bda) approved `AGENT_INLINE_ENTRY` 128 → 32 KiB, which `tests/am1/results/leader-cycle.md`
proposed, on condition that the 16 cuts over 300 KB were settled first. This file settles them. It also records why the
32 KiB numbers in that proposal were wrong, and counts the B0-32 pushes in 32 to 128 KiB. Nothing is built. Tags
follow `design/SHARED.md`, "Read this first".

## 1. The 16 cuts over 300 KB are not a bound failure

m64r33's 16 cuts over 300 KB were all on site 1, from 00:14:13.5 to 00:14:18.5Z, the 5 s right after site 1 took over at
00:14:13 [measured, `tests/fanout/raw/tsave-m64r33.jsonl.xz`, kinds 12, 29 and 30, on `b033-m64`]:
- **The entries those cuts became were small.** Each cut joins to the first `AppendQueued` of entry (last index
  appended before it) + `held` + 1. Entries 351 to 366 had AppendEntries bodies of 2,999 to 4,209 B, except 352 (68,501 B)
  and 362 (30,951 B). Each applied with 43 to 53 events and 64 to 68 resolutions.
- **Their cut bytes were reply renders, not packs.** A cut's `bytes` column is the sum of each command's
  `encoded_len`, which for an issue command includes its `render`. `ENTRY_SOFT`'s `soft` counts `stored_len` without it
  (`RepoCmd::stored_len`, `crates/git/src/repo.rs`). The entry stores the events `decide` returned, and they carry no
  render.
  - These cuts held 64 Agent commands at about 12.7 KB each, of which about 550 B was stored (`soft` 33.6 to 54.9 KB).
    That matches an issue command with the owner's `Authz` hex in its render (about 10.3 KB) plus its reply render.
- **Why renders came back:** under `leader-render`, the edge's first send leaves the owner `Authz` out and every later
  send carries the whole render (`commit_issue_rid`, `workers/edge/src/api.rs`). The transfer made the old leader answer
  `NotLeader`, so the edges re-sent to site 1 with whole renders. Edge `authzSource` counts agree:
  - 00:07:00–00:14:12Z: dep 4,980, render 13;
  - 00:14:12–00:14:20Z: dep 55, **render 529**;
  - 00:14:20–00:23:00Z: dep 12,896, render 210 [measured, Workers Logs, field `authzSource`].
- **The bound held.** Over agent-only entries, AppendEntries bytes less `soft` peaked at 194,413 B. That is the
  base64 of 128 KiB (174,762 B) plus each entry's envelope and events. m3fb29 and m3fhs1 never passed 174,762 B.
  Whether 17 m64r33 entries passing it by up to 20 KB is all envelope is [inference].
- **What it costs:** these bytes count toward `ENTRY_MAX` (1.5 MB) and the leader's queued memory, never toward the
  entry or the round. No cut ended `EntryMax` (0 of 602). No fix is needed for the bound, so no fail-on-main test was
  written.
- **Correction to `leader-cycle.md`.** Its "77% of all cut bytes were outside `ENTRY_SOFT`'s count, mostly Agent inline
  packs" mixed these renders into cut bytes. On AppendEntries bodies, which hold no render, the share outside `soft` is
  78% on m64r33. That share is inline packs (as base64) plus each entry's envelope. So the conclusion stands, on the
  right measure.

## 2. The 32 KiB proposal was mis-scaled

`PackRef::Inline` serializes its bytes as a base64 string (`crates/git/src/pack.rs`, `b64`), and the entry codec is
postcard over serde. One `PackRef::Inline` of 30,000 bytes encodes to **40,005 bytes** [measured, a scratch test
through `graftyard_raft::codec::to_bytes`]. So a raw bound of `AGENT_INLINE_ENTRY` puts up to 4/3 of it in the entry.
`leader-cycle.md` compared raw bytes with entry bytes.

Entries over 64 KiB that a raw bound would leave over 64 KiB (each entry's `soft` plus the bound's base64 plus 600 B,
from AppendEntries bodies):

| Trace | Over 64 KiB now | 48 KiB | 32 KiB | 24 KiB | 16 KiB |
|---|---|---|---|---|---|
| B0-29 m3fb29 (3,383 entries) | 843 | 843 | **150** | 15 | 10 |
| B0-33 m64r33 (720 entries) | 305 | 307 | **170** | 107 | 48 |

`leader-cycle.md` said 15 would remain at 32 KiB on m3fb29. The right figure is 150. To get to 15 the bound must be
24 KiB.

**The cost is larger than proposed.** Any Agent pack over the bound stages alone. Journey 9's packs are uniform
10,000 to 70,000 B, plus 5% at 1 MiB (`tests/am1/rest/wire.mjs`, `journey9`):
- at 32 KiB, 59% of agent pushes newly stage (packs of 32,768 to 70,000 B);
- at 24 KiB, 72% do.
Each pays the `Stage` round trip and the R2 place, p50 546 + 638 ms on m3fpc1 (A36). Hosted B0-29 m3fb29 staged 228 of
3,674 commits at 128 KiB.

**In-memory model** (`crates/raft/tests/admission.rs`, release, one constant changed, `STAGE_MS` 500):

| `AGENT_INLINE_ENTRY` | B0-29 commit p99 (first send to ack, staging included) | Staged | B0-33 +90 ms: agent/s, refused, Human p99 | B0-32: within 5 s |
|---|---|---|---|---|
| 128 KiB (main) | 260 ms | 13 | 217.6, 120, 600 ms | 500 of 500 |
| 64 KiB | 670 ms | 39 | 217.6, 115, 600 ms | — |
| 48 KiB | 700 ms | 112 | 217.6, 106, 600 ms | — |
| 32 KiB | **700 ms** (test fails its 600 ms line) | 179 | 217.6, 100, 595 ms | 500 of 500 |
| 24 KiB | fails | — | 217.6, 95, 600 ms | 500 of 500 |

- The model's links are 0.3 ms per KB with no 64 KiB step, so it shows the staging cost and none of the round gain. It
  is not evidence against the change on the hosted round.
- It does show the cost lands on every staged push: a 40 KB push that used to commit in one round now waits 500 ms
  for its stage first.
- Hosted, the leader's `commitMs` (B0-29's gate measure) would not include staging, but the push's end-to-end time
  would, and so would B0-33's 5 s `AGENT_PUSH_BOUND`.

## 3. B0-32 pushes in 32 to 128 KiB

From `tests/am1/results/lm1-rows-m3f-data/m3fgb1-raw/` and `m3fhs1-raw/`, and the generator that wrote them
(`tests/am1/rest/a1.mjs`):
- **Burst pushes: 0 of 500 on each row.** Every burst pack is `randomBytes(256 << 10)` (`planFor`, `b0-32`), already
  over `INLINE_MAX_HOT` (64 KiB) and over 128 KiB. They stage today; m3fhs1 shows 270 with a `stage` stage.
- **Merge pushes: 0 of 26 (m3fgb1) and 0 of 33 (m3fhs1).** Each is `randomBytes(5_000)`.
- **Background journey 9 agent pushes: about 59%,** about 450 of m3fgb1's 762 and m3fhs1's 763. The raw files hold no
  pack sizes, so this comes from the generator's 10,000 to 70,000 B uniform draw, not a per-push count. These are the
  pushes that would newly pay `Stage` plus place on a B0-32 row.

## What would move the round without staging

The bytes that cross the 64 KiB step are mostly base64. Storing an inline pack as raw bytes in the entry (postcard
bytes, not a base64 string) cuts each pack's share of an entry by a quarter and stages nothing. On these traces that
would leave over 64 KiB, with the bound kept at 128 KiB:
- 507 of 843 entries on m3fb29;
- 285 of 305 on m64r33.

With raw bytes and a 48 KiB bound, 128 and 176 would remain. That is a codec and record-format change, on stored
entries and the digest's input, so it needs a format version and the design's say [inference on effort]. It is not
proposed for this branch.
