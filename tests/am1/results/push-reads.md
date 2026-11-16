# `push-reads`: a push's reads, fewer and together

Branch `push-reads`, off main `87c2cd23`. No hosted run yet; B0-32's rerun, with this branch's `server-timing`, is the
hosted check.

## Why

The "after the run" notes for B0-32 m3fr7 in `tests/am1/results/lm1-rows-m3f.md` list the reads on a hot push's path, in order. The read path:
- read the repository group twice (access rows, then agent rows);
- read the install's `Authz` twice;
- ran the token read after the name resolve;
- probed the group before every commit, including the re-commit after `Stage`.

Two of the five traced pushes spent 17.3 and 10.9 s before validate.

## The change

- **Design.** `design/DESIGN.md`, journey 1, step 1. DESIGN stays at 2,200 lines.
- **Item 1, one rows read.** The read gate's push read is `{"rows": {"access": true, "agent": true}}`, and `push_context` reuses it. An older member answers access alone without `"agent": true`, and the edge then reads the agent rows separately.
- **Item 2, concurrent starts.** `authenticate` runs beside `resolve`, and the install `Authz` starts once the token names a caller that reads it. `install_authz` reads once per request.
- **Item 3, the probe.** `commit()` skips `ensure_group` while `graftyard_lookup::knows_leader`. That falls back to the probe after any `NotLeader` or after a reply from an older term.
- **Item 4, Stage first.** A group whose leader answered `Stage` to this isolate within `HOT_WINDOW` stages an admitted push's inline pack over `INLINE_MAX_HOT` before its first commit. "Recently seen hot" is that `Stage` answer within 5 s; the mark lapses on its own, and nothing else sets it. A cool group's push keeps its inline pack.
- **Item 5, push timing.** A test-faults edge answers `receive-pack` with `server-timing`: `resolve`, `auth`, `install`, `gate`, `context`, `validate`, `admission`, `commit`, `stage`, and `total`.
- **Decision.** `decisions/DECISIONS.md`, "`push-reads`".

## Checks

- `a_push_reads_its_access_and_agent_rows_at_once` fails on main (`left: Null, right: true`) and passes here.
- `a_commit_probes_again_after_not_leader_or_a_stale_term` checks the fallback (item 3). Before this branch, `knows_leader` did not exist, and an older term's reply replaced the held leader.
- `only_a_group_that_answered_stage_within_hot_window_stages_first` checks item 4's two conditions: a cool group never stages first, and the mark lapses.
- `tests/m0carried/plant23.py`: 10 of 10 caught.
- Full checks on the branch: fmt, workspace clippy, wasm clippy for site and edge with and without test-faults, 1,019 tests passed and 0 failed, Go (server and porcupine), and the three design scripts (each step's full output kept; `/home/user/workspace/fullchecks.sh`).
