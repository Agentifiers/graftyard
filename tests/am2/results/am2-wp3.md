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

## Not done here

- **Hosted.** It needs the lead's go after this merges. On graft-m4a, 2026-11-24 UTC, after B0-02's BF-46 and M4-WP4's
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
