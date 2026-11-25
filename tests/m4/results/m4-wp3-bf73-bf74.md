# BF-73 and BF-74 on graft-m4a

Status (2026-11-25 17:34Z): **r1 passes every line of both rows.** The lead gave GO on main `23de37af`. The window ran from 16:56Z (`state`) to 17:34Z (teardown, the done line) and carried its own readers day. Both rows together spent $0.09 against $3 stops: graft-m4a's UTC day went from $7.54 to $7.67 by `acctcost.py` (prefix graft-m4a), and the rows' cost since `cost.from` was $0.09. Raw logs are in `tests/m4/raw/wp3/bf74/r1/` and `tests/m4/raw/wp3/bf73/r1/`.

## r1: verdict

### BF-74

| Line | Result |
| --- | --- |
| The install served on a custom hostname in graftyard.dev, with its `HOST` on it | **Met.** Workers Custom Domain `m4a.graftyard.dev` → `graft-m4a-edge` (17:08Z). Edge day `73e9a637` with `HOST=m4a.graftyard.dev`; site day a `58e814b5`, b `60fe0cd8`, c `3e293af0` with `GRAFTYARD_HOST=m4a.graftyard.dev`. Discovery's `issuer` read `https://m4a.graftyard.dev/_services/token` |
| The first check reads 403 1010 | **Met** (17:22:53Z). `graftyard oidc check --edge https://m4a.graftyard.dev`: `HTTP 403` as `Python-urllib/3.12`, exit 1. The body is Cloudflare's 1010 page: `error code: 1010` as text when urllib sends no `Accept`, and the HTML form of the same page when ureq sends `Accept: */*` |
| The rule writes one configuration rule and reads it back | **Met** (17:23:22Z). Before: no `http_config_settings` entrypoint (10003). `bic-rule` printed `Created`, then `Kept` on the second run (no write). After: one rule, `graftyard-oidc-bic`, expression `(http.host eq "m4a.graftyard.dev" and starts_with(http.request.uri.path, "/_services/token/"))`, `bic: false`, and nothing else in the zone |
| The second check reads 200 | **Met** (17:23:42Z). `JWKS as Python-urllib/3.12: 200, 2 keys`, exit 0 |
| The token's `iss` is `https://m4a.graftyard.dev/_services/token`, and the Python verification passes | **Met** (17:25Z). A `workflow_dispatch` job in `m3org/bf74-r1` (run on an unmodified runner 2.338.2) wrote one token for `aud` `bf74`. `rp.py` discovered the keys from its `iss`, with both fetches 200 as `Python-urllib/3.14`, verified RS256 under kid `g-7DvKNw…` (slot B), and accepted it (`rp exit 0`) |

### BF-73

Each token was minted on the custom host. `rp.py` verified it with urllib's defaults and accepted it under the policy built from the install's records: `GET /user`, the repository's run list, and the pull request's branch. It refused the same verified token under a policy naming another actor and one naming another branch.

| Run | `sub` | `actor` / `actor_id` | `run_number` | `head_ref` / `base_ref` | `job_workflow_ref` |
| --- | --- | --- | --- | --- | --- |
| `pull_request`, PR #1 from `bf-pr-mwwearo4` (run 2031978, 17:26Z) | `repo:m3org/bf73-r1:pull_request` | `m3-admin` / `7100003` | 1 | `bf-pr-mwwearo4` / `main` | `m3org/bf73-r1/.github/workflows/claims.yml@refs/pull/1/merge` |
| `workflow_dispatch` on `main` (run 2031981, 17:28Z) | `repo:m3org/bf73-r1:ref:refs/heads/main` | `m3-admin` / `7100003` | 2 | empty / empty | `...claims.yml@refs/heads/main` |
| after `PUT .../customization/sub` (run 2031984, 17:29Z) | `repo:m3org/bf73-r1:ref:refs/heads/main:actor:m3-admin:run_number:3` | `m3-admin` / `7100003` | 3 | empty / empty | `...claims.yml@refs/heads/main` |

| Line | Result |
| --- | --- |
| Every token is accepted by the policy that names its run's values | **Met**, 3 of 3 (`policy_accepts`, with `mismatch` `{}`) |
| Every token is refused by one that names another actor or branch | **Met**, 3 of 3 for each (`other_actor_refused`, `other_branch_refused`: verified, refused on that one claim) |
| `run_number` counts the workflow's runs | **Met**: 1, 2, 3, equal to each run's place in the run list, and `github.run_number` printed the same in each job |
| The third token's `sub` follows the customization | **Met**: `PUT` 201, `GET` read back `use_default` false and the four keys, and the third `sub` is the template's |

