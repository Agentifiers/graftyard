# M0-WP15 working state (resume from here)

Paths: host copy `src/wp15/host/`, graft-mcp `src/wp15/mcp.ts`, drivers `run/wp15*.ts`, configs `wp15/*.jsonc`.
Out-of-repo: venv `~/.cache/wp15-venv` (mcp 2.3.0), `~/.cache/wp15/github-mcp-server` (v2.0.3 build), go-sdk v1.8.0 in /tmp/gosdk.
Scripts: `graft-wp15-host` (layer on), `graft-wp15-base` (LAYER=off), `graft-wp15-mcp`, `graft-wp15-aux` (CIMD docs + webhook sink).

- [x] copy host, venv out of repo, check_links clean
- [x] host: oauth.ts (login/oauth, ghu_/ghr_, DELETE token/grant/installation token, mcp/grants), LAYER flag
- [ ] deploy configs + run/wp15.ts deploy/setup
- [ ] graft-mcp (src/wp15/mcp.ts)
- [ ] aux worker (CIMD + hooks)
- [x] L0-08 clients and negatives (written to results)
- [ ] L0-11 GHES subset, recorder, diff
- [ ] L0-07 manifest, JWT, webhooks, revoke
- [ ] results doc, edits, checks, teardown

Progress 10:30Z: deployed aux + host (base, mcp in deploy service `wp15-deploy`, log results/raw/wp15-deploy.log).
Clients written: wp15/ts-client.ts (v2|v1), wp15/py_client.py, wp15/goclient (binary ~/.cache/wp15/goclient), run/wp15-browser.ts.
Next: run/wp15.ts setup; seed issue #1 on acme/mono (alice PAT); smoke ts v2.
- [x] L0-08 clients run, raw results/raw/wp15-l008-clients.jsonl
10:55Z: gql.ts (GHES 3.22 schema executor, schema at .gen/ghes-3.22.graphql), ghes.ts REST subset; deploying host+base.
L0-11 plan: run/wp15-tee.ts = TLS proxy on https://ghes.wp15.test:8443 (/etc/hosts, cert ~/.cache/wp15/tls) that sends each
request to host (layer on) then base (layer off), returns host's, records both to results/raw/wp15-l011.jsonl. Known diff
candidates inherited from WP10/13: `graftyard_gate` on review responses, `graftyard_task` on access_tokens responses.
11:05Z L0-11 full run in service wp15-l011 (log results/raw/wp15-l011-run.log, summary results/raw/wp15-l011-summary.json). Stop it after DONE.
11:20Z L0-11 written to results. Next: L0-07.
11:30Z L0-07 plan: new src/wp15/host/apps.ts (manifest flow /settings/apps/new + conversions, manual create + key/secret,
  RS256 App JWT verify, install via /github-apps/{slug}/installations/new, /app, /app/installations, repo installation,
  check-runs list, App webhooks from committed() signed with derive(secret, webhook:<slug>) to aux /hooks/<slug>,
  graftyard/overlap check run from PathSets when insights installed). Drivers: wp15/l007-vendor.ts claude|cursor|codex,
  plus github-mcp-server on an installation token. Then 100 pushes 1 s after revoke.
L0-07 design: apps.ts state `ghApps` (kv "ghApps"), App ids from 20, installations from 200. Manifest apps get
  webhook_secret = derive(M0_TOKEN, webhook:<slug>) so aux /hooks/<slug> verifies. Overlap check from PathSet events
  (layer on, insights installed). Vendor branch prefixes claude/, cursor/, codex/, ghmcp/ (pkg/auth is leased; use docs/shared.md).
  Codex doc perms have no Checks: expect 403 on check write and read (finding).
11:50Z apps.ts written + wired into host.ts (tsc clean). Deploying host+base (service wp15-deploy, log results/raw/wp15-deploy.log).
12:05Z L0-07 full run in service wp15-l007 (log results/raw/wp15-l007-run.log; per vendor results/raw/wp15-l007-<v>.json). Stop after DONE.
12:15Z L0-07 run done; claude/cursor/ghmcp all ok, codex fails checks (no Checks perm in OpenAI doc). Next: write L0-07 + edits.
12:25Z L0-07 written to results. Next: proposed edits, teardown, checks.
12:35Z proposed edits written. Next: teardown section, status line, checks.
12:45Z results complete; services stopped; deployments left up. Only final message left.
