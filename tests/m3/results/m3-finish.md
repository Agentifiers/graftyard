# Milestone 3's finish on graft-m1

This round works toward every M3-WP3 row Done hosted, on the graft-m1 install. Branch `m3-finish`, from `m3-rotate` (`74550b6`, on main `655fee3`). Everything ran on `graft-m1-*`. Nothing touched `graft-m2b-*` or `graft-m1-lin-*`.

## What was deployed

| Script | Version | What changed |
| --- | --- | --- |
| `graft-m1-edge` | `9339fb5b-aaa5-4d0e-b943-43e7ab44b446` | R20's `POST /api/graftyard/v1/release/tokens` and the test-faults `/_bed/precreate`, at `5938d1b`; was `8a824825`. Deployed 20:07:04Z |
| `graft-m1-site-a`, `-b`, `-c` | `7fe1aaa9`, `70748bab`, `438f715c` | Unchanged: no site code changed, so no site day |
| `graft-m1-ops` | `ca658d46` | Unchanged |

[measured, `tests/m3/raw/finish/edge1.log` and `wrangler deployments list`]

An edge version is not a site-script version, so it ships without a site day (AGENTS.md, "Invariants never to break").

## R20: install creates every `Tokens` shard. Done.

The operator's decision: install creates every `Tokens` shard ahead of time, and `Names` stays create-on-first-use.

### Why a first request took 13 s

The earlier guess was one `ELECTION_AFTER`. That guess was wrong. The cost is the members' `/join`.

- A `/create` with `campaign: false` on a fresh shard, so joins only, took 9.9 s and 9.9 s (`tokens:2994`, `tokens:2995`). The campaign after it took 1.2 s to a leader [measured].
- A join on a fresh member measures each peer: `RTT_SAMPLES` (20) sequential pings to each of the other two sites, in `workers/site/src/member.rs`, `join`. The rows read back were 120 ms (enam–weur) and 215 ms (enam–apac). The apac member's own pings run to two sites over 100 ms away, so 20 × (about 200 + about 270) ms is about 9.4 s [inference from the measured RTT rows].
- A full `/create` took 11.5 s to 13.1 s on four fresh shards, against 13.0 s median before [measured].

### What was built (`5938d1b`)

- `graftyard_identity::TOKEN_SHARDS` (3,072).
- `workers/edge/src/precreate.rs`, its own module, which keeps it apart from AM1-WP2's edge changes:
  - `POST /api/graftyard/v1/release/tokens?from=F&count=N` creates the groups of `Tokens(F .. F+N)`, at most 16 per request and two at once inside one invocation.
  - The body is the signed genesis, the same credential as the entries endpoint. The edge accepts it against its anchor and creates only if it is the genesis the `Release` group holds.
  - The answers are 200, 503 with each failed shard, and 422.
  - With test-faults, `/_bed/precreate` does the same under the `GRAFTYARD_GENESIS` stand-in's record, for a test install with no `Release` group.
- `graftyard install --genesis` calls `precreate_tokens` after the genesis commits: 192 ranges of 16, 8 requests at once. It reports failed ranges and tells the operator to run `install` again. `create` returns at once for a group that answers, so a rerun is idempotent.
- One invocation cannot create all 3,072, because of the six-connection and subrequest limits (`research/cloudflare.md`, Workers limits). That is why the CLI drives the ranges.
- Test: `install_creates_every_tokens_shard_and_reports_the_ranges_that_failed`, against a stub edge. Every shard is covered exactly once, the genesis bytes are the body, and a range answered 503 is reported.
- Planted bugs, both caught by that test:
  - ranges stepped by 15, so they overlap;
  - failed ranges dropped.

### Hosted (`eaff64b`)

