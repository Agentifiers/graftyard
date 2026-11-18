# fanout

Concurrent validate inside one git shard, and `fanout` (BUILD-PLAN AM1-WP1, "Not yet").

- `bench.py IMAGE [--cpus N] [--mode validate|push]`: validates per second for one network in the shard container at
  1, 2, 4 and 8 concurrent pushes. `run-bench.sh` runs the before (`aa9e097`) and after images as an orb service and
  writes `raw/bench.jsonl`.
- `plant.py`: the plants for this branch's new code; each must turn a test red.

## Hosted slot rules (operator, 2026-11-16 and 2026-11-17)

- The only meter is `acctcost.py`, account-wide; read it before and after every hosted step.
- One hosted run at a time across the account, in a slot the operator grants, with an estimate and a stop sent first.
- Every run ends with:
  1. `teardown.py --sweep` (no shard container left running);
  2. `/retire` of every finished test group (`idleretire.mjs`);
  3. **the retired groups' bucket objects deleted** (`purge.py`, which `idleretire.mjs` runs once every member it
     retired answers "not joined"; it checks the group itself and refuses a network with any member joined). A
     retired group's packs left in the buckets make every `*/15` sweep ask its dead group and alarm each pack
     (`decisions/OPEN-QUESTIONS.md`, R25). Only networks the run itself created are purged; the sweeper never
     deletes a group's packs on its own;
  4. `joined.mjs` over every repository the run created, including ones a driver created and then skipped, ending
     "none joined". A test repository left joined is refolded by the site's fold audit once a day (`fold_audit.rs`,
     4 chains each `*/15` run): on `graft-m3f` 40 leftovers from 2026-11-13 to 11-16 wrote 501,211 rows in one
     50-minute stretch of 2026-11-18 (`decisions/OPEN-QUESTIONS.md`, K10). Retiring does not delete a chain's registry
     rows, so the audit's `homed` count stays the same; the joined count is the check. Which tool applies depends on
     the build:
     - a test-faults build (graft-m2b, graft-m3f): `idleretire.mjs` or `retirenets.mjs`,
       then `purge.py`. Both call the test route `/retire`, which a product build does not serve;
     - a product build (graft-m4a, and every customer install): delete the repository through the API, then
       `POST /api/graftyard/v1/admin/repos/{id}/purge` (`workers/edge/src/purge.rs`) as a site admin, with
       `{"early": true}` before `retain_until`, and the second call once `secondPassAt` has passed. It refuses a
       repository that is not tombstoned, so a product install's test repositories are deleted first;
  5. an `acctcost.py` reading 30 minutes later, spanning two `*/15` cron runs, showing the install idle (under $0.10 an
     hour).
- Anything that writes Durable Object rows in a loop is a bug to note and fix.
