# BF-72 on graft-m4a

Status (2026-11-25 23:01Z): **r1 passes every line.** The lead gave GO on main `6b48a6dc` once AM2's kill row (done 21:53Z) and LM-1's BF-75+77 edge day (done 22:42:44Z) had left graft-m4a. The window ran from 22:50:24Z (`check` and the cost read) to 23:01:40Z (teardown, the done line). The days and the row spent $0.04 against a $3 stop: `acctcost.py` (prefix graft-m4a) read $0.04 since 22:50:24Z, and graft-m4a's UTC day went from $8.66 to $8.69. Raw logs are in `tests/m4/raw/wp3/bf72/`, and the plan is `tests/m4/wp3/bf72/plan.md`.

## r1: verdict

Run 4521984 of `caller.yml` in `m3org/bf72`, dispatched 22:58:30Z. Its three jobs ran on unmodified `actions/runner` 2.338.2 slots, one at a time. `bf72.mjs` read each line from the job logs (`r1/result.jsonl`, `r1/job-*.log`).

| Line | Result |
| --- | --- |
| 1. Every job succeeds | **Met.** `build` 4521985, `call / deploy` 4521986, `after` 4521987: all `Success` |
| 2. The called job runs as `call / deploy` | **Met** |
| 3. Its inputs arrive typed | **Met.** `target=prod-7 dry=true dry_is_bool=true`: `target` came from `needs.build.outputs`, and the boolean input compared equal to `true` |
| 4. It gets the inherited secret | **Met.** `key_sha=db48f20ca25a9894`, the first 16 hex digits of the SHA-256 of the value the driver set through `gh secret set` on stdin. `DEPLOY_KEY` was deleted after the run (204) |
| 5. The file ran is the tag's, not the branch's | **Met.** `version=v1` and `url=https://prod-7.example/v1`, while the decoy branch `v1`'s file prints `/branch` |
| 6. The later job reads the output | **Met.** `after` printed `url=https://prod-7.example/v1 result=success` |
| 7. `job_workflow_ref` names the called file (F118) | **Met.** `m3org/bf72-shared/.github/workflows/deploy.yml@refs/tags/v1` |
| 8. `workflow_ref` stays the caller's | **Met.** `m3org/bf72/.github/workflows/caller.yml@refs/heads/main` |
| 9. The token carries the caller run's `actor` and `run_number` | **Met.** `actor=m3-admin run_number=1`: the dispatching admin, and run 4521984 is the first run of `caller.yml` in `m3org/bf72`. This line shows that `CreateRunPlanned { actor }` commits `RunActor` and `RunNumber` hosted (`decisions/DECISIONS.md`, "`reusable`") |

Sheds over the row's window (`tieroff503.py 22:56:58Z 22:59:39Z graft-m4a-edge`): `tierOff` 0, `elect` 0, `edge503` 0 (`sheds.json`).

## r1: what ran

| Step (Cloudflare time) | Result |
| --- | --- |
| `check` and the cost read, 22:50Z | Sites a `d64445ff`, b `076f699d`, c `87aa0cfd` (AM2's site day); edge `78130d2f` (LM-1's); ops `ac4cd9a5`; every schedule `[]`. The day read $8.66 |
| Site day, 22:50Z to 22:52Z | Built from `6b48a6dc` with test-faults, image `e55ab2ad` as BF-71's and AM2's days used: a `f7704ee3`, b `b6e66c3a`, c `7bbf3cbd`. Every flag was kept (BF-71's, `GRAFTYARD_OIDC_HEAD_REF=1`, `GRAFTYARD_OIDC_CLAIMS=1`, `GRAFTYARD_APP_HOOKS=1`, the workers.dev host), and `GRAFTYARD_REUSABLE=1` was added. Crons `[]` (`day-site.log`) |
| Edge day, 22:52Z to 22:56Z | `7b3bf05d` at 100%. `days.sh` now carries every plain-text var the live edge held. The binding diff shows only `FORMATS_ENABLED`, which gained `21.9`; 21.7, 21.8 and 32.28 were kept. edgever exit 0 (`day-edge.log`, `edge-before.json`, `edge-after.json`) |
| `check`, 22:56Z | As above, with the new versions (`day-check.log`) |
| Row, 22:56:58Z to 22:59:40Z | `bf72.mjs` created `m3org/bf72` and `m3org/bf72-shared` (tag `v1` and the decoy branch `v1`), set the secret, dispatched at 22:58:30Z, and every job passed by 22:59:39Z (`row.out`). The runners' JIT keys were shredded, and only their `.out` logs are kept |
| Teardown, 23:01Z | `m3org/bf72` and `m3org/bf72-shared` DELETE 204; anonymous and admin reads answered 404 at 23:01:40Z (`teardown.log`). No runner process left. No ops deploy, so no ops token was minted |

**Left on graft-m4a:**
- sites a `f7704ee3`, b `b6e66c3a`, c `7bbf3cbd` (main `6b48a6dc`, test-faults, AM2's flags plus `GRAFTYARD_REUSABLE=1`, the workers.dev host), crons `[]`;
- edge `7b3bf05d` at 100% (main `6b48a6dc`, test-faults); `FORMATS_ENABLED` now also holds 21.9, and every other plain-text var is as LM-1 left it;
- `graft-m4a-ops` `ac4cd9a5`, every schedule `[]`;
- LM-1's org `actions-mirror2`, untouched.
