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
  4. an `acctcost.py` reading 30 minutes later showing the install idle (under $0.10 an hour).
- Anything that writes Durable Object rows in a loop is a bug to note and fix.
