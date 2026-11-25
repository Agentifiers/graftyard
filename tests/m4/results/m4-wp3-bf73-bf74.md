# BF-73 and BF-74 on graft-m4a: plans

Status (2026-11-25): **built at $0, not run hosted.** Both rows wait for the lead's GO, after AM2's kill row on graft-m4a. Each row stops at $3, read from `cost.from` before every step (`tests/fanout/acctcost.py`, prefix graft-m4a).

## What each row proves

- **BF-73** (F119 to F122; `tests/m4/wp3/bf73/row.sh`). It runs three times, and each run mints a token for audience `bf73`:
  - a same-repository `pull_request` run;
  - a `workflow_dispatch` run;
  - a dispatch after `PUT .../actions/oidc/customization/sub` with `use_default: false` and keys `repo, context, actor, run_number`.

  `claims.mjs` takes each expected value from the install, not from the token:
  - `actor` and `actor_id` from the admin's `GET /user`;
  - `run_number` from the run's place among the workflow's runs in the repository's run list;
  - `head_ref` from the pull request's branch, and `base_ref` `main`;
  - `job_workflow_ref` and `sub` from the repository and ref.

  The relying party (`tests/m4/wp3/bf74/rp.py`) must accept the policy that names those values. It must refuse a policy naming another actor and one naming another branch, while the signature still verifies. The job must also print `github.actor` and `github.run_number` equal to the expected values.
- **BF-74** (F130; `tests/m4/wp3/bf74/row.sh`). The steps run in this order:
  - a Workers Custom Domain `m4a.graftyard.dev` for `graft-m4a-edge`;
  - the sites' `GRAFTYARD_HOST` and the edge's `HOST` set on it;
  - `graftyard oidc check` before the rule, which must read 403 1010;
  - `graftyard oidc bic-rule --host m4a.graftyard.dev`, run twice, where the second run must write nothing, then read back;
  - the check again, which must read 200;
  - one dispatched job's token;
  - `rp.py` with urllib's defaults. It discovers the keys from the token's `iss`, which must be `https://m4a.graftyard.dev/_services/token`.

## Run order (one m4a window, about 90 minutes)

