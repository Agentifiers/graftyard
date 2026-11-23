# M5-WP6's pure half: BF-44 in memory and `graft-ops` executing `Rotate`

`build/BUILD-PLAN.md`, M5-WP6. Code only, nothing hosted, $0. Branch `m5-wp6` from main `cd10ac82`.

## BF-44 in memory: pass, on tests that predate this package

Row: a vendor-signed undo not in the parent's `undo` list, and a `Resume` after `Keys { vendor: None }` without
customer re-signed undos. Pass: both refused.

- `an_undo_commits_only_byte_for_byte_from_the_current_intents_list` (`crates/release/src/tests/group.rs`): an undo
  not in the list, signed by the right key, is `NotAnUndo`; one in the list under another known key is `NotAnUndo`;
  one bound as a forward entry is `BadBound`; one for another intent is `NotCurrent`; a used one is `UndoUsed`.
- `a_vendor_intent_resumes_after_keys_vendor_none_only_with_a_re_signed_undo`: after the revocation runbook's `Halt`
  and `Keys { vendor: None }`, a `Resume` with no undo is refused, one whose undo is still the vendor's signature is
  `BadSignature`, and one re-signed by the customer commits; the old vendor undo is then `NotAnUndo`.
- `the_halted_intents_undos_survive_keys_vendor_none`, `a_vendor_signed_intent_after_keys_vendor_none_is_refused`,
  and `bf6_every_historical_entry_is_refused_after_keys_vendor_none` cover the rest of the runbook.

No test was added: every case the row names already fails on a wrong implementation.

## BF-46's pure half: `graft-ops` executes `Rotate`

Design: `design/DESIGN.md`, "Install operations", Rotation runbook, edited first (the active slot is the fold's; the
refused rows; the `MintWrapKey` order). Decision: `decisions/DECISIONS.md`, "`rotate-probe`". Open: R45.

| Secret | Steps (`rotate_steps`) | Confirming call, off the request path |
| --- | --- | --- |
| `DeployToken` | probe, switch | `GET /accounts/{id}/workers/scripts` with the new token |
| `ReadToken` | probe, switch | `GET /accounts/{id}/r2/buckets/{first site's bucket}` with the new token, `location` in the answer |
| `MintWrapKey` | publish, wait, switch, retire | The new slot unwraps and its `PutVerifyingKey` commits on `TokenShard(0)`; the switch waits one installation-token lifetime; the old `kid` retires at the switch plus `SIGNING_KEY_CACHE`, `JOB_TOKEN_MAX` and `CLOCK_SKEW` |
| `SshHostKeys`, `McpAppKey`, `McpClientSecret`, `InsightsAppKey`, `HostedRunnersAppKey`, `SecretsWrapKey`, `TunnelToken` | refused by name | None: `graft-ops` pages each run and steps nothing until the customer's `Abort` (R45) |
| `AgentsAppKey`, `WorkspaceAppKey` | the agents account's `graft-ops` | The install's pages; the agents executor key may step them |

The switch is the switch step's `IntentStep`: `apply` records `(slot, at)` in `ReleaseFold::slots`, and `graft-ops`
reads `DEPLOY_TOKEN_<slot>` for its API calls and `MINT_WRAP_KEY_<slot>` for `/mint` from its view. Both slots are
bound at deploy (`workers/ops/deploy.mjs`, `"slots"`), so a switch ships no version. Slot 0 falls back to the
unslotted binding. `/mint` answers 503 on an isolate that holds no view yet, because the slot it reads is cached for
`SIGNING_KEY_CACHE`.

Tests (`crates/release/src/tests/group/run.rs`, through `ops::run` against the real group):
- `bf46_a_token_rotate_probes_the_new_slot_then_switches_with_no_deploy`: `DeployToken` and `ReadToken` to slot 1, then
  `DeployToken` back to 0; probe, then switch, then one `IntentDone`; no deploy and no upload.
- `bf46_a_failed_probe_pages_and_never_switches`: three runs page and commit nothing; the slot moves once the value is
  fixed.
- `bf46_mint_wrap_key_publishes_waits_a_token_lifetime_switches_then_retires`: a minute short of the lifetime it still
  waits on slot 0, and it switches only at the lifetime.
- `bf46_a_rotate_graft_ops_does_not_run_pages_by_name_and_an_abort_ends_it`: each refused row and a `Rotate` to the
  active slot page with the reason, step nothing, and end on the customer's `Abort` with the slot unchanged.
- `bf46_the_install_graft_ops_leaves_the_agents_secrets_to_the_agents_account`.
- `a_killed_graft_ops_reruns_a_rotate_step_and_commits_each_step_once`: killed after 1 to 3 calls in every run, a
  `MintWrapKey` rotation commits steps 0, 1, 2, 3 once each, then one `IntentDone`.
- `workers/ops` `a_mint_wrap_key_rotate_publishes_the_new_key_and_retires_the_old_after_the_longest_token`, which also
  holds `ROTATE_PUBLISHED_FOR` equal to `INSTALLATION_TOKEN_TTL`.
- `cli` `a_rotate_intent_names_its_secret_and_slot`: `release entry` builds `{"Rotate": {"secret", "slot"}}`.

plant95 (`tests/m0carried/plant95.py`): 14 of 14 caught.

No wire enum changed: `Intent::Rotate`, `SecretName` and every tag are as before. `RotateStep` is new and never
encoded; `rotate_steps` is a stored format up to each list's switch, because `apply` moves the slot at that index.

## Left

- BF-46 hosted for `DeployToken`, `ReadToken` and `MintWrapKey`, with live traffic: no new version, zero failed calls,
  the old value unused after the switch. It needs a `graft-ops` deployed with both slots of each bound, which is a new
  version of `graft-ops` (an `OpsRoll`, not a site day).
