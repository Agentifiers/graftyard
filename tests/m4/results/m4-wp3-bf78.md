# BF-78 (`on: push`) on graft-m4a

Status (2026-11-26 00:56Z): **r2 passes every line.** The lead gave GO on main `26237276`. The window ran from 00:28Z (`flag`) to 00:56:25Z (teardown, the done line), and the row spent $0.31 against its $3 stop, by `acctcost.py` with prefix graft-m4a from 00:28:21Z. r1 (00:07:55Z, $0.99) was not met only on `github.event`'s `pusher` and `sender`, which read `ghost`, and the lead accepted its runs, filters, event fields and tokens. Raw logs are in `tests/m4/raw/wp3/bf78/r2/` and `r1/`.

## r2: verdict

| Push | Ref | Runs started (expected the same) | `github.event` | Token `sub` |
| --- | --- | --- | --- | --- |
| p1 change under `src/` | `refs/heads/main` | any, branches, ignore, paths | `created` false, `forced` false | `repo:m3org/bf78-r2:ref:refs/heads/main` |
| p2 docs only | `refs/heads/main` | any, branches, ignore | as p1 | as p1 |
| p3 new branch | `refs/heads/rel/1.0` | any, branches | `created` true | `...:ref:refs/heads/rel/1.0` |
| p4 new branch | `refs/heads/rel/1.0-wip` | any | `created` true | `...:ref:refs/heads/rel/1.0-wip` |
| p5 new branch | `refs/heads/dev` | any, ignore | `created` true | `...:ref:refs/heads/dev` |
| p6 lightweight tag | `refs/tags/v1` | any, tags | `created` true | `...:ref:refs/tags/v1` |
| p7 force push | `refs/heads/dev` | any, ignore | **`forced` true** | `...:ref:refs/heads/dev` |
| p8 delete | `refs/heads/dev` | none | | |
| p9 `[skip ci]` | `refs/heads/main` | none | | |

- **Runs** (`r2/push/result.json`, `ok` true): `exact`, `none_elsewhere` and `one_each` hold for all nine pushes, at the pushed SHA and ref. All 20 jobs succeeded on 20 ephemeral runners (unmodified 2.338.2), and no 503 was retried.
- **`github.event`**, p1 to p7: `ref`, `before`, `after`, `created`, `deleted` false, `forced` (true only on p7), `head_commit.id` the pushed SHA, `repository`, and no `added` key. **`pusher.name` and `sender.login` are `m3-admin` on all seven**, and on the setup push.
- **Token**, p1 to p7: verified by `rp.py` within seconds of its job writing it (RS256, discovery and JWKS 200 with `--agent`, F130). `sub` is `repo:m3org/bf78-r2:ref:<the pushed ref>`, `actor` `m3-admin`, `actor_id` 7100003, `ref` the pushed ref, `event_name` `push`. No token appears in a job log, and every token file is removed once verified.
- Each `any.yml` job printed `context event=push ref=<ref> sha=<sha> actor=m3-admin`.

## r2: what ran

