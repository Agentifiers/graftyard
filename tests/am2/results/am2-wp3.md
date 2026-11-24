# AM2-WP3: `graft-mcp` release wiring, code only

Branch `am2-wp3`, from main `6c869987`. Code only, nothing hosted. Cleared by the lead on 2026-11-23 as scoped: the
part of AM2-WP3 (`build/BUILD-PLAN.md`) main lacked. `McpRoll`, `McpPointer`, `Pointers.mcp`, `next_roll`'s order and
undo, and `graft-ops`'s `mcp` script were already in `crates/release` and `workers/ops` (M5). `graft-mcp` already
forwarded the override header it got on every edge call.

## What was built

- **`graftyard_release::view::forward`**, `graft-front`'s plan for one request, pure so every rule is tested:
  - **Route** (`front_route`). The host `MCP_HOST` names (`mcp.<host>`) goes to `graft-mcp`, compared as a DNS name
    (no case, port, or trailing dot). Every other host goes to the edge, so does every host when the front has no
    `graft-mcp`.
  - **Pin.** The answer is checked against the pointer of the script it went to: `Pointers.mcp` for an MCP request,
    `Pointers.edge` for any other.
  - **Header** (`overrides`). One `Cloudflare-Workers-Version-Overrides` value names every pinned script, as a
    Structured Fields dictionary (`graft-edge="…", graft-mcp="…"`), and a `None` pointer adds nothing, so the
    deployment's split applies [doc, https://developers.cloudflare.com/workers/configuration/versions-and-deployments/version-overrides/:
    the header is a dictionary keyed by Worker name, and it carries across `fetch()` service bindings]. The same
    header goes to both scripts: `graft-mcp` forwards it to the edge, so an edge undo pins `graft-mcp`'s edge calls.
    A front with no `graft-mcp` never reads `Pointers.mcp`.
- **`workers/front`.** The `MCP` `fetch()` binding, used only when both `MCP_HOST` and `MCP_SCRIPT` are set; the plan
  above; the mismatch alarm per script (`ALARM graft-front: graft-mcp pinned to … answered from …`); `mcp` and
  `mcpPin` in `/_front/status`. The version report is taken off the client's response as before, unless the client
  asked for it.
- **`workers/mcp`.** A `VERSION` version metadata binding, and `x-graftyard-mcp-version` on each answer to a request
  that carries `x-graftyard-front`.
- **Deploy scripts.**
  - `workers/front/deploy.mjs` takes `--mcp <script> --mcp-host <host>`, which go together, and `--domain <host>`
    (repeated) for Workers Custom Domains (`routes` with `custom_domain`). Domains are operator setup outside any
    request path.
  - `workers/mcp/deploy.mjs` binds `VERSION` and takes `--upload-only` (`wrangler versions upload`), for an `McpRoll`.

## Checks

- `crates/release/src/tests/group.rs`: `one_override_header_carries_every_pinned_script_and_none_for_an_unpinned_one`,
  `graft_front_routes_only_the_mcp_host_to_graft_mcp`, and
  `graft_front_checks_each_request_against_its_own_scripts_pin_and_sends_both_pins`. `graftyard-release`'s tests
  pass.
- `tests/m0carried/plant102.py`: **17 of 17 caught**. The plants cover a header that keeps one pin, empty or
  separator-less members, hosts that end or start with the MCP host, case, port, an empty MCP host, everything to
  `graft-mcp`, the wrong pin checked either way, a header missing either pin, a front without `graft-mcp` reading
  `Pointers.mcp`, the MCP pin keyed on the edge's name, and a missing version report not alarming.
- The full checks and both edgeboot builds: see the commit message.

## Can `FormatsEnable` be turned off? (the lead's question before graft-m4a's edge day)

**Not in the `Release` group: no.** `ReleaseEntry` has `FormatsEnable` and no disable entry. `ReleaseFold::enabled` only
grows (`crates/release/src/group.rs`: `check` refuses an already enabled format, and nothing removes one). This is the
two-release rule's step 4: once a format is enabled, swap-back below the reading release is refused and pages
(`design/SHARED.md`, "Record formats and the two-release rule"; BF-7 measured it on graft-m4a with 65000.1, which
stays enabled for good, `tests/am1/results/install-m4a.md`).

