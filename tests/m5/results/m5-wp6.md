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
- BM-5's vendor-token revocation mid-roll.
