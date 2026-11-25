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

plant95 (`tests/m0carried/plant95.py`): 14 of 14 caught; 15 of 15 with BF-46 hosted prep's publish-wait mutant (2026-11-23).

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

## BF-46 hosted (2026-11-24, graft-m4a): pass for `DeployToken`, `ReadToken` and `MintWrapKey`

Gate line (`build/BUILD-PLAN.md`, M5-WP6): `Rotate` of each `SecretName` with live traffic. Pass: no new script
version; zero failed calls; old value unused after the switch. Fail: a new version, or failures.

**Result: pass for the three secrets `graft-ops` runs (done line 00:49:30.781Z, the restore's `IntentDone`).** Six
`Rotate`s (each secret to slot 1, then each back to 0) ran on `graft-m4a-ops` `de3c4bdb` and shipped no version. 235
mints across them all answered 201, and each token's read answered 200. The R45 rows stay refused, as ruled.
Driver `tests/m5/bf46/row.sh`, tag `m4abf46`, logs `raw/bf46/m4abf46/`. Spend from 00:06:48Z to 00:54Z: $0.11 on
graft-m4a (`tests/fanout/acctcost.py`, `PREFIX=graft-m4a`), under the $5 stop.

**How graft-ops was deployed (the lead's ruling).** `workers/ops/deploy.mjs --executor` with the `"slots"` option, a
test-install deployment step outside the measured path. Binding both slots is a bindings change an `OpsRoll` cannot
make (it uploads with `keep_bindings`), and graft-m4a's group holds no Ops release. Build: main `05aace32`,
test-faults, `GATE=pending`, 60 s dwells, `OPS_STEP_MIN_ENTRIES` 50, a 1-minute cron, both slots of each secret
bound. Version `de3c4bdb`, deployed 00:11:08Z. The deploy's Cron Trigger did not fire, as in BF-7 and the gate check
row; a re-PUT at 00:17:03Z did.

**`MintWrapKey`'s publish wait is a test-faults override.** `OPS_ROTATE_PUBLISHED_MS` = 600,000 (10 minutes, not
`ROTATE_PUBLISHED_FOR`'s hour), read only by a test-faults build. Each retire is still dated `switched +
SIGNING_KEY_CACHE + JOB_TOKEN_MAX + CLOCK_SKEW`, about 25 h out.

**Throwaway tokens only.** `graft-m4a-bf46-deploy-0` and `-1` (Workers Scripts Write and Workers Containers Write) and
`graft-m4a-bf46-read-0` and `-1` (Workers R2 Storage Read), all minted 00:08:11Z to 00:08:12Z; kid 2 from `mintkey`.
`MintWrapKey` slot 0 stayed the install's kid 1 secret.

| Secret, slot | `Rotate` (index, time) | Probe (Workers Logs) | `Switch` `IntentStep` (index, time) | `IntentDone` | Ops version before / after |
| --- | --- | --- | --- | --- | --- |
| `DeployToken` → 1 | 193, 00:14:11Z | 00:17:29Z: 27 scripts listed under slot 1 | 195, 00:17:30.346Z | 196, 00:17:31Z | `de3c4bdb` / `de3c4bdb` |
| `ReadToken` → 1 | 197, 00:18:52Z | 00:19:26Z: `graft-m4a-site-a-packs` location ENAM under slot 1 | 199, 00:19:27.295Z | 200, 00:19:28Z | `de3c4bdb` / `de3c4bdb` |
| `MintWrapKey` → 1 | 201, 00:20:32Z | publish 00:21:28Z: `tokens:0` `PutVerifyingKey` kid 2, `retired: null` (step 0, index 202); wait to 00:32:28Z (step 1, 203) | 204, 00:32:29.261Z | 206, 00:32:31Z | `de3c4bdb` / `de3c4bdb` |
| `DeployToken` → 0 | 207, 00:35:11Z | 00:35:26Z: 27 scripts listed under slot 0 | 209, 00:35:26.860Z | 210, 00:35:27Z | `de3c4bdb` / `de3c4bdb` |
| `ReadToken` → 0 | 211, 00:36:14Z | 00:36:29Z: the same bucket's location ENAM under slot 0 | 213, 00:36:30.257Z | 214, 00:36:31Z | `de3c4bdb` / `de3c4bdb` |
| `MintWrapKey` → 0 | 215, 00:37:29Z | publish 00:38:26Z: kid 1, `retired: null` (216); wait to 00:49:27Z (217) | 218, 00:49:28.373Z | 220, 00:49:30.781Z | `de3c4bdb` / `de3c4bdb` |

The retire rows: after the first `MintWrapKey` switch, kid 1 `retired` 2026-11-25 01:33:29Z (step 3, 00:32:30Z); after
the restore's, kid 2 `retired` 2026-11-25 01:50:28Z (00:49:29Z). The restore's publish rewrote kid 1 with no
`retired`, which cleared the first retire, so kid 1 is valid with no end again. The group after the row: 221 entries,
nothing current, every slot 0.

**The minting probe** (`tests/m5/bf46/mint.mjs`, one installation token through the edge every 10 s and one read of
`m3org/j4` with it; `raw/bf46/m4abf46/mint.jsonl`, `kids.txt`). 00:13:03Z to 00:52:07Z: **235 mints, 235 × 201, 235
reads × 200, no non-201.**

| Window | Mints | kid 1 | kid 2 |
| --- | --- | --- | --- |
| start to `DeployToken` → 1 (00:17:30Z) | 27 | 27 | 0 |
| to `ReadToken` → 1 (00:19:27Z) | 12 | 12 | 0 |
| to `MintWrapKey` → 1 (00:32:29Z) | 78 | 78 | 0 |
| to `DeployToken` → 0 (00:35:26Z) | 17 | 9 | 8, the first at 00:32:47Z, 18 s after the switch |
| to `ReadToken` → 0 (00:36:30Z) | 7 | 2 | 5 |
| to `MintWrapKey` → 0 (00:49:28Z) | 78 | 13 | 65 |
| after, to 00:52:07Z (the probe's last mint) | 16 | 10 | 6, the last at 00:51:27Z |

kid 1 and kid 2 interleave after each `MintWrapKey` switch. Each `graft-ops` isolate keeps the key it read for up to
`SIGNING_KEY_CACHE` (one hour) and reads the slot only when its key is missing or that old, so an isolate warm before
the switch mints on the old kid until it is recycled or the hour runs out. That is the design ("old value unused after
the switch" holds for each isolate's next key read, and the old key stays published and valid until its retire, about
25 h later). Every mint on either kid answered 201 and its token read 200.

**Verdict lines.**
- **`DeployToken`: pass.** No new version (`de3c4bdb` before and after both switches); 0 failed calls (each probe
  listed 27 scripts; 235 mints 201); after each switch `graft-ops`' calls used the new slot's token. The account audit
  log shows each throwaway token's only call is its probe: `deploy-1` at 00:17:28Z, `deploy-0` at 00:35:25Z, both
  `List Worker Scripts` 200. No deploy ran in the row, so no other call was due.
- **`ReadToken`: pass.** No new version; 0 failed calls; `read-1` at 00:19:26Z and `read-0` at 00:36:28Z, each its
  probe's `Get Bucket` 200, and nothing else.
- **`MintWrapKey`: pass, with the `SIGNING_KEY_CACHE` reading above.** No new version; 0 failed mints in 235 across
  both switches; kid 2 minted 18 s after its switch, and kid 1 again after the restore's. "Old value unused after the
  switch" holds per isolate at its next key read, not at the switch instant.

**Teardown (00:52:14Z to 00:53:14Z, `raw/bf46/m4abf46/teardown.log`, `lists.log`).** Probe stopped; ops cron off; the
four tokens revoked; the four new store secrets deleted; `graft-m4a-ops-deploy-token` left holding the revoked
`deploy-0`, as before the row; token files shredded. graft-ops is `5f29ee48`, the test-faults build of main `ad8fd52f`
with `exec-ck-pending.json`: `GATE=pending`, `OPS_STEP_MIN_ENTRIES` 50, 60 s dwells, no slots, `OPS_SCRIPTS` the three
sites, schedules `[]`. That is `4bbfd9e5`'s configuration in a new version. The lists: 0 `graft-m4a-bf46` tokens, 0
`graft-m4a-bf46` secrets, the store at 17, schedules `[]`, every slot 0. Idle runs went on firing to at least 00:53:29Z,
the known tail after a removed cron. Per the lead's ruling, M4-WP4 hears "graft-ops free" once, after BF-47.

**Row notes (`raw/bf46/m4abf46/notes.txt`).** The first setup run was cut off at 00:07:50Z by the caller's 58 s
timeout during the graft-ops build, after the mints and the store writes; `mint()` now keeps a token a cut-off run
minted, and the rerun at 00:09:23Z kept all four. The probe service first started without the env and exited; it was
restarted at 00:13:03Z, before any `Rotate`.

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

## BM-5's revocation mid-roll, hosted (2026-11-24, graft-m4a)

**Pass.** It ran on graft-m4a from 05:06Z to 05:39Z, and the row spent $0.02 (`acctcost.py` on the `graft-m4a` prefix). The driver is `tests/m5/bm5/row.sh` and its logs are in `tests/m5/results/raw/bm5/m4abm5/`. It ran after AM2-WP3 handed `graft-ops` back at 04:37Z, with a $5 stop.

**Stand-ins.** graft-m4a has no vendor:
- `Keys.vendor` read null at generation 2 before the row.
- The vendor key is a throwaway Ed25519 seed (`a0aa16b7…`), named by a customer-signed `Keys` (entry 315, generation 3).
- The vendor's release-bucket token is a throwaway API token, `graft-m4a-bm5-vendor`, with Workers R2 Storage Bucket Item Write on `graft-m4a-releases` only.
- R12 is R9's `formats.txt` plus the site build of `05aace32` and a marker file, signed with the release key. It is sites only, so no pinned script moves. The vendor token put it under `r12/` over the S3 API: six puts, all 200.
- `graft-ops` ran test-faults `05aace32` with `GATE=pass`, a 1-minute cron and 60 s dwells (`f33cf37d`), under a fresh deploy token.

**Before:**
- sites a and b on R9 (`e23f69e9`) and c on R10 (`1a0f0e50`), the "N" of each site;
- a `75bdd60d`, b `627ed36c`, c `54bbb423`; edge `eef95142` at 100% and `f4e193b9` at 0%;
- nothing drained and no intent current; group count 315.

**The run, in `Release` group order:**

| Entry | Time (Z) | Signer | What |
| --- | --- | --- | --- |
| 316 | 05:13:03 | executor | `ReleaseUploaded` R12 (`419303e4…`) |
| 317 | 05:13:08 | **vendor** | `SiteDay { a, R12 }` (`release roll R12` under the vendor key) |
| 318–328 | 05:13–05:21 | drain, executor | site a's day: Drain, 1%, 10%, 50%, 100% with `SiteRelease`, Undrain, Hold, `IntentDone` |
| 329 | 05:21:41 | **vendor** | `SiteDay { b, R12 }` |
| 330–333 | 05:22–05:26 | drain, executor | b drained, steps 0 to 2 committed, so b's day stood past 10%. Its deployment read 50/50 at 05:26:19, because `graft-ops` had already deployed the 50% step it was dwelling on |
| — | 05:26:22 | — | vendor token revoked. A put under it 15 s later answered **401 Unauthorized** |
| 334 | 05:27:06 | customer | `Halt { intent }` |
| 335 | 05:27:08 | customer | `Keys { vendor: None }` (generation 4) |
| 336 | 05:27:10 | customer | `Abort { intent, target: R9 }` |
| 337–341 | 05:27:35–05:27:44 | executor, drain | the `Abort` runs: b to R9 at 100%, undrain, `IntentDone` |
| — | 05:27:33 | **vendor** | `release roll R12` signs `SiteDay { b, R12 }`; refused **422 `UnknownKey`**: "signed by key KeyId(a0aa16b7984a), which the latest Keys does not hold" |
| 342 | 05:28:00 | customer | `release roll R9` signs `SiteDay { a, R9 }`, site a being the one the finished intent moved |
| 343–353 | 05:28–05:35 | drain, executor | site a's day back to R9, `IntentDone` 05:35:34 |

`release roll R9` would next sign `SiteDay { c, R9 }`. It was written with `--out` and not posted, because c's N is R10 and c never moved.

**After (`checks`, 05:37Z):**
- a `75bdd60d` (tag R9) and b `627ed36c` (R9), each the version it ran before; c `54bbb423` (R10), never touched;
- edge `eef95142` 100% and `f4e193b9` 0%, unchanged;
- `site_release` and `rolled` the same as before the row; nothing drained; no intent current;
- `Keys` generation 4, vendor null; group count 354.

The `Abort` went from commit to `IntentDone` in 34 s, with the vendor key already gone and the vendor token revoked. Every site and pinned script is at its N.

**Teardown (05:39Z).**
- The `graft-ops` cron was set yearly, then `[]`.
- `graft-m4a-ops` was set to `ac4cd9a5` at 100% by version id, the version AM2-WP3 parked.
- `graft-m4a-bm5-deploy` was revoked (the vendor token already was), and both token files were shredded.
- `graft-m4a-ops-deploy-token` still holds the revoked bm5 token.
- `r12/` stays in `graft-m4a-releases`, with R12's `ReleaseUploaded` at 316.
- The throwaway vendor seed is in `~/.graft-m4a/bm5/`; no `Keys` names it any more.

**Driver fault found and fixed.** The orb has no `xxd`, so setup's first `Keys` entry was never built and vendor stayed null. The driver decodes the hex in Python now, and the `keys` phase posted the entry before the roll.

## BM-5's unreadable format, code (2026-11-24, branch `bm5-format`, $0)

**What was missing.** Every site build named one constant `RELEASE` (`workers/site/src/host.rs`), so a leader on N+1
read N's `verify` refusals as same-release and never handed off, and no site build read a format N does not.

**Built.**
- **Release seq** (`decisions/DECISIONS.md`, "`release-seq`"). `graft-ops` tags each version `<seq>-<release id>`;
  seq is the release's place in the `Release` group's uploads. A site member takes its `ReleaseTag` from its
  `VERSION` tag at restore. A tag with no seq runs as `RELEASE` and logs it once per isolate.
- **Test format.** `REPO_FORMAT_TEST` (0x20.0xfff0) is a push in the V1 shape with `Push::test_format` set. Only a site
  built with the `test-format` feature (which needs `test-faults`) reads it; no product build does.
- **Aimed `FORCE_FORMAT`.** A test-faults edge with `FORCE_FORMAT_SITE=<n>` forces the format only on a push whose
  chain's leader it last saw on site n (`graftyard_lookup::cached_leader`); without it, BF-16's behaviour holds.

**Tests.**
- `bm5_a_release_s_seq_is_its_place_in_the_group_not_its_upload_time` (`crates/release/src/tests/group/run.rs`).
- `a_member_s_release_is_its_version_tag_s_and_any_other_tag_is_the_oldest` (`workers/site/src/host.rs`).
- `bm5_an_unreadable_format_from_a_newer_site_commits_nothing_and_moves_the_group` (`crates/git/tests/push.rs`): the
  A, B, C hand-off on the git stand-in, with older refusers (moves) and same-release refusers (stays).
- `bm5_a_forced_format_aimed_at_a_site_reaches_only_that_site_s_leaders` (`workers/edge/src/lib.rs`).

**Plants** (`tests/m0carried/plant210.py`): 6 of 6 caught. They cover seq taken from upload time, seq 0 accepted, a malformed tag that panics, seq derived from the release id (so sites differ), same-release refusers handing off (`<=`), and `FORCE_FORMAT_SITE` ignored.

The hosted row is below, in "BM-5's unreadable format, hosted".

## BM-5's unreadable format, hosted (2026-11-24, graft-m4a, row m4af1, $0.46)

**Verdict: passes.** On all three repos the push in 0x20.0xfff0 committed on no member, and every group then led on
site b. Driver: `tests/m5/bm5/format.sh`; raw logs in `tests/m5/results/raw/bm5/m4af1/`.

**Setup** (12:16:32Z to 13:04:26Z, all from main 801d63ad).
- `graft-ops` 07131128 (test-faults, `GATE=pass`, 60 s dwells) uploaded N = 4c6729de with seq 22 and N+1 = 2f379da8
  with seq 23. N+1's site build has `test-format`. Every site script then held versions tagged `22-4c6729de…` and
  `23-2f379da8…`.
- `release roll n` ran the three site days (12:27:46Z to 12:54:42Z). `release roll n1 0` ran site a's day, done at
  13:04:21Z. `site_release` ended a = 2f379da8, b and c = 4c6729de, with no intent current and nothing drained.
- The edge was redeployed as 3443bb26 with every binding kept plus `FORCE_FORMAT=32.65520` and `FORCE_FORMAT_SITE=0`.
- Three fresh repos homed on site a: bm5f-m4af1-1, -3 and -4 (repo:65589, 65591, 65592). A fourth, -2, got a 422
  during b's drain and has no config; it is excluded.

**Pushes** (`push.log`). For each repo: one clone through the edge (its leader a, term 2), then six plain PAT pushes
3 s apart, each to its own ref `bm5f/m4af1/<repo>/<i>`.

| Repo | Push 1 (forced, leader a) | NoLead alarm on a | New leader | Pushes 2–6 |
|---|---|---|---|---|
| 65589 | 13:04:49Z, HTTP 503 | 13:05:13Z, refusal at index 32 by site 1 at seq 22 | b, term 3 | HTTP 200, indexes 35–47 |
| 65591 | 13:05:46Z, HTTP 503 | 13:06:11Z, index 30 | b, term 3 | HTTP 200, indexes 33–45 |
| 65592 | 13:06:42Z, HTTP 503 | 13:07:07Z, index 24 | b, term 3 | HTTP 200, indexes 27–39 |

- **Nothing in 0x20.0xfff0 committed.** On every member of every repo, the index the alarm names (32, 30, 24) holds a
  term-3 entry, appended by b after a truncated its own. No member's `/log` holds a `/1` ref from the forced push.
- **The refusal reads as older.** Site a's alarm is `NoLead { site: SiteId(1), refusal: Refusal { release: ReleaseTag
  { id: 4c6729de…, seq: 22 }, … reason: "format Format { family: 32, version: 65520 } is not read…" } }`, then
  `TransferStarted { to: SiteId(1), why: NoLead }`. Site b logged no NoLead.
- **The forced format reached only a's leaders.** `recv:32.65520` rose to 3–4 per group with push 1 and never rose
  after it. Pushes 2–6 went to b in the normal format.
- **Ending state, 13:12Z.** All 9 members agree on commit (56, 47, 41), and each push's entry has the same index and
  term on all three.

**Caveats.**
- The supervised push service restarted once after DONE and pushed repo 65589's `/1` again at 13:07:39Z, through
  leader b in the normal format (HTTP 200). It committed at index 54, term 3. It is not the forced push.
- **Driver fault, fixed.** `formats.txt` must be sorted, and the first N+1 had 32.65520 appended at the end, so
  `graft-ops` refused it. N+1 was re-signed sorted; the refused id db46d152 never reached the group.

**Seq-less versions on graft-m4a.** Every version before this row has a bare-id tag or none: sites b2bb49ec,
5f9e7a9f and 74869876, and every R9, R10 and R12 upload. Builds before 72fbc0a4 have no seq code and run the constant
`RELEASE` (seq 1). New code reading a seq-less tag also runs `RELEASE`, and logs it once per isolate
(`a_member_s_release_is_its_version_tag_s_and_any_other_tag_is_the_oldest`). A rollback to any of them therefore
ranks as oldest. This row did not roll back to one: no site ran a seq-less version during it, and the Workers Logs
query for "names no release seq" found 0 lines on sites a and b.

**Left as found and as changed.**
- The edge is back on eef95142 at 100% and f4e193b9 at 0%. `graft-ops` is parked on ac4cd9a5 with the cron off,
  and the row's deploy token is revoked.
- Site a still runs N+1 (2f379da8, built with `test-format`); b and c run N. A product build never reads 0x20.0xfff0.
- New prefixes `bm5f-n/` and `bm5f-n1/` are in `graft-m4a-releases`.
- Spend $0.46 on the graft-m4a prefix (`tests/fanout/acctcost.py`, 12:16:32Z to 13:14:48Z).

## BM-5's `graft-ops` swap-back, hosted (2026-11-24, graft-m4a, row m4am1, $0.02)

**Verdict: passes.** A bad `graft-ops` at 1% was swapped back by `OpsPointer` with the API blocked: an edge isolate
committed the pre-signed pointer from its own mint errors, and once unblocked `graft-ops` returned to N at 100% and
ended the intent. Driver: `tests/m5/bm5/mint.sh`; raw logs in `tests/m5/results/raw/bm5/m4am1/`.

**Setup** (14:56:03Z to 15:11:57Z, every build from main 6aa73504).
- `graft-ops` 4603cf96 (test-faults, `GATE=pass`, 60 s step dwell, a 120 s first-day dwell for the 1% step, minute
  cron, `ops` in `OPS_SCRIPTS`). Its cron fired only after a second schedule reset, at 15:08Z (BF-47's gotcha).
- **N.** e23f69e9's Ops version c7079529 carries a bare tag, so N is a fresh ops-only release, 7def7980, the plain
  test-faults build. `graft-ops` uploaded it as e8626037, tagged `24-7def7980…`, and its `OpsRoll` (443 to 446) left
  e8626037 at 100% with c7079529 at 0%.
- **N+1.** 5bc88414, the `test-mint-fail` build, uploaded as c24d1836 (seq 25). `release roll` signed its `OpsRoll` at
  448 with the `OpsPointer` to N pre-signed.
- **The edge.** 06329970, main's test-faults edge with eef95142's bindings and 11 vars, at 100%: eef95142 predates
  `MintWatch`.
- **API blocked.** At 15:11:57Z, as soon as `graft-ops` deployed c24d1836 at 1% and e8626037 at 99%, the deploy token
  in `graft-m4a-ops-deploy-token` was replaced by an invalid value. A probe under it answered 400.

**The swap-back** (`load.log`, `checks.log`).

| Time | Event |
|---|---|
| 15:12:23Z | m3-ci installation-token mints through the edge, 24 at a time, 100 a batch |
| 15:12:23Z to 15:13:31Z | 700 mints: 688 answered 201, 12 answered 503 (1.7%, about N+1's share) |
| 15:13:54Z | edge 06329970: "graft-ops mint errors past MINT_ERROR_RATIO: submitting the pre-signed OpsPointer" |
| 15:13:55Z | `OpsPointer` committed at 449, customer-signed (the pre-signed undo); the intent is `Undoing` |
| 15:13:56Z | the same isolate: "mint undo: OpsPointer committed" |
| 15:14:43Z | `graft-ops`: `Wait { why: "no deployment of Ops read" }`, the API blocked; the split still c24d1836 1% |
| 15:15:36Z | deploy token restored |
| 15:16:52Z | `graft-ops` deploys e8626037 100% with c24d1836 0% kept, then `IntentStep` 0 at 450 |
| 15:16:54Z | `IntentDone` at 451 (the done line) |

- **No deploy between the pointer and the unblock.** graft-ops' deployments list runs from 15:12:20Z (1%) straight to
  the 15:16:52Z return, and the only ops log line in that time is the blocked `Wait`. The load stopped when the pointer
  committed, so this row did not measure mints answered under the pin.
- **One isolate tripped**, after about 700 mints. The edge had 1 "mint undo" line and 1 MINT_ERROR_RATIO line, both
  from that isolate, and no other isolate submitted. The bound needs 2 thrown or 5xx N+1 answers among few
  (2·ln(1/0.01) ≥ ln 10,000), so an isolate needs about 200 of its own mints at 1% [inference]. That is well inside
  the lead's 3,000-mint line.
- `rolled.Ops` is 7def7980 (N). N+1's `ReleaseUploaded` stays in the group, and the `rolled` record never named it.

**Left as found and as changed.**
- The edge is back on eef95142 at 100% and f4e193b9 at 0%. `graft-ops` is parked on ac4cd9a5, schedules `[]`.
- The row's deploy token `graft-m4a-bm5m-deploy` is revoked and its file shredded. `graft-m4a-ops-deploy-token`
  holds that revoked token.
- `rolled.Ops` moved from e23f69e9 to 7def7980. The graft-m4a-ops versions e8626037 (N) and c24d1836 (N+1) stay.
- New prefixes `bm5m-n/` and `bm5m-n1/` are in `graft-m4a-releases`. Sites are untouched.
- Spend $0.02 on the graft-m4a prefix (14:56:03Z to 15:20:57Z); the UTC day stands at $17.95.

**Driver faults, fixed in the driver.**
- A version upload's `keep_bindings` keeps the script's newest version's vars, not the deployed one's, so the first
  edge upload carried clause 1's `FORCE_FORMAT` and `FORCE_FORMAT_SITE`. `upload-edge.mjs` now copies eef95142's
  plain-text vars itself; 691d05c1 and 2f0a107d, the two wrong uploads, ran about 1 minute each before 06329970.
- `watch` read `current` with jq's `?`, so it missed `IntentDone`; fixed.

## BF-46 hosted, `SshHostKeys` on graft-m3f (2026-11-24 to 25): pass on m3fs2, after m3fs1 failed on deployment state

Gate line (`build/BUILD-PLAN.md`, M5-WP6) as the lead set it for this row: no new script version across the rotation;
zero failed sessions; the host key is the old slot's before the switch and the new slot's after. Driver
`tests/m5/bf46/ssh-row.sh` (steps) and `tests/m5/bf46/ssh-probe.sh` (sessions); logs `raw/bf46ssh/m3fs1/` and
`raw/bf46ssh/m3fs2/`. Spend on the graft-m3f prefix from 23:00Z to the end sweep: $0.73, both tries, under the $3
stop. graft-m3f's day 2026-11-24 closed at $1.90, and 2026-11-25 to the sweep is $0.43.

**Result: pass (m3fs2, done line 00:56:30.497Z, the `IntentDone`).** m3fs1, the first try, failed with a known cause:
two of three sites ran an old `graft-sshd`. The rerun on fronts that all ran the new image had 0 failed sessions.

### m3fs1: fail, deployment state

**Setup** (a site day was approved for this row):
- sshfront image `graft-m3f-sshfront@sha256:6085be00…` built from main `b9dd485f`; shard image reused (`7d5c7540…`).
- Site day, test-faults with `--ssh`: a `5b742740`, b `41a4dc72`, c `6d7a5008`, done 23:14:11Z. Edge `2bf87f77` at
  23:17:53Z. Every site and edge schedule `[]`.
- `graft-m3f-ops` `c726123b`: test-faults, `GATE=pending`, 1-minute cron, `ssh_announce_ms` 300,000 (5 min for
  `SSH_ANNOUNCE_FOR`'s 30 days). The cron fired only after a re-PUT at 23:28Z (R52).
- New slot B written to `SSH_HOST_KEY_B` at about 23:23Z (`deploy/ssh-front/hostkeys.mjs`; R60's guard passed).

| UTC | |
| --- | --- |
| 23:24:32 | `Rotate { SshHostKeys, 1 }` committed at index 715 |
| 23:29:28 | probe: "every site holds" the new slot's three fingerprints (`34javTLN…`, `Rh8cnvVq…`, `xSxG/fOt…`) |
| about 23:35:28 | the switch: view slots `SshHostKeys: 1`; meta/ssh `active: B` |
| 23:47:42 | probe stopped |

**Sessions** (stock OpenSSH and git through `ssh.graftyard.dev`, every 20 s):
- Before the switch: 38 rounds, 0 failed; every key scan served slot A's ed25519 `TMEOnXhy`.
- After the switch: 37 rounds. The pinned session (known_hosts from the run's start, `UpdateHostKeys=yes`) failed in
  8, rc 128 (rounds 47, 49, 50, 61, 65, 70, 71, 75). Fresh key scans mixed slot B's ed25519 (13 of 37) with slot A's
  (24 of 37).
- No new script version from 23:14Z to the stop.

**Cause (verified from the Containers API at 23:44Z).**
- Sites a and b's `sshfront` instances ran image `…4ed05f64`, started 22:42Z and 22:50Z, before the site day. Only
  site c ran `6085be00`, started 23:15:31Z.
- A new site version restarts no running container, and `SshFront` (`workers/site/ssh.mjs` `up`) started its
  container only when none ran. So a and b kept the pre-R45 `graft-sshd`, which reads the active slot once at start,
  and they served slot A after the switch.
- The edge spreads sessions over the sites, so a pinned client that learned keys from a or b never saw the new B
  announced, and failed on c.
- DESIGN's front-door bad-deploy row already said a `graft-sshd` image changes when its `SshFront` restarts the
  container. Only `GitShard` did that (`workers/site/src/shard.rs` `restart_on_image`).

**Fix, branch `sshfront-image`, $0, not deployed** (it rides graft-m3f's next site day; `decisions/DECISIONS.md`,
"`sshfront-image`"):
- `SshFront` records the image it started and restarts on a version whose image map names another `sshfront` image.
- `graft-sshd`'s banner is `SSH-2.0-graftyard_2`.
- The `SshHostKeys` probe reads each running front's banner through the Registry (`/ssh_front`) and refuses one below
  generation 2. The runbook line ("restart each front before rotating") alone would not have stopped the next
  install with an old front from repeating this.
- plant214: 7 of 7 caught.

### m3fs2: pass, back to slot A on all-new fronts

**Fronts before the Rotate** (Containers API, 00:47:06Z, `raw/bf46ssh/m3fs2/fronts-before.log`): `ssh#0`, `ssh#1`
and `ssh#2` all running `sha256:6085be00…`, started 00:41:23Z, 00:20:07Z and 23:15:32Z. The idle stop
(`IDLE_MS`) cycled a and b after m3fs1's probe stopped, and their next sessions started the version's image.
Versions unchanged since the site day: sites a `5b742740`, b `41a4dc72`, c `6d7a5008`, edge `2bf87f77`, ops `c726123b`.

| UTC | |
| --- | --- |
| 00:47:15 | probe started (fresh known_hosts pinned at round 1, slot B's keys) |
| 00:49:51.8 | `Rotate { SshHostKeys, 0 }` committed at index 11 of the row's view (`release post` index 722) |
| 00:50:27.8 | probe step: "every site holds" slot A's `TMEOnXhy…`, `XEJXBQhW…`, `qgOnPPlr…`; `IntentStep` 0 |
| 00:56:26.8 | the 5-minute announce ends; `IntentStep` 1 |
| 00:56:28.6 | the switch, `IntentStep` 2: view slots `SshHostKeys: 0` |
| 00:56:30.5 | `IntentDone` (the done line) |
| 01:03:10 | probe stopped; its user key deleted (204) |

**Sessions:**
- Before the switch: 27 rounds, 0 failed; every key scan served slot B's ed25519 `34javTLN`; 5 clone-and-push
  rounds, each read back over HTTPS.
- After the switch: 20 rounds, 0 failed, the pinned session included; 3 clone-and-push rounds, each matched over
  HTTPS. Key scans: 19 of 20 served slot A's `TMEOnXhy`. The first came at 00:56:41Z, 13 s after the switch, and the
  last slot-B scan at 00:56:59Z, inside one `graft-sshd` refresh (60 s).
- No new script version: every deployment and version count is the same at 01:03:12Z as at 00:47:08Z
  (`raw/bf46ssh/m3fs2/versions.log`).
- The probe's `ls.fp` and `pinned.fp` fields read the run's first key exchange in every round, because `ssh -E`
  appends. They are not evidence in either try; the key scans and exit codes are. The driver now empties the logs
  each round.

**Slot B's original key is not restored.** The original slot B (`CTNCAohl…`, `VTDCSGU6…`, `aMp29mjt…`) had no
private half by the time of this row, so slot B now holds the row's keys (`34javTLN…`, `Rh8cnvVq…`, `xSxG/fOt…`). Slot
A, the starting slot, is active again.

**A17, half answered.** A new value written under a bound, running script reached it with no new version. The
edge's `/meta/ssh` listed slot B's new public keys within seconds of the 23:23Z Secrets Store write. It reads
`SSH_HOST_KEY_B` through the site Registry's `/ssh_slots` and caches it 60 s per isolate. The other half, the time
from a put to the first read in a running isolate, is the HostedRunnersAppKey timing row's.

**Teardown** (01:03:54Z to 01:07Z):
- `graft-m3f-ops` back to `3ed6e453` at 100%. The driver's `versions deploy` with a short id exited 1; the
  full id worked, and the driver now resolves it.
- Every graft-m3f schedule `[]`.
- Token `graft-m3f-bf46-deploy` revoked; `graft-m3f-bf46-deploy-token`, `-executor-key` and `-drain-key` deleted.
- End sweep: 3 shards of the probe's repository (net 3932163) torn down, then no shard container running.
- Kept as asked: the `Keys` at Release index 694 and the `graft-m3f-export` bucket.
- Left running: the three `sshfront` containers on `6085be00`, which stop after `IDLE_MS`.