`github.actor` printed `m3-admin` in all three jobs, so F120's job-message half also holds.

## r1: what ran

| Step (Cloudflare time) | Result |
| --- | --- |
| `state`, 16:56Z | Sites a `2f8a1178`, b `421db7d6`, c `b9960b91`; edge `3c453149`; ops `ac4cd9a5`, schedules `[]`. graftyard.dev had no custom domains, no DNS records, no config rules, and two universal packs |
| `domain`, 17:08Z | Created. `/_services/token/.well-known/jwks` answered 200 on the custom host within 10 s, and `/status` answered 404 until the edge day (the API routes only on `HOST`). The first two tries waited on `/status` through the orb's proxy, which answers CONNECT to the new name with 502, so they were stopped with nothing written. Both row scripts now unset the proxy, as BF-70's runner steps did |
| Readers day, 17:09Z to 17:10Z | Main `23de37af`, test-faults, BF-71's flags, workers.dev host: a `b5c08fed`, b `f6029797`, c `88d85c1b`. An edge-day build started first by mistake was stopped before upload (the edge stayed `3c453149`) |
| Edge day, 17:11Z to 17:14Z | `73e9a637`: only `HOST` and `FORMATS_ENABLED` (+21.7, +21.8) changed; edgever exit 0 |
| Site day, 17:16Z to 17:17Z | `CLAIMS=1`: `GRAFTYARD_OIDC_HEAD_REF=1`, `GRAFTYARD_OIDC_CLAIMS=1`, host `m4a.graftyard.dev`; crons `[]`. Reretire then read `RERETIRE OK bf74-r1` |
| BF-74 `before`, `rule`, `after`, `token`, `rp`, 17:22Z to 17:25Z | As in the verdict |
| BF-73 `pr`, `dispatch`, `custom`, 17:26Z to 17:29Z | As in the verdict |
| `back`, 17:29Z to 17:32Z | Site day a `304052f8`, b `d802c6e4`, c `655e6de9` (flags kept, workers.dev host). Its edge half first stopped on an unbound `ADDFMT` before deploying, now fixed. Rerun as `r1/back edge`: `d4173e47`, only `HOST` changed back, 21.7 and 21.8 kept. **Sites and edge named different hosts from 17:30:25Z to 17:32:17Z**, during which a minted token's `iss` would not match discovery's `issuer` |
| Teardown, 17:33Z to 17:34Z | `m3org/bf73-r1` and `m3org/bf74-r1` deleted (204; an anonymous read then answered 404). The rule deleted, and graftyard.dev's `http_config_settings` read back `[]`. The custom domain deleted: graftyard.dev's custom domains read `[]`, and its Advanced certificate pack is `pending_deletion`. The token file shredded. No deploy token was minted or used: no row step deployed `graft-m4a-ops` |

**Left on graft-m4a:**
- sites a `304052f8`, b `d802c6e4`, c `655e6de9` (main `23de37af`, BF-71's flags plus `GRAFTYARD_OIDC_HEAD_REF=1` and `GRAFTYARD_OIDC_CLAIMS=1`, the workers.dev host);
- edge `d4173e47` at 100% (`FORMATS_ENABLED` now holds 21.7 and 21.8);
- `graft-m4a-ops` `ac4cd9a5`, every schedule `[]`.

`ActionsOidcKey` is still on slot B.

## The plan as approved
### What each row proves

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

### Run order (one m4a window, about 90 minutes)

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

### Why m4a goes back to the workers.dev host

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

### Checks at $0

- `tests/m4/wp3/bf74/rp_test.py` drives `rp.py` against a local issuer through 11 cases, and plant250 catches 7 of 7 of its mutants.
- plant249 covers the claims code: numbering, actor, `sub` template, job message, trigger, and format reads.
- `claims.mjs`, `token.mjs` and both row scripts were only syntax-checked before r1. r1 ran them, and found three driver bugs, each fixed in the branch with r1's results: the orb proxy (above), `back`'s unbound `ADDFMT`, and two log filters that read the runner's echo of a script line instead of its output. The `custom` run's `context_line` passed, but by reading the echo, which shows the evaluated expressions; the output line also says `run_number=3`.