- **Pre-create.** `tests/m3/hosted/precreate.mjs`, 12 ranges at once, from 20:08:32Z: 191 of 192 ranges answered 200, in 21 min. Range 784 had one shard, `tokens:791`, refused by all three members with "claim unanswered": the registry claim timed out at 5 s under load. Its rerun answered 200 in 14 s [measured, `tests/m3/raw/finish/precreate.jsonl`, `precreate-retry.txt`].
- **Check.** A second sweep found every group at once: 192 of 192 ranges answered 200 in 79 s, a median of 4.7 s per range of 16 probes. Every one of the 3,072 shards answers [measured, `precreate-verify.jsonl`].
- **Latency.** `tests/m3/hosted/device-latency.mjs`, 24 device flows as gh's client, approved as `m3-member` [measured, `tests/m3/raw/finish/device-latency.jsonl`]:

| | Before (`m3-hosted.md`) | After |
| --- | --- | --- |
| `POST /login/device/code`, median | 13.0 s (65 requests) | **0.83 s** (24) |
| `POST /login/device/code`, max | 14.4 s | 1.3 s |
| Token exchange, median | about 15 s | **1.9 s** (max 2.5 s) |
| `gho_` tokens issued | | 24 of 24 |

The edge's `commit_system` still calls `create`, whose probe costs one read, about 0.1 s to 1 s, on an existing group.

## `mint-key` kid 1 retire. Done.

Kid 1's retire-not-before time was 2026-11-07T21:36:36Z (`1794087396621`, `tests/m3/results/m3-close.md`).

- 22:33:58Z: `tokens:0` held both keys with `retired: null`, at index 34 [measured, `tests/m3/raw/finish/retire/before.txt`].
- `graftyard mint-key step --old-kid 1 --old-public 7ec36cd3… --new-kid 2 --new-published 1794076372120 --switched 1794080136621` answered "retire kid 1 at 1794087396621 ms". It wrote the `PutVerifyingKey { kid: 1, key: kid 1's public key, retired: 1794087396621 }` request [measured, `step.out`, `retire.request.json`].
- 22:34:26Z: the request committed as the operator on `tokens:0` at index 35 [measured, `commit.out`].
- After the commit, kid 1 reads `retired: 1794087396621` and kid 2 reads `retired: null` [measured, `after.txt`].

### Checks

No kid 1 token survives. The m3-close control token expired at 20:34:25Z, and its file was deleted. The `MintWrapKey` secret now holds kid 2, so nothing can mint a new kid 1 token. Kid 1 was therefore checked with a probe: a fresh kid 2 token whose JWS header was rewritten to `kid: "1"`. That token names kid 1, and `verify` looks the key up by the header's `kid` before it checks the signature.

- **Offline, with the edge's code** (`graftyard_identity::stateless::verify`, `tests/m3/hosted/tools` bin `verifyghs`), over the hosted `tokens:0` fold read through `fresh` after the commit [measured, `verify-offline.txt`]:

| Token | At | `verify` |
| --- | --- | --- |
| fresh kid 2 | now (22:37Z) | ok, kid 2 |
| kid 1 probe | `retired` − 1 ms | `Signature`: the key is still live, and the bytes are not kid 1's signature |
| kid 1 probe | `retired` | **`Retired`** |
| kid 1 probe | now | **`Retired`** |

  The fresh kid 2 token answers `NotYet` at the two earlier times, because its `iat` is later than them. That is expected.