- R45's rows.

## BF-46 hosted: plan and prep (2026-11-23, $0; the run is cleared for graft-m4a after 00:00Z on 2026-11-24)

The lead cleared the row with a $5 stop. M4-WP4 handed graft-ops over at 11:34Z, but graft-m4a's UTC day was at
$35.23 of its $40 cap, so the row runs on the next UTC day. The driver is `tests/m5/bf46/row.sh`, with the phases
setup, rotate, watch, logs, teardown and lists. Every log it writes goes through a scrub, and no token value is
printed.

- **Throwaway tokens, not the install's.**
  - DeployToken: slots 0 and 1 are two new account tokens with Workers Scripts Write and Workers Containers Write.
    Slot 0 is the secret graft-ops already binds, which held M4-WP4's revoked token.
  - ReadToken: slots 0 and 1 are two new tokens with Workers R2 Storage Read.
  - MintWrapKey: slot 1 is a new kid 2 drawn by `mintkey`. Slot 0 stays the install's kid 1.
  - All four tokens are revoked at teardown, and the four new store secrets are deleted. The store holds 17 of its 100
    secrets, so the row's four fit.
- **Deployed with `deploy.mjs`, not by an `OpsRoll`.**
  - Binding both slots is a binding change. An `OpsRoll` uploads with `keep_bindings` and changes only code
    (`decisions/DECISIONS.md`, the `graft-ops` executor entry), and graft-m4a's group holds no Ops release.
  - So the row deploys graft-ops with `workers/ops/deploy.mjs --executor` and its `"slots"` option, as every graft-m4a
    ops deploy has been, from main once this head merges, because the publish-wait override below is in it.
  - The build is test-faults with `GATE=pending` and a 1-minute cron.
  - The switches then ship no version, which is what BF-46 measures: the ops deployment's version is read before and
    after each `Rotate`.
- **MintWrapKey's wait is lowered, and the results will say so.** `OPS_ROTATE_PUBLISHED_MS`, which only a test-faults
  build reads (`tests/m5/opsfaults.sh`: product 0, test-faults 1), sets the publish wait to 10 minutes instead of one
  hour. An hour of graft-m4a at its idle rate is cheap, but the row's other legs would wait on it. The retire it
  commits is still dated `switched + SIGNING_KEY_CACHE + JOB_TOKEN_MAX + CLOCK_SKEW`, about 25 h later, so kid 1 stays
  valid past the row.
- **Live traffic.** `tests/m5/bf46/mint.mjs` mints an installation token through the edge every 10 s and reads
  `m3org/j4` with it, recording each answer's status and the token's `kid`. A switch shows up as the kid moving from 1
  to 2 within `SIGNING_KEY_CACHE` with no failed mint. A dry run at 12:14Z answered 201 with kid 1, and the read
  answered 200.
- **What the row reports.**
  - Each probe's answer: the script count, and the bucket's location.
  - Each `Switch`'s `IntentStep` index and time, and the slot read from the group after it.
  - The publish and retire rows on `tokens:0`, with the time between publish and switch.
  - The ops version before and after each switch.
  - The mint counts, by status and kid.
- **Restore.**
  - Every slot goes back to 0 by a second `Rotate` of each secret. MintWrapKey's goes back to kid 1: its publish
    rewrites kid 1's row with no `retired`, which clears the retire the first rotation set, and its retire dates kid 2
    about 25 h out.
  - graft-ops is redeployed from main `ad8fd52f` with 4bbfd9e5's configuration. `~/.graft-m4a/ops/exec-ck-pending.json`
    was rebuilt from 4bbfd9e5's own bindings: `GATE=pending`, `OPS_STEP_MIN_ENTRIES` 50, both dwells at 60 s, and no
    slots. Then the cron is turned off.
- **Stop.** $5 on graft-m4a's day, counted with `tests/fanout/acctcost.py` (`PREFIX=graft-m4a`). graft-m4a idle cost
  $0.01 from 11:35Z to 11:55Z.

## BM-5's revocation mid-roll, in memory (2026-11-23, code only)

Before this row, the group tests covered each piece of the runbook on its own:
- `Halt` stops graft-ops (`an_abort_of_an_edge_roll_returns_the_edge_to_target_with_the_roll_kept_beside_it`);
- `Keys { vendor: None }` refuses every replayed and re-signed entry (`bf6_...`);
- a customer `Abort` after it ends the halted intent (`a_vendor_signed_intent_after_keys_vendor_none_is_refused`).

No test ran the three in order mid-roll with graft-ops executing the `Abort`, and none checked BM-5's "every site and
pinned script" back at N. `bm5_halt_keys_vendor_none_then_abort_mid_roll_returns_every_script_to_n`
(`crates/release/src/tests/group/ops.rs`) adds that case. It runs twice: once with site 1's day at 10%, and once
with the edge's roll at 10%. Each run checks these things:
- graft-ops is idle while halted;
- the `Abort` runs to `IntentDone` under the executor key with the vendor key gone. The site returns to N at 100%,
  and the edge returns to N with R kept at 0%;
- a vendor-signed intent from `release roll 1` is refused `UnknownKey`;
- a customer-signed `release roll 1` rolls back only the sites whose finished days moved them, then every script runs
  N with nothing drained.

Planted mutant: an `Abort` that moves only an `EdgeRoll`'s script is caught by the site case, and the other 82
release tests pass with it.

An `Abort` moves only the intent it ends. Sites that earlier, finished intents moved return to N through customer-signed
intents, which is what DESIGN's runbook says ("each site the intent touched"). BM-5's hosted half is still M5-WP7's.