**The edge's `FORMATS_ENABLED` variable is a different switch, and it can be turned off.** It is a script variable
(`workers/edge/src/lib.rs`, `Config::enabled`), read per isolate and set at each edge day. It decides which formats
this edge writes; members still read every format their release knows. An edge day without 20.4 and 17.4 makes the
edge stop writing them. Rows it already wrote stay readable, because the sites' release reads them. So adding 20.4
and 17.4 to graft-m4a's `FORMATS_ENABLED` for the row is reversible by the next edge day. A `FormatsEnable` entry for
them would not be, and the WP3 day commits none.

## Hosted, day d1 on graft-m4a (2026-11-24)

The lead's go from main `c6e4113b` or later, in the slot M4-WP4 gave (its "graft-ops free" at 03:11Z, after BF-46,
BF-47 and its check row). Drivers `tests/am2/hosted/wp3-day.sh` from branch `am2-wp3-day` on main `4246e550`; graft-ops
from main `a2c1b11b` (ops code unchanged since R9's `05aace32`). Raw logs in `tests/am2/raw/wp3-d1/`, one per step.
Start 03:21:44Z; done line **04:37:13Z** (end sweep and state read). $5 row stop.

### Verdicts

| # | Check | Verdict | Evidence |
|---|---|---|---|
| 1 | Start state from BF-47 and M4-WP4 | pass | `state.log`: count 288, current none, drained `[]`, site_release a, b `e23f69e9` (R9), c `1a0f0e50` (R10); rolled Edge, Ops on R9, no Front or Mcp; ops `ac4cd9a5`, schedules `[]`. So w1 is each script's first roll and no undo exists yet for either, as `next_roll` says. |
| 2 | Exactly two custom domains; zone ends as it started | pass | `domains.log` (before) and `domains-2.log` (after) are identical: 0 Workers custom domains, 0 DNS records, Spectrum `ssh.graftyard.dev`, the two universal packs. During the day: `m4a.graftyard.dev` and `mcp.m4a.graftyard.dev`, both on `graft-m4a-front`; two Advanced packs issued (`9da05bf5`, `3069976d`, active by 03:37Z), and deleting the domains removed them (`certs.log`). |
| 3 | `FrontRoll` executed by graft-ops | pass | w1 (`762a96d1…`): intent at 03:57:45Z, edge index 1012, 0 undos (`roll.log`); front `2d211afd` 1%, then 100%, IntentDone 03:59Z. w2 front `5ee2eb15` (IntentDone 04:13Z), w3 front `57581ba4` (04:27Z). |
| 4 | `McpRoll` executed by graft-ops | pass | w1 McpRoll, 0 undos (no N): `e7c66cdb` 1% at 04:03Z, 100% at 04:05Z (`opstail.jsonl`). w2 McpRoll carried **1 undo** (`McpPointer` to w1). |
| 5 | Pre-signed `McpPointer` committed on a gate breach with the deploy token invalid | pass | w2 `c70db1f4` at 1% at 04:15Z. Token PATCHed to an invalid value at 04:16:00Z (probe 400, `block.log`); graft-ops `5421ad57` with `GATE=breach:wp3`; its 04:18:00 run committed `McpPointer` (entry 304, the pre-signed undo, signature by the customer key `031923…`) without reading the deployment (it logged "no deployment of Mcp read"). After unblock and gate pass (`940bafa4`), the undo's deploy put `e7c66cdb` 100%, `c70db1f4` 0% (04:21Z). |
| 6 | graft-front holds requests to N while the pin holds | pass | Sampler through `mcp.m4a.graftyard.dev`, 202,847 requests, 0 non-200, 0 alarms (`mcp.jsonl.gz`). First pinned answer 04:18:02.5Z, last unpinned 04:18:05.4Z (isolates' view refresh). From then to 04:28:20.8Z, **119,218 of 119,218** carried pin `e7c66cdb` and were answered by `e7c66cdb`, while the deployment still split 1% to `c70db1f4` until 04:21Z. `/_front/status` at 04:20Z: 0 mismatches, `mcpPin` `e7c66cdb`. |
| 7 | MCP session at N through `mcp.<host>` | pass | `session-4.log` (after w1): initialize (protocol 2026-07-28), tools/list 8 tools, `list_issues` ok, version `e7c66cdb`. Pre-roll sanity `session-3.log` on the apps-step version `044e4429`: same three answered. |
| 8 | MCP session while the pin holds | pass | `session-5.log` at 04:19Z: initialize, tools/list 8, `list_issues` ok, every answer from `e7c66cdb` (the pin) while the split was `c70db1f4` 1%. |
| 9 | The next roll clears the pin | pass | w3's intent (04:28:18Z) cleared `Pointers.mcp`: unpinned answers from 04:28:20.8Z; w3 `016538f0` at 1% from 04:29:06Z, 100% at 04:31Z with `e7c66cdb` at 0%. Status at 04:32Z: `mcpPin` null, 0 mismatches. |
| 10 | Teardown | pass | `teardown.log`: ops cron reset (yearly, then `[]`); graft-ops back to **`ac4cd9a5` by version id** (the lead's ruling), whose own bindings read `GATE=pending`, three site scripts, 50 entries, 60 s dwells (`ops-parked.log`), and no ops run after 04:36:00Z. `graft-m4a-front`, `graft-m4a-mcp`, `graft-m4a-mcp-aux` deleted (with both domains). The four apps-init secrets deleted. Row token `graft-m4a-wp3-deploy` (`23494962…`) revoked, its files shredded. Edge back to **`eef95142` 100%, `f4e193b9` 0% by version id** (`edge-3.log`), whose own `FORMATS_ENABLED` is the handoff's 26 without 20.4 and 17.4, `HOST` the workers.dev name: **20.4 and 17.4 are off on the edge again**. End sweep: no shard running. |
| 11 | Spend | pass | $0.05 on the graft-m4a prefix (`cost.log`, 03:21:44Z to 04:39:54Z), under the $5 stop. |

**AM2-WP3 passes hosted, 11 of 11.**

### What the day found (harness, not product)

- **The edge's `HOST` must be the front's hostname.** `deploy-edge.mjs` sets `HOST` to the edge's workers.dev name,
  so the first edge day (`c8539208`) answered every REST and OAuth path through `m4a.graftyard.dev` with 404
  "Repository not found." (`Host::endpoint` misses the authority), and the first session failed at the host's
  authorize. The second edge day (`833bb489`, `edge-2.log`) set `HOST=m4a.graftyard.dev`, and the session driver
  now takes `GRAFT_EDGE` from it. This is the product's rule (`HOST` is the install's hostname); a production
  install's edge has one.
- **The aux Worker** did not deploy from a `<(...)` config, since its `main` is relative; the driver now writes it
  beside `aux.jsonc` (`apps-aux.log`).
- **The first w2 roll hung** for four minutes in `release roll`'s POST without committing (the group stayed at 299);
  stopped and rerun, it committed at once (`roll-4.log`). Not seen again.
- **`/settings` shows the newest uploaded version, not the deployed one**, so after a by-id restore it still lists
  the later version's vars. The verdicts above read each deployed version's own bindings.

## Not done here

- **Hosted.** Done: see "Hosted, day d1" above. The plan as ruled, on graft-m4a, 2026-11-24 UTC, after B0-02's BF-46 and M4-WP4's
  check row, in a slot agreed with M4-WP4:
  - an edge day with FORMATS plus 20.4 and 17.4, and `apps init` (4 store secrets, deleted at teardown);
  - a row-scoped deploy token, minted and revoked;
  - two Workers Custom Domains under `graftyard.dev`, `<install>.graftyard.dev` and `mcp.<install>.graftyard.dev`,
    with the zone's custom domains listed before and after;
  - a `FrontRoll`, then an `McpRoll` at 1% with the gate breached and the deploy token invalid, until `graft-ops`
    commits the pre-signed `McpPointer`;
  - one MCP client session through `mcp.<host>` at N and one while the pin holds, each showing `initialize`,
    `tools/list`, and one tool call answered;
  - `graft-ops` back at its parked version (`GATE=pending`, no cron) at handback. $5 row stop.
