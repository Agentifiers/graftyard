# Milestone 3's close on graft-m1

This round closes Milestone 3 on the graft-m1 install. It covers:
- the stateless wire form for issue and merge commands;
- the hosted `Audit` consumer;
- the rows that ran only in memory (BF-3, BF-30, BF-33's leader kill, BF-34, BF-35, BF-36), run hosted as far as the routes reach;
- the small gaps from `m3-hosted.md`;
- F51;
- BM-3, the Milestone 3 gate.

Everything ran on `graft-m1-*`. Nothing touched `graft-m2b-*` or `graft-m1-lin-*`. Branch `m3-close`, from main `0e7456a`.

## What was deployed

| Script | Version | What changed |
| --- | --- | --- |
| `graft-m1-site-a` | `7fe1aaa9-b084-482a-98df-04b63c189d2d` | Site day 3: code at `420ae12`, `GRAFTYARD_AUDIT=1`; was `10430b64` |
| `graft-m1-site-b` | `70748bab-462b-45c0-a0c2-79554d94cf15` | Same; was `307605cc` |
| `graft-m1-site-c` | `438f715c-dc37-474c-b015-0b2ad097afd4` | Same; was `9c4bf22c` |
| `graft-m1-edge` | `8a824825-df78-4c56-81d1-1df3c07581cc` | Format 32.9, `POST /login/oauth/link`, and two bed-route additions; was `07e0176d` |
| `graft-m1-ops` | `ca658d46-850d-4c02-8e0d-463a46b0f903` | Unchanged |

[measured, `wrangler deployments list` at 18:44Z]

- **Site days.** There were three site days, each deploying each site twice (two phases).
  - Day 1 rolled the git shards onto an old image, `…39b56e46b43e`, by mistake.
  - Day 2 rolled them back to `…57f7e90c0891` and shipped the Audit code with Audit off. Its versions were `71178400`, `a44a83be`, `7534c027`.
  - Day 3 rebuilt from `0479288`, whose site code equals `420ae12`'s, and set `GRAFTYARD_AUDIT=1` [measured, `tests/m3/raw/close-siteday{1,2,3}.log`].
- **Day 3's shard roll.** It ended early: `catchup` for repository 196625 got no answer within curl's 300 s. Afterward, all 42 running shard instances were on `…57f7e90c0891`, 196625's included. Only stopped instances still name older images [measured, the Containers API, 18:03Z].
- **Edge versions.** The edge went through `5a651065` (32.9) and `b6e639de` (the link route and the bed read) before `8a824825` (the bed route's `kill`).
- **Kid 2.** Kid 2's `PutVerifyingKey` is on `tokens:0` at index 24, beside kid 1's. The `MintWrapKey` secret still holds kid 1 ("`mint-key rotate`" below).
- **Hosted state added.**
  - The App `m3-triage` (1179648), installed on `m3org` as 7190002 with `issues: write`.
  - The install account (1): its `Placement` row, its `Authz` chain opened, and `m3-admin` as its admin.
  - The account `m3-suspend` (7290001) and its repository `bf3` (1376256).
  - The accounts 7300001 to 7300009 and 7301001 to 7301009 from `rows.ts`.

## What was built

1. **Stateless wire form for issue and merge commands** (`65a7b07`, `1815c70`).
   - An issue command under a `ghs_` token is sent in repository format 32.9 and carries the verified `InstallationClaims`.
   - `decide` authorizes it with `authorize_app` against the installation's current row. A permission the token lacks on a repository it reaches answers 403 "Resource not accessible by integration". A private repository it does not reach answers 404.
   - GitHub documents both. "Resource not accessible by integration" means "your token has insufficient permissions". A 404 answers a private resource "to avoid confirming the existence of private repositories" [doc, docs.github.com, "Troubleshooting the REST API"].
   - A merge by an App answers 403 at the edge, because `Merged` names a `UserId`.
   - `an_installation_tokens_issue_writes_follow_its_app_permissions` fails on the old code.
   - 32.8 is AM1-WP2's agent push, so this form took 32.9.
2. **The hosted `Audit` consumer** (`776eaa4` pure, `023116e` site).
   - `AuditCursor` on `Names` and `Tokens` shards, formats 0x10.3 and 0x11.3. These are Names event tag 13 and op tag 17, and Tokens event tag 12 and op tag 13.
   - The cursor sits in a nested trailing part, so a fold with no cursor keeps main's bytes.
   - `audit::scan` reads only entries at least `FINISH_AFTER` (60 s) old. It names what each owes: a credential's `AddCredential`, an App slug's `PutApp` or `CloseApp`, and an account's `AccountLogin`.
   - The `Finisher` plans each item with `finish::credential`, `finish::app_registration`, or `finish::account_login`, and commits it. It moves the cursor last, or once `CURSOR_STRIDE` (1,000) entries have passed.
   - On a site, the leader's alarm runs one batch in band (61 s after it applies entries), with no queue re-send.
   - `finish::transfer` is not submitted: it needs the transfer `Settings` command and the move Workflow, neither built.
3. **F51** (`212aa36`). A status's id is now max(the edge's minted id, the highest id on its commit + 1), so ids on one commit follow commit order whatever the edges' clocks say. The reply renders the id `decide` returned (`IssueOut::Id`). `a_later_status_from_an_edge_whose_clock_trails_still_wins` fails on the old rule.
4. **`after_signup`** (`1fce620`). Signup's follow-ups now copy the install account's `Placement` record into the new owner's `InitPlacement`, as DESIGN says. No edge signup route exists yet, so `setup.ts` still writes the row by hand.
5. **`graftyard admin bootstrap`** (`0479288`, CLI `cli/src/admin.rs`).
   - It draws a 32-character link and writes it to a new 0600 file.
   - It writes the operator's `PutCode { kind: SignInLink, account, client: None }` for the link's code shard as a request file.
   - The edge's new `POST /login/oauth/link` reads the row, spends it once (`UseCode`), and commits `SignIn { method: Link }` on the account's `Identity` chain. It answers 201 with the session id.
   - The browser cookie and the page after it are not built.
6. **`graftyard mint-key rotate | step`** (same commit).
   - `rotate --kid N` draws a signing key and a wrap key. It writes the `MintWrapKey` text (`graftyard_ops::wrap`) to a new 0600 file, and writes `PutVerifyingKey` for kid N as a request file.
   - `step` runs `rotation_step`. It says wait, or "switch now" (write the secret with the customer's Wrangler login). When the old key is due to retire, it writes the old key's `PutVerifyingKey { retired }`.
   - Two CLI tests check:
     - the link is the operator's and only the operator's;
     - the secret file's mode is 0600 and is never overwritten;
     - the wait, switch, and retire times, and that the leader accepts the retire.
7. **Hosted drivers** (`tests/m3/hosted/`): `bootstrap.ts`, `bf33-kill.mjs`, `rows.ts`, `bf3-suspend.sh`, and `tools/src/bin/authzfold.rs`, which decodes an `Authz` fold from its `dep` read.
   - The edge's test-only `/_bed/identity` route can now read a chain (`{"read": q}`).
   - The route's first send now carries `kill` and `down_ms`, as `/_bed/push` does.

## Planted bugs

`tests/m3/close/plant.sh` applies each plant alone, runs the gate tests, and reverts. All 8 were caught [measured, `tests/m3/raw/close-plant.out`]:

| Plant | Caught by |
| --- | --- |
| cursor-before-work: the cursor moves before the batch's items | `audit` tests |
| no-minute: `scan` reads entries younger than `FINISH_AFTER` | `audit` tests |
| ssh-key-not-owed: an SSH key's `AddCredential` is not owed | `audit` tests |
| cursor-by-anyone: a user may move the cursor | `audit` tests |
| cursor-moves-back: `decide` takes a lower cursor | `audit` tests |
| cursor-chases-itself: no stride rule, so cursor entries owe a batch forever | `audit` tests |
| app-permission-ignored: `authorize_app` skips the permission | `an_installation_tokens_issue_writes_follow_its_app_permissions` |
| status-id-from-edge-clock: F51's old rule | `a_later_status_from_an_edge_whose_clock_trails_still_wins` and 2 more |

The handoff counted 9 plants. The file holds 8.

## The hosted rows

### Stateless issue writes. Pass, 10 of 10 each.

`tests/m3/hosted/ghs-issues.mjs`, with `@octokit/app`:
- `m3-triage` (`issues: write`) created a label on `m3org/j4`: 201, 10 of 10.
- `m3-ci` (`contents` only) got 403 "Resource not accessible by integration", 10 of 10.

[measured, `tests/m3/raw/ghs-issues/summary.json`]

`validate.py` against the GHES 3.22 description passed the 10 label creates and the membership read after BM-3. It failed the 10 refusals, because the description lists `201`, `404`, and `422` for `POST /repos/{owner}/{repo}/labels` and no 403 [measured, `tests/m3/raw/close-validate.txt`]. The same description lists 403 for `POST .../issues`, `POST .../issues/{n}/comments`, `PATCH .../issues/{n}`, and `PUT .../pulls/{n}/merge`, but not for `POST .../issues/{n}/labels`.

GitHub's troubleshooting page says a "Resource not accessible by integration" error means the token "has insufficient permissions" [doc]. Public reports show it returned as HTTP 403 by github.com [doc, `github/rest-api-description` #6994; community discussion 38215]. So the status matches what GitHub sends, and the description leaves 403 out of the label operation [inference]. Whether GHES 3.22 itself sends 403 or 404 there is [unverified]: no GHES is at hand. The edge serves issue and comment creation only through GraphQL (gh's mutations). Among the REST issue-family writes, it serves only the label create [measured, `workers/edge/src/api.rs`'s routes], so the operations whose 403 the description does list were not exercised over REST.

### Audit, hosted. Pass, 1 of 1.

- At 18:03:34Z, a PAT for `m3-member` was stored on `tokens:765` with no `AddCredential`.
- At 18:04:33Z, `m3-member`'s `Identity` chain listed 42 credentials, not the PAT.
- At 18:05:45Z it listed 43, the PAT among them, titled `Pat` [measured, `tests/m3/raw/close-audit/identity-{before,after}.json`].
- Workers Observability shows the leader of `tokens:765` on site a:
  - at 18:03:41Z, `audit tokens:765 0..3 2069ms {"owed":1,"refused":0,"steps":0}`: the entry was not yet a minute old;
  - at 18:04:38Z, `3..6 916ms {"owed":1,"refused":0,"steps":1}`.

  [measured, `tests/m3/raw/close-audit/obs-audit-tokens765.json`]
- In the same 6 h window, six `names:` shards logged a batch with `refused: 0`, and no batch logged a refusal.
- **A gap the consumer found.** From site day 3 on, the leaders of `tokens:2348` and `tokens:2469` logged `AccountId(DbId(7000003)) has no placement record` and the same for 7000004, every 2 s to 30 s: 1,000 lines in 3 h [measured, Observability].
  - These are the Milestone 1 seed accounts `wp6-reader` and `wp6-revoked`, whose PATs predate M3. They had no `Placement` row and no `Identity` chain, so `finish::credential`'s `AddCredential` had nowhere to go.
  - `tests/m3/hosted/m1-identity.sh` wrote both for the four seed accounts (`7000001` already had its row). The errors stopped at 18:57:56Z, and the next batches logged `{"owed":1,"refused":0,"steps":0}` [measured, `tests/m3/raw/close-audit/m1-identity.log`].
  - `steps: 0` fits `tokens:2469`: `wp6-revoked`'s PAT is revoked there, so nothing lists it. The entry on `tokens:2348` names another token of 7000003's, which I did not identify [unverified].
  - `wp6-reader`'s seeded PAT is on `tokens:1153`. At 19:08Z, after the backfill, `wp6-reader`'s `Identity` chain listed it, titled `Pat`. It listed nothing before, because the chain did not exist [measured]. That listing is the `Audit` consumer's own work, on a token stored in Milestone 1.
  - A failed batch leaves the cursor where it is and retries after `RETRY_MS` (30 s, a [guess] in `workers/site/src/audit.rs`), with no limit. That is why the gap was visible in the log at once, and also why it would retry for as long as the gap lasted.

### `graftyard admin bootstrap`. Pass, 11 of 11.

`tests/m3/hosted/bootstrap.ts` ran the CLI, committed its request through the tools edge as the operator, and used the link at the edge. Over 11 runs (1, then 10):
- `PutCode` committed, 11 of 11;
- the first use answered 201 with `m3-admin` and method `Link`, 11 of 11;
- a second use of the same link answered 401, 11 of 11;
- a link one character off answered 401, 11 of 11.

[measured, `tests/m3/raw/close-bootstrap/run{1,10}.log`]

The install account had no group on `names:24` or `authz:1`, because the test install has no genesis acceptance. The script wrote the install account's `Placement` row and opened its `Authz` chain, as `after_genesis` would.

### `graftyard mint-key rotate`. Started; the switch waits an hour by design.

- At 18:32Z, `graftyard mint-key rotate --kid 2` wrote kid 2's `MintWrapKey` text (mode 0600) and its request.
- The request committed on `tokens:0` at index 24. `mint-key step` then answered "wait: kid 2 signs from 1794079972120 ms" (19:32:52Z), one `INSTALLATION_TOKEN_TTL` after the row [measured, `tests/m3/raw/close-mintkey/`].
- A mint afterward still worked under kid 1: 344 characters, header `{"alg":"EdDSA","typ":"JWT","kid":"1"}`. Both rows are on `tokens:0` with `retired: null` [measured, `mint-after-kid2.txt`].
- Not done in this round:
  - the secret switch (`wrangler secrets-store secret update` with the kid 2 text);
  - a mint under kid 2;
  - kid 1's retire, which falls at the switch plus `SIGNING_KEY_CACHE`, `INSTALLATION_TOKEN_TTL`, and `CLOCK_SKEW`: over 2 h after the switch.

  The rotation is tested end to end in memory (M3-WP4, `crates/identity/tests/wp4.rs`).

### BF-33: device flow with the code shard's leader killed. Pass, 120 of 120.

`tests/m3/hosted/bf33-kill.mjs`, as gh's client (`178c6fc778ccc68e1d6a`). Each run:
1. A device code is requested and approved as `m3-member`.
2. The exchange's `UseCode` is sent through the edge's bed route, with `kill` on its first send to the code shard's leader.
3. gh's `POST /login/oauth/access_token` runs twice.

| Set | Kill | Runs | Kill fired (a re-send after a miss) | Exchange | Replay |
| --- | --- | --- | --- | --- | --- |
| 1 | `after_quorum` and `before_commit_sent`, alternating; every 5th held down 12 s | 100 | 100 | `expired_token`, 100: login restarts | no token, 100 |
| 2 | `before_commit_early` | 20 | 20 | `expired_token`, 20 | no token, 20 |

[measured, `tests/m3/raw/close-bf33/{run100,early20}.jsonl`]

- No run produced a token from a spent code, and none failed in another way. Each run ended as BF-33's "restarts cleanly".
- In every run, the killed command committed: the edge's re-send under the same request id found its outcome. So the "completes" branch with one token was not reached hosted.
  - `before_commit_early` was meant to lose the entry, but every run committed it anyway [inference: the followers held the early AppendEntries and the new leader committed it].
  - The completion path with one token per login is gh's 20 of 20 in `m3-hosted.md` and the in-memory 100 of 100 (M3-WP3).
- The runs held down 12 s took 12.7 s to 12.8 s and 24 sends.
- The design's other cases also hold hosted: a used code's replay is refused, and an expired code fails. The `device-replay.mjs` runs in `m3-hosted.md` cover them.

### BF-3: a suspended user's pushes. Pass, 200 of 200. One key from two edges: pass, 100 of 100.

- **Suspension.** `tests/m3/hosted/bf3-suspend.sh`, with stock git and the user's PAT, on `m3-suspend/bf3`:
  - the push before suspension was accepted;
  - after `PutAccount { suspended: true }`, 200 of 200 pushes were refused 401 with the ref not moved;
  - after the suspension was lifted, a push was accepted.

  [measured, `tests/m3/raw/close-bf3/run200/`]

  The first attempt is kept as `run200-nonff/`. Its control push was refused as a non-fast-forward, from a reused remote, so it proves nothing. The script now starts from the remote's `main`.
- **One SSH key added from two requests at once.** `rows.ts` added one fingerprint for two accounts at the same moment, 100 times, through the tools edge [measured, `tests/m3/raw/close-rows/run-b.log`]. The result is in "BF-3, the key half" below.
- **Not hosted.** BUILD-PLAN's in-memory form held 100 pushes waiting at the suspension. Hosted pushes ran one after another. The SSH push is BF-52 (M6-WP2).

### BF-36: InstanceAuth switch. Pass.

On the install account's `Authz` chain (`rows.ts`, twice) [measured, `tests/m3/raw/close-rows/try-a.log`, `run-b.log`]:
- `SetInstanceAuth(Oidc)` from `m3-member` was refused `Forbidden`, from the operator `Forbidden`, and from `m3-admin` (the install org's admin) committed.
- The next `SignIn` was:
  - `WrongMethod` by Password;
  - a session by OIDC;
  - a session by Link.
- `Builtin` was restored, and a Password sign-in then started a session.
- No OIDC provider is wired to the edge, so "the next sign-in uses OIDC" ran as the `Identity` command the edge would send after verifying one.

### BF-34: invitations. The org half passes. The repository half and Terraform are not hosted.

- **Org half.** `rows.ts`, twice, on fresh orgs:
  - an invitation's `Accept` by the wrong user was `Forbidden`;
  - the invitee's `Accept` committed, and the fold names it `Member`;
  - the other invitation's `Decline` committed, and the fold names no role;
  - a later `Accept` of the declined one was `NoInvitation`.

  [measured, `authzfold` on the fold's `dep` read]
- **Repository half.** Outside collaborators are not hosted: the edge has no `rest:collaborators` route. `InviteCollaborator` and `AnswerInvitation` are repository commands with no hosted sender. `TestAccGithubRepositoryCollaborator` needs that route.
- **The member write.** "A `Collaborator` write for an org member is refused" is the same command, so it is not hosted either.

### BF-30: transfer and enterprise change. The enterprise half passes. The transfer is not hosted.

- **Enterprise half.** `SetEnterprise` from an org admin was `Forbidden`. From the operator it committed, a re-send committed with no change, and the fold names the new enterprise [measured, `rows.ts`, twice].
- **Transfer.** Not hosted: it needs the transfer `Settings` command and the move Workflow driving the group through `Joint`, and neither is built.

### BF-35: a new PAT and SSH key listed at once. Pass.

`rows.ts`: a PAT's `Store` then `AddCredential`, and an SSH key's `AddSshKey` then `AddCredential`, each pair read from the `Identity` chain right after.

| Run | Pairs | PAT listed | Both listed |
| --- | --- | --- | --- |
| `try-a` | 3 | 3 | 3 |
| `run-b` | 100 | 100 | 100 |

[measured]

- BF-35's clients, `gh ssh-key list` and `GET /user/keys`, have no route on the edge. The read is the `Identity` fold those routes would render.
- Audit was on during the run, but it finishes only entries over 60 s old, and each pair was read within seconds of its commits.

### BF-3, the key half

| Run | Keys | One owner | `KeyTaken` for the other |
| --- | --- | --- | --- |
| `try-a` | 3 | 3 | 3 |
| `run-b` | 100 | pending | pending |

[measured]

### Rows that cannot run hosted, and why

- **BF-30's transfer.** The `Settings` command and the move Workflow are not built.
- **BF-34's repository half and `TestAccGithubRepositoryCollaborator`.** The edge has no `rest:collaborators`, `rest:teams`, or repository-invitation routes. Terraform's GitHub provider calls them.
- **BF-35 through `gh ssh-key list` and `GET /user/keys`.** The edge has no `GET /user/keys` route.
- **BF-36 with a real OIDC provider.** No OIDC sign-in route exists. The method check runs on the `Identity` chain, as above.
- **BF-3's SSH half.** The SSH front is M6.

## BM-3: journey 4 over HTTPS. Pass: 200 of 200 revoked tokens refused on the next write, 100 of them stateless

`tests/m3/hosted/journey4-close.sh`, run as the orb service `m3c-j4` on edge `8a824825` and the day-3 sites. It ran 100 rounds against `m3org/j4` with stock git. Each round:
1. A fresh PAT for `m3-member` pushes, then is revoked (`DELETE /applications/{gh}/token`), then pushes again.
2. A fresh `ghs_` from installation 7190001 pushes, then is revoked (`DELETE /installation/token`), then pushes again.

At round 50, inside the run, `m3-admin` removes `m3-member` from `m3org`. The member's live PAT then pushes, and `m3-admin` adds the member back. A refusal counts only if git exits non-zero and the remote ref did not move.

| Step | Count |
| --- | --- |
| PAT pushes accepted | 100 of 100 |
| PAT revokes, 204 | 100 of 100 |
| Pushes with the revoked PAT refused, 401, ref not moved | 100 of 100 |
| `ghs_` pushes accepted | 100 of 100 |
| `DELETE /installation/token`, 204 | 100 of 100, none re-sent |
| Pushes with the revoked `ghs_` refused, 401, ref not moved | 100 of 100 |
| Push with the removed member's live PAT, round 50 | refused 403, ref not moved |

[measured, `tests/m3/raw/close-j4/journey4.tsv`, `revokes.tsv`, `journey4.summary`; the run took 18:19:06Z to 19:19:52Z, about 36 s a round]

- **Round 50.** Removal 204. The member's push was refused 403, ref not moved. Re-add 200.
- **The `ghs_` revokes.** curl's total time for each `DELETE /installation/token` was 0.40 s at least, 0.46 s median, 0.64 s p99, and 0.94 s at most. None answered `000`, so none was re-sent. They reached two Cloudflare addresses, `172.67.202.67` 56 times and `104.21.93.11` 44 times [measured, `revokes.tsv`].
- **Earlier runs.** A run started at 18:08Z was stopped after round 5 so the edge could be redeployed. All its steps were correct, and it is kept as `close-j4-stopped`. An earlier 2-round run, `close-j4-aborted`, was also correct.

### Round 97 of the last run

In the last run (`m3-hosted.md`), round 97's `DELETE /installation/token` got no reply in 120 s. Workers Analytics for `graft-m1-edge` shows:
- 44 requests at 06:40Z;
- 35 at 06:41Z;
- 3 at 06:42Z;
- no row at all for 06:43Z;
- 28 at 06:44Z.

[measured, `tests/m3/raw/close-round97-analytics.json`]

Observability logged no edge request from 06:42:11Z to 06:44:12Z. The request never reached the edge, so the loss was on the client's side or the network's, not the edge's or a group's [inference]. A lost request that was never acknowledged is not a revoked token accepted: that token was never revoked, and its next push was rightly accepted. `journey4-close.sh` now records curl's connect, TLS, first-byte, and total times and the remote IP for every revoke, and sends a `000` once more under the same token. This run's 100 revokes all answered at once, so round 97's loss did not recur in 100 tries.

## The learner half of BM-3

BM-3's second sentence is "Learners rerun the Milestone 0 learner gate." B0-18 passed 6 of 6 on `graft-m2b` with all 19 members reset onto `e174090` (`tests/m3/results/m3-wp5.md`). Since `e174090`, the code a learner runs has these changes:
- `crates/raft`, `crates/lookup`, `crates/types`, and `workers/site/src/host.rs`: nothing;
- `workers/site/src/member.rs`: 3 lines that host the `Identity` family (`9c48e48`);
- `workers/site/src/family.rs`: new reads and the Audit alarm on the identity families (`9c48e48`, `248a837`, `65a7b07`, `1815c70`, `023116e`).

The repository family a learner serves changed only by reading 32.7 and 32.9, the issue formats, which B0-18's read-index load never sends. So the m2b 6 of 6 counts for BM-3 [inference, from `git log e174090..HEAD`]. It was not rerun on graft-m1's build.

## F51

Settled in code by `212aa36`: the repository group's `decide` assigns the id, above every status the commit holds. The edge's clock now only orders statuses on commits the group has none for. This is the "commit order" answer F51 itself names [inference: it uses the commit's own rows rather than an `IdBlock` `DbId`, so it needs no id lease]. The test proves the rule with two edges a minute apart.

The hosted rerun of BF-23 through two edges with skewed clocks was not run, because the edge cannot fake its clock. F51's row in `decisions/OPEN-QUESTIONS.md` is the design team's to close, with this commit as its evidence.

## Checks

At `8c1092e` [measured, `tests/m3/raw/close-checks.log`]:
- `cargo fmt --check`: clean.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- wasm32 clippy: clean for `workers/site` and `workers/edge`, with and without `test-faults`, and for `workers/ops`.
- `cargo test --workspace`: 466 passed, 0 failed, 3 ignored.
- `check_design.py`, `check_names.py`, `check_links.py`: OK.

## Tokens and logs

- Every token reached git through a credential helper reading a 0600 file, and curl through a header on stdin.
- The bootstrap link went from a 0600 file into a form body. The `MintWrapKey` text went only to `~/.graft-m3h/mintkey-kid2.json` (0600).
- `rg "gh[pousr]_[A-Za-z0-9]{20,}|github_pat_|BEGIN [A-Z ]*PRIVATE KEY|eyJ[A-Za-z0-9_-]{20,}"` over `tests/m3/raw` (without the two linked reference trees), `tests/m3/hosted`, `cli`, `workers/edge/src`, and `workers/ops/src` matched one line: the test key fixture already in `cli/src/config.rs`. No 32-character link and no wrap key appears in the raw logs.
- Workers Observability, last 6 h, has no event for `gh[pousr]_…`, `github_pat_`, `eyJ…`, `PRIVATE KEY`, `client_secret`, `device_code=`, `link=`, `"wrap"`, or `wrapped` [measured, `tests/m3/raw/close-obs-scan.txt`].

The scan ran again after BM-3, BF-33, and BF-3. Over the same trees plus `tests/m3/results`, it matched only the patterns written as text, in this file and `close-obs-scan.txt`, and the fixture in `cli/src/config.rs` [measured].

## Design gaps and contradictions found

- **The Audit cursor's chain.** SHARED, "Event fan-out": "`PrSync` holds its position as a `RepoEvent::ConsumerCursor` row in the chain's core tier, committed with the work it covers, and the other consumers that commit to the chain they read do the same [inference]".
  - `Audit` reads `Tokens` and `Names` but commits to `Identity`, `Authz`, and `Names`. Its cursor cannot be committed with the work it covers.
  - It is committed on the chain it reads, after the work. A batch lost before the cursor runs again and re-sends idempotent steps.
  - The sentence does not cover a consumer that reads one chain and writes others.
- **Groups created on first use, against sign-in latency.** DESIGN, Failure modes: "Members and learners are created by name on first use." This round measured it again:
  - A new `Tokens` or `Names` group took 13 s to 14 s to create through the tools edge [measured, Observability `/create` wall times].
  - `rows.ts`'s BF-35 pairs took about 25 s each, almost all of it creation.

  This is R20 (main `74ffceb`). Proposed answer: install creates every `Tokens` shard's group, and `Names` shards on first use stay as they are [guess].
  - An idle group bills nothing (K6).
  - 3,072 creations at install, at 13 s each, take hours in sequence but minutes if run in parallel.

  Not built, because it is the design team's call.
- **The transfer.** BUILD-PLAN M3-WP3 lists "Repository transfer and an org's enterprise change (SHARED, \"Multi-chain operations\"), with the owner placement move of M1-WP7". The transfer's `Settings` command and the move Workflow are still not built, so `finish::transfer` has nothing to finish and BF-30's transfer cannot run hosted.
- **The first admin's link.** DESIGN, "Install operations": "`graftyard admin bootstrap` asks `graft-ops` to mint a one-time sign-in link". `graft-ops` has no operator route (R14). The CLI therefore mints the link itself and writes the operator's `PutCode` request, which is the same row.
  - DESIGN does not say what the link opens. This build answers `POST /login/oauth/link` with a session id and no cookie [inference].
  - SHARED's `SignInMethod::Link /* `graftyard admin bootstrap`, whatever InstanceAuth says */` holds: the hosted `SignIn` by Link committed under OIDC.
- **BF-33 hosted cannot reach "completes".** BUILD-PLAN BF-33: "Each login completes or restarts cleanly". With the kill on the exchange's `UseCode`, the edge's re-send always found the command committed, under all three kill modes. Hosted, the kill shows "restarts cleanly" 120 of 120 and never "completes".
- **The install account on a test install.** DESIGN Genesis step 5 opens `Authz(Genesis.account)`. The graft-m1 test install has no genesis acceptance, so `names:24` and `authz:1` existed only after `bootstrap.ts` wrote them. BF-36 on the install's chain therefore ran on a hand-opened chain.
- **The plant count.** The handoff counted 9 plants. `tests/m3/raw/close-plant.out` holds 8, all caught.