- **Over HTTPS with stock git** (`tests/m3/hosted/retire-push.sh`, 5 pushes per token to `m3org/j4`'s main) [measured, `push.txt`]:
  - fresh kid 2 token: 5 of 5 accepted, ref moved;
  - kid 1 probe: 5 of 5 refused ("Authentication failed"), ref not moved.
- `GET /repos/m3org/j4/issues` answered 200 for both tokens and for no token at all, so a read cannot tell them apart. The push is the check.
- **What the probe does not show.** A genuine kid 1 signature refused as `Retired` was not observed. No kid 1 token exists to try, and no kid 1 signing key is kept. The code path is the same: `verify` refuses on `retired` before it checks the signature. The probe's `Retired` at `retired` and later, and its `Signature` 1 ms before, show the row is what refuses it [inference].
- Token scan: no `ghs_`, `ghp_`, or `gho_` text, and no `wrap` or `wrapped` field, in `tests/m3/raw/finish/` or on any process argv. The probe's token files were in a 0700 temp directory and were deleted [measured].

## Checks

At `dc81e96`, every check passes:
- `cargo fmt --check`;
- workspace clippy;
- wasm32 clippy for site and edge, with and without test-faults, and for ops;
- `cargo test --workspace`: 467 passed, 0 failed;
- `check_design.py`, `check_names.py`, and `check_links.py`.

[measured, `tests/m3/raw/finish/checks.log`]

## BF-30: transfers across enterprises and placements, and an org changing enterprise. Done hosted.

The row (`build/BUILD-PLAN.md`, M3-WP3): "Both routes reach the group throughout. `deps` names the new owner after `Settings`."

### How the move goes: one voter site per config change, not a two-placement `Joint`

`Member::reconfigure` refuses a `Joint` whose halves differ by two sites (`NotOneChange`) and accepts one whose halves differ by one site. A test shows both: `a_joint_of_two_placements_differing_by_two_sites_is_refused` in `crates/identity/tests/group_move.rs` [measured]. So the move is a sequence. Each swap is a one-site `Joint` (join the new member at the next generation, then commit `Joint { old, new }`) followed by its `Normal`, with `Home` and a leadership transfer at the end. Leadership leaves a site before that site is dropped (`graftyard_identity::group_move::next_step`). `crates/raft` is unchanged, so neither the long sweep nor Porcupine needed a rerun.

Hosted, a `Tri` → `Na3` move ([0,1,2] home 0 to [0,3,1] home 3) passed through these configs: `0,1,2 h0` → `0,1,2,3 h0` (the one-site `Joint`) → `0,1,3 h0` → `0,1,3 h3`. A route sampled between steps saw each of them [measured, `configs` in `tests/m3/raw/finish/bf30-b/transfers.jsonl`].

### What was built

- **Repository format 32.10, the transfer's own `Settings`** (`crates/git/src/transfer.rs`, `2f09612`).
  - Wire form: `RepoCmd::Transfer(TransferSettings { owner, enterprise, request })`.
  - Only the finisher sends it, with no account and no key.
  - Its only dep is `Authz(owner)`. It refuses with `NotReserved` unless that fold holds the repository's name as `Reserved` or `Live`, with `WrongEnterprise` unless the enterprise matches, and with `NotRead` when the dep is missing. A re-send writes nothing.
  - Record tag 0x54.
- **Authz format 0x12.4, the `Audit` cursor on an owner's `Authz` chain** (`bb4acb5`).
  - `AuthzEvent::AuditCursor { through }` takes tag 16, and `AuthzOp::AuditCursor` takes tag 29. Only the operator writes it, and it never moves back.
  - It is stored in a new trailing part of `AgentRows`, so a fold whose cursor never moved keeps AM1-WP2's bytes.
  - `Audit` owes a `Transfer` for a `Reserved` name and an `Enterprise` change for an `Enterprise` event. It plans the move, `Settings`, `Live`, `Alias`, and `RepoOwner` (`finish::transfer`), and each `Live` repository's `Settings` for an enterprise change (`finish::enterprise_change`).
- **Site** (`bb4acb5`, `e0ffb24`, `b18894e`).
  - The member route `/move` runs one `next_step` on the leader. A refusal is retried at the new leader, both a `notLeader` answer and the `"NotLeader(..)"` that `reconfigure` returns when leadership moves during the step.
  - The `Authz` family has an `Audit` alarm and an in-band `/finish`.
  - The `Repo` family has a `{"settings": true}` read.
- **Edge** (`workers/edge/src/transfer.rs`, `e0ffb24`).
  - `POST /repos/{owner}/{repo}/transfer` commits `ReserveTransfer` under the caller's credential, then starts `/finish` on the destination's `Authz` leader, then answers 202 with `minimal-repository`. It is sent only once 32.10 and 18.4 are in `FORMATS_ENABLED`.
  - With test-faults, `x-graft-kill: after_reserved` skips the in-band finish.
- **`/repositories/{id}` during a transfer** (`b18894e`). Hosted run a found this. The finisher's last two steps are the old owner's `Alias`, then `RepoOwner`. Between them, `RepoOwner` still names the old owner, which by then holds only the `Alias`. The id route looked for a `Live` name only, so it answered 404 in that window: 35 of 2,715 samples in run a, 1 to 3 per transfer, in every transfer. `AuthzFold::id_name` now falls back to the `Alias`. The `Audit` transfer test checks it, and it fails on the `Live`-only lookup (`audit.rs:602`) [measured].
- **Test-only.**
  - The bed route reads a repository chain under either owner's record, with the isolate's cached configs dropped (`cold`) and with the answering leader's config (`2a2b9ee`).
  - The four-site table `tests/m3/finish/sites4.json` adds `graft-m1-site-d` in `wnam`, the fourth region.
  - The driver is `tests/m3/hosted/bf30.ts`.

### The hosted run

The second placement is `Na3`: [0,3,1] home 3, which shares sites 0 and 1 with `Tri`. The two enterprises (E1, E2) and five orgs were made fresh for each run:

| Org | Placement | Enterprise |
| --- | --- | --- |
| a | Tri | E1 |
| b | Tri | E2 |
| c | Na3, home 3 | E1 |
| d | Tri, home 1 | E2 |
| g | Tri | E1 |

Each transfer is `POST /repos/{o}/{r}/transfer` over HTTPS with m3-admin's PAT. m3-admin administers every org.

During each transfer, five routes were sampled back to back until it was done, and once more after:
- `rOld` and `rNew`: the repository chain's settings read through the old owner's record and through the new owner's record, each cold;
- `hId`: `GET /repositories/{id}/issues`;
- `hOld`: `GET /repos/{old}/{name}/issues` (200, or 301 once the old name is an `Alias`);
- `hNew`: `GET /repos/{new}/{name}/issues`, counted from its first 200 on, because before `Live` it answers 404 by design.

A transfer is done when four things hold:
- `RepoOwner` names the new owner;
- the chain's settings, which are what `deps` reads (`RepoFold::authz_owner` and `authz_enterprise`), name the new owner and its enterprise;
- the new name is `Live`;
- the old name is an `Alias`.

After that, a label write by the new owner's admin must answer 201, and one by the old owner's admin must answer 403 or 404. Both go through `deps`.

**Run b, at `087e296`, on the final deploy (below)** [measured, `tests/m3/raw/finish/bf30-b.log`, `bf30-b/transfers.jsonl`]:

| Kind | Transfers | Passed | Done, median | Route samples | Route failures |
| --- | --- | --- | --- | --- | --- |
| Across enterprises (a → b, E1 → E2, same placement) | 4 | 4 | 7.0 s | 150 | 0 |
| Across placements, root networks (a → c, Tri → Na3) | 5 | 5 | 18.1 s | 1,111 | 0 |
| Across placements and enterprises (c → b, Na3 → Tri, E1 → E2) | 4 | 4 | 23.4 s | 569 | 0 |
| Home only (b → d, Tri home 0 → Tri home 1) | 4 | 4 | 12.6 s | 288 | 0 |
| Back across placements (c → a) | 1 | 1 | 20.7 s | 105 | 0 |
| **Total** | **18** | **18** | **17.6 s** | **2,223** | **0** |

- In all 18 transfers, `deps` named the new owner after `Settings`. The new owner's admin wrote a label (201), and the old owner's admin was refused (403).
- **`Audit` alone finished the transfer whose finisher was killed after `Reserved`** (#9, `t9` from a to c, with `x-graft-kill: after_reserved`). The edge answered 202 in 1.3 s and started nothing. The destination's `Authz` chain had no in-band run for entries 16 to 18. At 05:42:02Z its leader's alarm batch `15..18` planned and committed the 5 steps (move, `Settings`, `Live`, `Alias`, `RepoOwner`) in 13.9 s, and the transfer was done 76.2 s after the 202. That is `FINISH_AFTER` (60 s), plus the alarm, plus the steps [measured, observability: "audit authz:7501013 15..18 13892ms {"owed":1,"refused":0,"steps":5}"]. Its 586 route samples during that time had no failures.
- **The org's enterprise change.** The operator committed `Enterprise { E2 }` on org g's `Authz`, and nothing else was sent. `Audit` gave both of g's repositories the transfer `Settings` with E2, 67.6 s later. A label write by g's admin then answered 201 on both, through `deps` that now name E2 [measured, `bf30-b/enterprise.jsonl`].
- Two of run b's planned 20 transfers, #18 and #19 (d → m3-admin, an org to a user), were refused with 422 "m3-admin already has a repository named t1" and "… t2". Run a had already moved its own `t1` and `t2` to m3-admin, the one destination both runs share. The 422 is the right answer: `Live` names are unique per owner. The driver now gives that destination `<name>-<tag>`, and run c covers the org-to-user case on its own (below).
- **Run c, at the same build** (`--only user`, two new repositories): d → m3-admin, an org to a user, from Tri home 1 to Tri home 0. 2 of 2 passed. Done took 10.5 s and 8.5 s. The routes had 90 samples and 0 failures. The new owner's labels answered 201 and the old owner's 403. The 202's `owner.type` is `User` [measured, `bf30-c/transfers.jsonl`].

**BF-30 on the final build: 20 of 20 transfers passed, with 2,313 route samples and 0 lost.** `deps` named the new owner after `Settings` in 20 of 20, and the enterprise change finished on 2 of 2 repositories [measured].

**Run a, before `b18894e`, is kept as the evidence for the fix.** It had 20 transfers with the same plan. Every transfer finished, and `deps` named the new owner in 20 of 20. But `/repositories/{id}` answered 404 in 35 of 2,715 samples, 1 to 3 per transfer, so the row failed. Its #20 also took 69.6 s instead of about 20 s: a `/move` step lost leadership while `reconfigure` ran, answered `"NotLeader(..)"`, and the in-band finish stopped there until the alarm finished it [measured, "audit authz:7500011 in band: repo:1507336 move: "NotLeader(…)"", `bf30-a.log`]. Both are fixed in `b18894e`, which went out with site day 5 and edge deploy 4.

**GHES 3.22.** `validate.py` checked the 202 `minimal-repository` from runs a, b, and c (3 REST replies) against `ghes-3.22.json`, with 0 failures [measured].

### Planted bugs: 9 of 9 caught

`tests/m3/finish/plant.sh`, at `087e296`. Each plant was applied alone and reverted after the gate tests ran [measured, `tests/m3/raw/finish/plant-bf30.log`].

| Plant | What it breaks | Caught by |
| --- | --- | --- |
| `settings-skips-deps` | transfer `Settings` records the owner without checking the owner's `Authz` names the repository | `the_transfer_settings_moves_deps_to_the_new_owner_and_its_enterprise`, `an_enterprise_change_moves_only_the_enterprise` |
| `settings-declares-no-deps` | transfer `Settings` declares no dep | the same two |
| `audit-skips-authz-cursor` | `Audit` owes nothing for a `Reserved` name | `audit_alone_finishes_a_transfer_whose_finisher_died_after_reserved` |
| `authz-cursor-never-moves` | the `Authz` cursor event applies nothing | that test, and `the_authz_audit_cursor_is_operator_only_monotone_and_format_4` |
| `move-drops-leader-first` | a swap drops the leader's site while another site could go | `a_move_to_a_placement_sharing_two_sites_swaps_one_and_keeps_writing` |
| `leave-without-transfer` | a `Joint`'s leader on the dropped site leaves without a transfer | `a_leader_on_the_site_a_joint_drops_transfers_before_it_leaves` |
| `settings-before-move` | `Settings` goes before the group's move | `audit_alone_finishes_a_transfer_whose_finisher_died_after_reserved` |
| `settings-ignores-enterprise` | an enterprise change compares only the owner | `audit_finishes_an_enterprise_change_on_every_repository` |
| `id-name-live-only` | `/repositories/{id}` finds only a `Live` name (run a's 404) | `audit_alone_finishes_a_transfer_whose_finisher_died_after_reserved` |

### Formats and tags taken

- Repository format 32.10 (`REPO_FORMAT_TRANSFER`), with `RepoCmd::Transfer` and record tag 0x54.
- Authz format 0x12.4 (`AUTHZ_FORMAT_V4`), with `AuthzEvent::AuditCursor` tag 16 and `AuthzOp::AuditCursor` tag 29.
- The edge sends a transfer only with `32.10` and `18.4` in `FORMATS_ENABLED`. (18.4 is 0x12.4 in decimal: `Format`'s family is printed in decimal.)

### Deployed for BF-30

| Script | Version | When | What |
| --- | --- | --- | --- |
| `graft-m1-site-a` | `5558864f-bc87-4018-8378-333c0a4436c0` | site day 5, 05:13–05:22Z | `b18894e`: 32.10 and 0x12.4 read, `/move`, `/finish`, the `Audit` `Authz` alarm, and the `NotLeader` retry |
| `graft-m1-site-b` | `7e6fe77c-1439-493b-a0f0-4d5982506c6c` | site day 5 | the same |
| `graft-m1-site-c` | `2a4c2784-2cac-4b24-9da5-65fbaa78776c` | site day 5 | the same |
| `graft-m1-site-d` | `77456b3c-65cd-4293-be54-58f6543b5c45` | site day 5 | the same; new in site day 4 (04:01–04:07Z, first version `c169bddc`), in `wnam` |
| `graft-m1-edge` | `ccede660-54bb-4200-9087-1076a5d22e4a` | 05:23Z | binds site d; `FORMATS_ENABLED` adds `32.10,18.4`; `id_name`; the bed route's repository read |
| `graft-m1-wp7-edge` | `63fdbcf6-b0e5-4a5c-9f06-a27d88775787` | 04:12Z | the tools edge, redeployed with the four-site table so it binds site d |
| `graft-m1-ops` | `ca658d46` | | unchanged |

- Site day 4 put sites a to c on `24e98e3` (a `918e466a`, b `5f93c0ba`, c `0037732f`) and created site d with its bucket `graft-m1-packs-d` and queue `graft-m1-prsync-d`.
- Edge deploys 2 (`cc2ccb56`) and 3 (`6de612aa`) came before deploy 4.
- Both site days ran with `GRAFTYARD_AUDIT=1` and `--roll-shards` [measured, `siteday4.log`, `siteday5.log`, `edge2.log`–`edge4.log`, `tools-edge.log`, `wrangler deployments list`].
- **Site day 5's `--roll-shards` step did not finish.** After the eighth deploy, the Containers API call that lists `graft-m1-site-c-gitshard`'s instances timed out (curl exit 28), and `deploy.mjs` exited 1. Every site version had already deployed by then. The roll had nothing to do: every running shard on sites a to d (41, 14, 1, and 5 instances) already ran the pinned image `…57f7e90c0891`, checked afterwards through the same API [measured].

### Checks

At `087e296` every check passes [measured, `tests/m3/raw/finish/checks.log`]:
- `cargo fmt --check`;
- workspace clippy;
- wasm32 clippy for site and edge, with and without test-faults, and for ops;
- `cargo test --workspace`: 498 passed, 0 failed;
- `check_design.py`, `check_names.py`, and `check_links.py`.

Token scan of `tests/m3/raw/finish/` for `ghp_`, `ghs_`, `gho_`, and `ghu_`: 0 matches. No process argv held a token. The driver keeps its PATs in memory and passes them in headers [measured].
