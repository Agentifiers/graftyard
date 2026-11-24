# B0-33: the user-token line and the Human writers (user-demote)

## What h1A showed

B0-33's gate needs "the user token admitted as `Agent`": a1's fast user, `m3-member`'s plain PAT, writes 100 GraphQL
comments a minute. That is past `DEMOTE_PER_MIN` (60), so the leader should class it Agent.

On merged main dc880f1d, `crate::admission` (`workers/edge/src/lib.rs`) returned `None` for a plain user token. The
edge sent no admission and the member called `Member::commit`, so the leader never saw the actor. h1A's fast user was
never demoted. On a1's side it got 198 answers of 200 and 300 of 503, the same as the Human writers.

The h1A to h3 runs of the set on dc880f1d record the line as **not reachable on dc880f1d; the build skipped
DEMOTE_PER_MIN for plain PATs**. B0-33 is met only once a run on the fixed build reads the fast user as Agent.

## The fix (`user-demote`, `decisions/DECISIONS.md`)

- The edge sends every authenticated user write with its actor, class Human. An anonymous write and a Human task
  command still send none.
- The leader reports each demotion, and the site counts them per actor in `/stats?summary=1` as `demoted`.

## The harness change: Human load spread over five user PATs

This is a harness change, not a FRAME decision.

**Why.** `DEMOTE_PER_MIN` applies to every user token, as DESIGN's agent block says ("its actor, user tokens
included"). Before this change a1 sent all of the row's Human load under the admin's one PAT:
- side:human comments, push:human pushes, and the merge path's push, PR open and merge;
- on h1A that was 693 writes, 166 of them in the busiest 60 s.

On the fixed build that PAT would be demoted, and the Human commit line would measure an Agent-classed writer.

**What changed.**
- a1 (`tests/am1/rest/a1.mjs`, `humanTokens`) sends those writes round-robin, one token per write, over HUMAN_TOKENS =
  5 user PATs: the admin's, plus four org members `a1-h0` to `a1-h3` (accounts 7100100 to 7100103).
- The four members are seeded as `seedRequesters` seeds users and given the org's base Write. Pushes, comments, PR
  opens and merges need only Write; a merge's `admin` flag only matters with `enforce_admins`, which the row does not
  set.
- Their PATs are minted once into `$M3_STATE/a1-human-tokens.txt` (mode 0600).
- The fast user stays alone on `m3-member`'s PAT at 100 a minute.

**Sizing.** The lead's rule: each token's busiest 60 s on h1A's raws stays at or under 45 writes. That is h1A's
`a1-samples` with side:human and push:human at one write each and a merge at three (push, PR open, merge), assigned
round-robin per write:

| Human tokens | h1A busiest 60 s per token | h2A busiest 60 s per token |
|---|---|---|
| 3 | 55, 55, 58 | 55, 56, 56 |
| 4 | 44, 39, 40, 44 | 45, 46, 41, 44 |
| 5 | 36, 37, 33, 34, 33 | 32, 35, 35, 35, 36 |

Four tokens pass on h1A but reach 46 on h2A. Five keep every token at 37 or under on both.

**Guards** (`tests/fanout/demoted.py`, run by `tests/fanout/b033.sh`, reading the row's `leader.demoted` and
`humanActors`):
- **The fast user not demoted:** a fail of the user-token line.
- **A Human writer demoted:** the run is invalid for the Human line, not a fail.
- **A site before user-demote** (no `demoted`): the line reads "not reachable".