1. **Readers.** Every member must read 0x15.7 and 0x15.8 before either is enabled. Main 46ea4200 alone lists no 0x15.7 in `Actions::reads()` (F121's reader miss). So this day must be built from a main that holds `oidc-claims`:
   - If AM2's kill-row site day is built from such a main, it is the readers day.
   - Otherwise BF-73's `site` step runs one: no `CLAIMS`, and `HOSTV` set to the workers.dev host.
2. **BF-74 `state`, then `domain`.** `domain` creates the custom domain; workers.dev keeps serving until the edge day.
3. **Edge day** (`ADDFMT=21.7,21.8 bash tests/m4/wp3/bf74/row.sh r1 edge`): `HOST=m4a.graftyard.dev`, and 21.7 and 21.8 added to the live `FORMATS_ENABLED`. Nothing else changes (F124's enable on a test install).
4. **Site day** (`CLAIMS=1 ... site`): `GRAFTYARD_HOST=m4a.graftyard.dev`, `GRAFTYARD_OIDC_HEAD_REF=1` and `GRAFTYARD_OIDC_CLAIMS=1`, with BF-71's flags kept.
5. **BF-74:** `before`, `rule`, `after`, `token`, `rp`.
6. **BF-73 on the custom host** (`GRAFT_EDGE=https://m4a.graftyard.dev bash tests/m4/wp3/bf73/row.sh r1 pr`, then `dispatch`, then `custom`). Its relying party uses urllib's default agent here, so its three runs also exercise BF-74's rule. On a workers.dev issuer it would have to send its own agent (F130).
7. **`back`:** a site day and an edge day with the workers.dev host. Both keep the flags and the formats.
8. **Teardown:**
   - both repositories are deleted;
   - the token file is shredded;
   - the custom domain is deleted (`KEEP=0`, the default).
   The configuration rule stays in graftyard.dev, matching only `m4a.graftyard.dev`, and is recorded in `install-m4a.md`. Deleting it is one call if the lead prefers.

The row takes three site days and two edge days in all: the readers day (unless AM2's kill row carries it), the flags-and-host day and `back`, plus the host-and-formats edge day and `back`. Run separately, the two rows would need two more of each.

The discovery document's `issuer` comes from the edge's `HOST` (`workers/edge/src/actions/oidc.rs`), and a token's `iss` from the sites' `GRAFTYARD_HOST`. Between steps 3 and 4 they disagree, so no token is minted until both days are done. `rp.py` fails a token whose `iss` is not discovery's `issuer`.

**Risk.** F130 measured BIC's 1010 on workers.dev. On graftyard.dev, a Pro zone with BIC on and security level medium, the `before` check is expected to read the same 403 1010 [inference]. If it reads 200, the row fails its first line ("a check that passes before the rule"). That would be a finding about F130, not a reason to change the rule.

**Cost [guess].** About $1 to $1.5 for the window. BF-70 r2 spent $0.69 on four jobs and its row; this window adds two days and five jobs. Each row keeps its own $3 stop.

**Needs:** `TREE` (a worktree at the row's main) with `worker-build --release --features test-faults` done in `workers/site`, logged in `$HOME/bf71/site-build.log`. Also `/tmp/bf71web`, `RUNNER` (actions/runner 2.338.2), `target/release/examples/jitconfig`, and `G` (the graftyard binary). The install token already holds Workers Routes, DNS, SSL and Certificates, and Config Settings write on graftyard.dev. graftyard.dev is a Pro zone in the install account, with Browser Integrity Check on, no custom domains and no `http_config_settings` rules. Its universal certificate covers `*.graftyard.dev`, which is why the host sits one level under the zone.

## Why m4a goes back to the workers.dev host

The edge routes the API only for its `HOST`'s authority (`graftyard_wire::Host::endpoint`). On the custom host, workers.dev serves nothing but `/_services/`. These still name the workers.dev host:

- **Site days that set `GRAFTYARD_HOST` to it:**
  - `tests/am2/hosted/r47-day.sh`
  - `tests/m4/wp1/bf58-kill.sh`
  - `tests/m4/wp2/hosted/siteday.sh`
  - `tests/m4/wp3/bf55-kill.sh`
  - `tests/m4/wp3/bf71/days.sh`
  - `tests/m4/wp3/oidc/days.sh`
  - `tests/m4/wp4/siteday.sh`
  - `tests/m5/b07r2/row.sh`
- **Edge days.** `tests/m1/integ/deploy-edge.mjs` defaults `HOST` to `<name>.<sub>.workers.dev`, and none of these pass `HOST`, so the next one silently moves the edge back while the sites stay on the custom host:
  - `tests/am2/hosted/wp3-day.sh`
  - `tests/am3/hosted/lf7-day.sh`
  - `tests/m5/bf41.sh`
  - `tests/m4/wp3/bf71/days.sh`
  - `tests/m4/wp3/oidc/days.sh`
- **`tests/am1/lf13/env-m4a.sh`** sets `GRAFT_EDGE` to the workers.dev URL, and 49 drivers source it.
- **The runner account's `graft-runners`** has `GRAFTYARD_URL` set to the workers.dev URL.
- **Records:** `tests/am1/results/install-m4a.md`, `tests/m4/results/m4-wp3-oidc.md`, `m4-wp3.md` and `m4-wp4.md`, and `decisions/OPEN-QUESTIONS.md`.

Leaving m4a on the custom host would mean changing every driver above to take the host as an input, plus a `graft-runners` deploy in the runner account. So the plan sets it back.

## Checks at $0

- `tests/m4/wp3/bf74/rp_test.py` drives `rp.py` against a local issuer through 11 cases, and plant250 catches 7 of 7 of its mutants.
- plant249 covers the claims code: numbering, actor, `sub` template, job message, trigger, and format reads.
- `claims.mjs`, `token.mjs` and both row scripts have only been syntax-checked (`node --check`, `bash -n`). They first run hosted.