| Step (Cloudflare time) | Result |
| --- | --- |
| `state`, 00:28Z | Sites a `c45101cb`, b `12ee6bf6`, c `1e21e393` (r1's `off`); edge `7b3bf05d`; ops `ac4cd9a5`, schedules `[]` |
| `flag`, 00:28Z | Site day from main `26237276` (the pusher fix, plus BF-76 head 1's Authz 0x12.15 reader, which nothing sends before its enable): a `89e5fa12`, b `3128617a`, c `3b7806db`, every live flag kept and `GRAFTYARD_PUSH_TRIGGER=1` added |
| `push`, 00:29Z to 00:54Z | `m3org/bf78-r2`, nine pushes, 20 jobs |
| `off`, 00:55Z | a `3d3e90ee`, b `a5411444`, c `b31691c8`: the same flags without `GRAFTYARD_PUSH_TRIGGER`; crons `[]` |
| `teardown`, 00:56:25Z | `m3org/bf78-r2` DELETE 204, anonymous read 404 |

m4a is left with:
- sites a `3d3e90ee`, b `a5411444`, c `b31691c8`, from main `26237276`, with every earlier flag (including `GRAFTYARD_REUSABLE=1`), push trigger off, crons `[]`;
- edge `7b3bf05d`, unchanged, whose FORMATS_ENABLED does not hold 21.12;
- ops `ac4cd9a5`, schedules `[]`.

## r1 (not met on `pusher` and `sender`)

r1's done line is 00:07:55Z, and it cost $0.99.

### r1: verdict

| Push | Ref | Runs started (expected the same) | `github.event` (bar `pusher`/`sender`) | Token |
| --- | --- | --- | --- | --- |
| p1 change under `src/` | `refs/heads/main` | any, branches, ignore, paths | ref, before, after, `created` false, `forced` false, `head_commit.id`, no `added` | accepted |
| p2 docs only | `refs/heads/main` | any, branches, ignore | as p1 | accepted |
| p3 new branch | `refs/heads/rel/1.0` | any, branches | `created` true, `before` zero | accepted |
| p4 new branch | `refs/heads/rel/1.0-wip` | any | `created` true | accepted |
| p5 new branch | `refs/heads/dev` | any, ignore | `created` true | accepted |
| p6 lightweight tag | `refs/tags/v1` | any, tags | `created` true | accepted |
| p7 force push | `refs/heads/dev` | any, ignore | **`forced` true** | accepted |
| p8 delete | `refs/heads/dev` | none | | |
| p9 `[skip ci]` | `refs/heads/main` | none | | |

- **Runs** (`push/result.json`): `exact`, `none_elsewhere` and `one_each` hold for all nine pushes. All 20 jobs (4 from the setup push, 16 from p1 to p7) succeeded on unmodified runner 2.338.2. Every `any.yml` job printed `context event=push ref=<ref> sha=<sha> actor=m3-admin`, and no token appears in a job log.
- **Tokens** (`rp-live.jsonl`): verified by `rp.py` (RS256 under kid `g-7DvKNw…`, discovery and JWKS both 200 with `--agent`, F130). The tokens for p1 to p7 each carried that push's `sub`, `actor`, `actor_id`, `ref` and `event_name` push.
  - The driver verified tokens only after every job had ended, and a token lives 300 s, so its own `token` check failed on p1 to p3 (expired). The tokens had been checked fresh by a watcher, and `push.mjs` now does the same, verifying each token within seconds of its job writing it.
  - The setup push's token (run 2627566) expired before the watcher started. It is not one of the nine pushes.
- **`pusher` / `sender` `ghost`**: the payload took logins from the repository's directory (`hookseq::directory`), which holds only the accounts the repository's rows name. A brand-new repository's rows name none, so the pusher rendered as `ghost`. The run's actor (`RunActor`, so the token's `actor` and `github.actor`) was right, because it reads the login from `Names`. The fix adds the pusher's login, read once, to the directory before the payload is built (`workers/site/src/actions_trigger.rs`, `push_runs`). No native test caught it, because the in-memory directory always names the pusher.

### r1: what ran

| Step (Cloudflare time) | Result |
| --- | --- |
| `state`, 23:37Z | Sites a `f7704ee3`, b `b6e66c3a`, c `7bbf3cbd` (AM2's handover); edge `7b3bf05d`; ops `ac4cd9a5`, schedules `[]`; install day $9.19 |
| `readers`, 23:38Z | Site day from main `5c1737ba`: a `dbfd0143`, b `be8958aa`, c `23529227`. Every flag kept, including BF-72's `GRAFTYARD_REUSABLE=1`; no push flag |
| `flag`, 23:40Z | a `48ed39c1`, b `3a51e5ec`, c `7a91b4a2`: only `GRAFTYARD_PUSH_TRIGGER=1` added |
| `push`, 23:41Z to 00:05Z | `m3org/bf78-r1`, nine pushes, 20 jobs on 20 ephemeral runners, no 503 retries |
| `off`, 00:07Z | a `c45101cb`, b `12ee6bf6`, c `1e21e393`: the flags as before `flag`, no `GRAFTYARD_PUSH_TRIGGER`; crons `[]` |
| `teardown`, 00:07:55Z | `m3org/bf78-r1` DELETE 204, anonymous read 404; every token file shredded |

m4a is left with sites a `c45101cb`, b `12ee6bf6`, c `1e21e393` (main `5c1737ba`, the 0x15.12 reader, every earlier flag, push trigger off), edge `7b3bf05d` (unchanged; FORMATS_ENABLED without 21.12), and ops `ac4cd9a5` with schedules `[]`.

## The row

The row (`build/BUILD-PLAN.md`, M4-WP3): workflows with `push` filters on branches, tags, `branches-ignore` and `paths`, then pushes that match and pushes that do not, including a tag push, a force push and a branch delete. One matching job requests `id-token: write`. Pass when:

- exactly the matching pushes start one run each, at the pushed SHA, with `github.event` as GHES 3.22 sends it;
- a branch delete starts none;
- the token's `actor` is the pusher and its `sub` is `repo:<owner>/<repo>:ref:<the pushed ref>`.

## What is built

- **Trigger** (`crates/actions/src/trigger.rs`, `push_events`, `push_run`, `skips_ci`): one push per ref an entry moves, read from `Refs`, `Merged` (the merger is the pusher) and the release's tag updates. A ref moved twice in one entry is one push with its last tip. Deletes, unchanged tips, queue refs, refs outside `refs/heads/` and `refs/tags/`, and the tags of an entry pushing more than three start none. A head commit with `[skip ci]` and the other markers, or a `skip-checks: true` trailer, starts none.
- **Site** (`workers/site/src/actions_trigger.rs`, `push_runs`): reads compare (forced when `behind_by > 0`), the pushed commits and the head commit from the git cache shard, builds `github.event` (`api::webhooks::actions_push`, with no `added`, `removed` or `modified`, as Actions' payload has none), takes paths from the two-dot diff when `before` is an ancestor, and calls `create_run` with the event. Only where a site day set `GRAFTYARD_PUSH_TRIGGER=1`.
- **Shard** (format 0x15.12): `ActionsOp::CreateRunPush` records the pusher (`RunActor`), the number (`RunNumber`) and `ActionsEvent::RunEvent` (the payload), and a reusable workflow's plan (`RunPlanned`) when it calls one. A push run's key names its ref, so one entry that moves several refs starts a run for each. Every op that creates a push run is sent as 0x15.12.
- **Job message**: `github.event` from the run's `RunEvent` (`runsvc::acquire_message`), `github.actor` from `RunActor`, so `$GITHUB_EVENT_PATH` holds the payload.
- Known gaps, recorded as F133: a new branch's and a force push's `paths` filters pass where GHES diffs against a base the git cache has no route for, and a new ref's `commits` list holds only the head.

## Checks at $0

- `crates/actions/tests/pr_trigger.rs`:
  - `one_entry_of_pushes_starts_a_run_per_matching_ref_at_its_commit`;
  - `push_events_skip_deletes_queue_refs_and_more_than_three_tags`;
  - `a_head_commit_with_a_skip_marker_skips_push_runs`.
- `crates/actions/tests/reusable.rs`, `a_planned_push_run_keeps_its_plan_and_its_event`.
- `shard.rs`, `a_push_run_records_its_event_and_keys_by_its_ref`.
- `crates/api/tests/webhooks.rs`, `an_actions_push_payload_carries_the_pushed_commits_and_forced`. Its payload passes the GHES 3.22 schema validator.
- plant255: 17 mutants. The trigger, key, format, actor, follower check, leader op, job message, `forced`, skip trailer, and push-with-plan paths.

## Plan (as written for r1)

One m4a window, about 45 minutes, in this order. Every step is `bash tests/m4/wp3/bf78/row.sh r1 <step>`, logged under `tests/m4/raw/wp3/bf78/r1/`. `state` runs before each step and reads the $3 stop from `cost.from`.

1. **`state`.** Each script's deployment, the hosts, `FORMATS_ENABLED`, ops schedules, and the install day's cost.
2. **`readers`.** A site day from the branch's merge commit on main (TREE), with 0x15.12's reader. It keeps the flags m4a carries: BF-71's, plus `GRAFTYARD_OIDC_HEAD_REF=1` and `GRAFTYARD_OIDC_CLAIMS=1`, and the workers.dev host. It has no `GRAFTYARD_PUSH_TRIGGER`. The sites already read 0x15.7 and 0x15.8 (AM2's kill-row day, 20:44Z, from 8a4c6397). So this day ships only 0x15.12, and 0x15.9 if bf72-reusable's day has not shipped it yet.
3. **Formats.** No edge code reads 0x15.12 and the edge sends no push op, so the row needs no edge day. F124's enable is recorded by adding 21.12 to `FORMATS_ENABLED` on the next edge day that happens anyway. AM2's next edge day adds 32.28; I'll ask AM2 to add 21.12 too. If the lead wants it inside this window, `edge` (ADDFMT=21.12) is one more edge day.
4. **`flag`.** A site day from the same TREE with `GRAFTYARD_PUSH_TRIGGER=1` added and nothing else changed. `bf74/row.sh site` prints the binding diff, which must show only that var.
5. **`push`** (`push.mjs`). It creates `m3org/bf78-r1` with five workflows on `main`, then makes nine pushes as `m3-admin` with `git push`. Before the first, it records the setup push's own runs and waits them out. The expected workflows are written out by hand from the filters:

   | Push | Ref | Expected runs |
   | --- | --- | --- |
   | p1 change under `src/` | `refs/heads/main` | any, branches, ignore, paths |
   | p2 docs only | `refs/heads/main` | any, branches, ignore |
   | p3 new branch | `refs/heads/rel/1.0` | any, branches |
   | p4 new branch | `refs/heads/rel/1.0-wip` | any |
   | p5 new branch | `refs/heads/dev` | any, ignore |
   | p6 lightweight tag | `refs/tags/v1` | any, tags (`paths` never applies to a tag) |
   | p7 force push | `refs/heads/dev` | any, ignore |
   | p8 delete | `refs/heads/dev` | none |
   | p9 `[skip ci]` in the message | `refs/heads/main` | none |

   For each push, it waits until no new run appears for 60 s. It then checks:
   - exactly the expected workflows ran, once each;
   - each run is `event` push at the pushed ref and SHA, and no other run appeared.

   Every job runs on an unmodified runner 2.338.2 on the orb, one ephemeral runner per job. Each `any.yml` job prints its context line and writes `$GITHUB_EVENT_PATH` and one ID token for audience `bf78` to the orb, never to a log. For each `any.yml` run, the driver then checks:
   - **`github.event`:** `ref`, `before`, `after`, `created` (p3 to p6), `deleted` false, `forced` (true only on p7), `head_commit.id` the pushed SHA, `pusher.name` and `sender.login` the admin's login, and no `added` key;
   - **the token**, through `rp.py`: `sub` `repo:m3org/bf78-r1:ref:<ref>`, `actor` and `actor_id` from `GET /user`, `ref`, and `event_name` push. `--agent` is set because the issuer is on workers.dev (F130);
   - the job succeeded, its log has the context line, and no token appears in a log.

   A 503 tierOff in the new repository's first minutes is retried every 2 s and recorded, and not counted.
6. **`off`.** A site day from the same TREE without `GRAFTYARD_PUSH_TRIGGER`, so no other row's push starts runs on m4a. Optional: if the lead prefers to leave push on, skip it and record that.
7. **`teardown`.** The repository is deleted, and the anonymous read afterwards must be 404. Nothing else on m4a changes.

**Cost [guess].** $0.3 to $0.8. That covers three site days (BF-73/74's three days and five jobs spent $0.09) and about 20 short jobs on the orb's runner. The stop is $3.

**Needs.**
- TREE: a worktree at main, with `worker-build --release --features test-faults` done in `workers/site` and logged in `$HOME/bf71/site-build.log`.
- RUNNER (actions/runner 2.338.2) and `target/release/examples/jitconfig`.
- `source tests/am1/lf13/env-m4a.sh`.
- No ops deploy token: no step deploys ops.

**Risks.**
- p1 to p9 assume the site's pump picks up each entry within the 60 s settle window. A slow pump shows up as runs counted under the next push, which fails `none_elsewhere` rather than passing silently.
- p3 and p5 are new branches. Under F133 their `paths` filter would pass, but no workflow with `paths` matches those branches, so the row does not depend on it.
