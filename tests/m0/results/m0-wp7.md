# M0-WP7: SSH chain (2026-10-30 to 2026-10-31)

Row B0-24 (`build/BUILD-PLAN.md`, M0-WP7). The zone is `graftyard.dev` (id `<cf-zone-id>`) on Pro, paid by the operator, in the Milestone 0 account. The zone was activated at 23:00 UTC on 2026-10-30, and every Spectrum call below came after that. Business and Enterprise zones are not in the account, so the row is **not run** on them, as M0-WP6 records for its rows.

## Verdict

| Row | Plan | Result | Gate | Verdict |
| --- | --- | --- | --- | --- |
| B0-24 | Pro | The Spectrum API refuses the design's app. `virtual_network_id` is rejected as an unknown field (10012), and an SSH app on port 22 with no extra fields is refused with "The requested protocol is not available" (13002) | Flip: "If Spectrum cannot reach a Tunnel origin on Pro or Business, SSH needs Enterprise for BYOC" | **Fail on Pro: the flip condition holds.** SSH needs Enterprise for BYOC |
| B0-24, Tunnel and containers only | Pro zone, no Spectrum | 2,634 of 2,637 SSH git sessions succeeded (99.886%) over 2.2 h, with 13 kills | At least 99.9% pass, under 99% fail | **Between the lines**: one session short of the 99.9% pass line (at most 2 of 2,637 may fail; 3 did) and well above the 99% fail line. Only 1 failure coincided with a kill. This half of the chain is not the row as written, because no client can reach it on port 22 without Spectrum |
| B0-24 | Business, Enterprise | Not run: no such zone in the account | | Not run |

## Step 1: Spectrum on Pro

**Setup, built first in the design's shape.** Tunnel `graft-m0-ssh` (`6dc68d9d-ab65-4767-9391-418b6bf46c0b`, `config_src: cloudflare`, `warp-routing` on), virtual network `graft-m0-ssh-vnet` (`ab484908-ad4a-4d2b-a301-01992163411a`), and route `10.222.0.22/32` to the tunnel in that network. A container can hold that address. `ip addr add 10.222.0.22/32 dev lo` succeeded inside a running front (its effective capabilities were `abac35fb`), and it was removed again because the app could not be created.

**What the API said.** Every request was `POST /zones/<cf-zone-id>/spectrum/apps` with the account's API token. The raw request and response bodies are in `results/raw/wp7/spectrum-*.json`.

| Body | Response |
| --- | --- |
| The design's shape: `protocol: "tcp/22"`, `dns: {CNAME, ssh.graftyard.dev}`, `origin_direct: ["tcp://10.222.0.22:22"]`, `virtual_network_id`, `proxy_protocol: "off"`, `traffic_type: "direct"` | `10012 json: unknown field "virtual_network_id"` |
| The same body with only `virtual_network_id` added to the three base fields | `10012 json: unknown field "virtual_network_id"` |
| `proxy_protocol: "off"` added to the three base fields | `10012 json: unknown field "proxy_protocol"` |
| `protocol`, `dns`, `origin_direct` to the private IP, the pay-as-you-go minimum | `13002 The requested protocol is not available.` |
| The same with a documentation-range public origin `tcp://192.0.2.1:22` (never created) | `13002` |
| `tcp/2222` to the private IP, and `tcp/25565` (Minecraft) | `13002` |

The minimal SSH app was retried 16 minutes after zone activation and again at 00:01 UTC, more than an hour after activation, and it still got 13002. No app was created: `GET .../spectrum/apps` returns `[]` before and after.

**What the docs say about these codes** (https://developers.cloudflare.com/spectrum/reference/error-codes/, read 2026-10-30):
- 10012: "Pay-as-you-go accounts are limited to `protocol`, `dns`, and `origin_direct` when creating or updating a Spectrum application. A common cause is an account without a Spectrum entitlement sending a full Spectrum configuration — any field outside those three will be rejected." Resolution: "contact your account team to enable Spectrum as a paid add-on."
- 13002: "The requested protocol is not available for the zone."

**Reading.** Two separate facts settle the flip on Pro.
1. **Pro cannot reach a Tunnel origin [doc and measured].** `virtual_network_id` is not one of the three fields a pay-as-you-go account may send, and the API refused it on this Pro zone. Even with the add-on enabled, a virtual-network origin needs "Application type must be TCP or UDP" (https://developers.cloudflare.com/spectrum/reference/configuration-options/, "Virtual network origin"), and "Protocols per plan" lists TCP as "No" on Pro and Business and "Paid add-on" only on Enterprise. Pointing the app at `<UUID>.cfargotunnel.com` through `origin_dns` is not allowed either, because `origin_dns` needs Enterprise and the limitations page says that path does not work. So no field a Pro zone may send connects Spectrum to a Tunnel.
2. **Even a plain SSH app is refused on this zone [measured].** 13002 came back for `tcp/22` with only the three allowed fields, and for `tcp/25565` too. The zone's entitlements (`results/raw/wp7/zone-entitlements.json`, 124 entries) contain no Spectrum entitlement except `logpush.spectrum_events.max_jobs_allowed = 0`. The subscription carries a usage component `spectrum_bytes_transferred` (rate plan `pro`: `unit_price 1`, `default 0`). That is a meter for bytes, not an entitlement. What turns the app on is not public. It may be the dashboard's first-use "Create an Application" modal (https://developers.cloudflare.com/spectrum/get-started/), or an add-on purchase. Taking either step needs the operator, and neither would change fact 1. Nothing was bought and no dashboard flow was started.

Business is very likely the same, because the docs give it the same "Paid add-on" availability and the same "TCP: No" [inference, not run].

## Step 2: the Tunnel half, measured without Spectrum

Spectrum is out of the path, so the client reaches the Tunnel the only other way Cloudflare offers on this plan: `cloudflared access ssh` as the SSH `ProxyCommand`, to `ssh-m0.graftyard.dev`, a proxied CNAME to `<tunnel>.cfargotunnel.com`, whose ingress rule sends `ssh://localhost:22`. Every client then needs cloudflared installed, which a GitHub-compatible `git clone git@host:...` user does not have. That is why this half cannot stand in for the row.

### What was built (`graft-m0-ssh`)

- **SSH front containers.** `SshFront` (`src/wp7/ssh.ts`) is a container class under the `durable_object` policy, one `basic` instance per object. `front-a`, `front-b`, and `front-c` were created with the enam, weur, and apac hints, and they landed in `ord06` (ENAM), `cdg14` (WEUR), and `hkg12` (APAC). Each container (`wp7/front/Dockerfile`, Alpine 3.22) runs OpenSSH 10 on loopback as the stand-in for `graft-sshd`, `git-shell` for upload-pack and receive-pack on one seeded repository (40 files, identical object ids on every replica), and one cloudflared 2026.10.7 replica of the same tunnel with `--protocol http2`. Each replica holds 3 or 4 edge connections in its own region's colos (ord, cdg, hkg).
- **Outbound handler.** `SshOutbound`, a `WorkerEntrypoint` of the same script, is registered with `interceptOutboundHttp` for three virtual hosts. It runs in the Workers runtime and reaches the stand-in groups through the script's own Durable Object binding:
  - `keys.graft` hands out the service host key (secret `SSH_HOST_KEY`) and the tunnel token (secret `TUNNEL_TOKEN`). Neither is baked into the image, and every replica serves the same Ed25519 fingerprint `SHA256:GCDfbpG3KG4ODrXC4LI4nA1yUYacmDBWuzGJA1xH4u4`, checked by the client with `StrictHostKeyChecking=yes`.
  - `tokens.graft` is sshd's `AuthorizedKeysCommand`, which resolves the key fingerprint on the `tokens` object.
  - `group.graft` is the pre-receive hook. It resolves the key again, then commits the ref updates to the `group` object's log and answers `seq=N`. git accepts the push only after that 200.
- **Supervisor.** Each front object's alarm runs every 2 s and restarts a container that is not running. That is the restart the design needs, because Cloudflare "does not guarantee" to keep containers running.

### Load

`run/wp7.ts load --run b0-24 --hours 2.2`, run as the supervised orb service `wp7-load` (resumable from `state.json`, with sessions appended to `results/raw/wp7/b0-24/sessions.jsonl`). Two workers in the orb (LAX) each run one SSH git session every 6 s, rotating clone, fetch, and push, for about 20 sessions a minute. Each push writes a new branch `wp7/b0-24/<n>`. A session counts as successful only if git exits 0 and the serving front named itself on stderr, and for a push only if the hook also reported the group's `seq`. Every 10 minutes the runner kills the replica that served most recently (`container.destroy()`). The supervisor restarts it, and the runner records when the restart happened and when the new cloudflared reported a ready connection, on Cloudflare's clock.

### Results (run `b0-24`, 2026-10-30 23:58:59 to 2026-10-31 02:10:49 UTC, 2.2 h)

| Measure | Value |
| --- | --- |
| Sessions | 2,637: 879 clones, 880 fetches, 878 pushes |
| Succeeded | 2,634 (**99.886%**) |
| Failed | 3, all "connection dropped mid-session": the SSH stream through `cloudflared access ssh` closed before git got a reply (`Connection closed by UNKNOWN port 65535` twice, `closed by remote host` once) |
| Failures that coincided with a kill | 1: fetch `n=1000` began 0.1 s before kill 5 on the replica that served it and closed after 708 ms |
| Failures with no kill nearby | 2: clones `n=123` (00:05:05) and `n=177` (00:07:47), before the first kill, with every replica up. Cause unknown. The pattern is the edge dropping the client's Tunnel stream before the SSH banner, not sshd refusing, because no `graft-front=` line came back |
| Pushes the client saw acknowledged | 878, and all 878 commit ids are in the group log (885 entries in total, including 7 from the smoke runs and the manual test) |
| Session time, success only | clone p50 1.68 s, p99 2.98 s; fetch p50 1.40 s, p99 2.87 s; push p50 1.64 s, p99 3.34 s |
| Replica that served each session | `front-a` (ord) 2,161, `front-b` (cdg) 408, `front-c` (hkg) 65. The Tunnel edge sends a LAX client to the nearest connected replica and uses the others only while it is gone or reconnecting |

**Kills.** Each kill was `destroy()` on the replica that had served most recently. Kill 1 hit `front-b`, which was serving then, and kills 2 to 13 hit `front-a`. Times are on Cloudflare's clock: restart comes from the front object's own event log, and tunnel ready from polling the new container's cloudflared `/ready`. The orb's clock runs 4.3 to 5.1 s behind, and the runner measured the offset at each kill. Kills 1 and 2 were rebuilt from the objects' event logs, because the first runner timed them on the orb's clock (see `results/raw/wp7/b0-24/kills.jsonl`).

| Kill | Replica | Restart | Tunnel ready | Failed sessions within 2 min | First successful session after the kill |
| --- | --- | --- | --- | --- | --- |
| 1 | front-b (cdg14) | 1.67 s | not timed | 0 of 41 | 6.7 s |
| 2 | front-a (ord06) | 0.36 s | not timed | 0 of 41 | 3.7 s |
| 3 | front-a | 1.32 s | 2.59 s | 0 of 40 | 2.9 s |
| 4 | front-a | 0.61 s | 2.20 s | 0 of 40 | 3.6 s |
| 5 | front-a | 0.79 s | 3.08 s | 1 of 41 | 6.2 s |
| 6 | front-a | 0.72 s | 2.76 s | 0 of 40 | 3.1 s |
| 7 | front-a | 1.96 s | 4.40 s | 0 of 40 | 3.3 s |
| 8 | front-a | 0.42 s | 2.54 s | 0 of 40 | 3.3 s |
| 9 | front-a | 0.48 s | 2.10 s | 0 of 40 | 3.7 s |
| 10 | front-a | 0.41 s | 2.72 s | 0 of 41 | 3.6 s |
| 11 | front-a | 0.82 s | 1.96 s | 0 of 40 | 4.1 s |
| 12 | front-a | 0.05 s | 1.50 s | 0 of 40 | 4.3 s |
| 13 | front-a | 0.73 s | 3.20 s | 0 of 40 | 3.2 s |

The supervisor restarted the container in 0.05 to 1.96 s (median 0.72 s), and the new cloudflared had a ready edge connection 1.50 to 4.40 s after the kill (median 2.59 s, 11 timed). Sessions never waited for that, because the edge sent new sessions to the other two replicas at once. After every kill, the first session that started after it succeeded on another replica. Sessions start every 3 s, and that first session ended 2.9 to 6.7 s after the kill. The killed replica took sessions again 9 to 63 s after its kill, as the edge moved traffic back to the nearest connector.

**Reading.** On this run the container half of the chain held up under kills: 13 kills cost one session, the one in flight on the killed replica. The rate, 99.886%, is one failure short of the 99.9% pass line and comes from a 2.2-hour sample, so it neither passes nor fails the row. Two of the three failures had no kill near them, so the floor here is set by the Tunnel stream path, not by kills. A day's run would be needed to tell 99.9% from 99.8% (about 21,000 sessions at this rate). That run is moot on Pro, because the Spectrum half does not exist there.

## What is modeled and what is real

| Part | Real or modeled |
| --- | --- |
| Spectrum app on Pro | Real API on the operator's Pro zone. Refused, so not in the load path |
| Client path | Real Cloudflare Tunnel from the orb (LAX) through `cloudflared access ssh`, a client-side helper the design's users would not have, in place of Spectrum on port 22 |
| Tunnel and cloudflared | Real: one tunnel, three replicas in three containers on three continents |
| SSH front containers | Real `durable_object`-policy containers with a supervisor in the object. `graft-sshd` is modeled by OpenSSH with an `AuthorizedKeysCommand` and a pre-receive hook |
| Outbound handler | Real `interceptOutboundHttp` virtual hosts handled by a `WorkerEntrypoint` with Durable Object bindings. The writes never touch the public zone |
| `Tokens` and the repository group | Modeled by one SQLite Durable Object each, with no quorum. The real commit path is B0-01 to B0-11 (M0-WP2) |
| Packs | Modeled. Each replica keeps its own bare repository, and pushed objects are not shared across replicas. Clone and fetch read the seeded `main`, which is identical everywhere |
| Kills | `container.destroy()` from the object, a clean kill. A host loss, where the object also moves, was not injected |
| Duration | 2.2 h (23:58:59 to 02:10:49 UTC), 13 kills against the row's day |

## Findings for the build

1. **sshd refuses `AuthorizedKeysCommand` in a Cloudflare container unless `/` is re-owned.** In the container `/` is owned by an unmapped uid (`stat` shows `UNKNOWN:UNKNOWN`), and sshd logs `Unsafe AuthorizedKeysCommand "/usr/local/bin/akc": bad ownership or modes for directory /` and refuses every key. The same image works under local Docker. `chown root:root /` at container start fixes it (`wp7/front/entry`). `graft-sshd` will meet the same check if it uses OpenSSH's, or must make its own.
2. **A running container keeps its object on old code across deploys.** After three `wrangler deploy`s, the three `SshFront` objects, each with a running container and a 2-second alarm, still answered with the first version's methods ("The RPC receiver does not implement the method"). They took new code only after `stop` destroyed the container and deleted the alarm, and the object went idle for about 50 s. This extends `results/container-survival.json`, which found the container survives a new version: here the object does too. An SSH front's code therefore changes only when its container is stopped, which fits the site-day rule.
3. **`setInactivityTimeout` is capped.** Asking for 24 h fails with "The maximum amount of time that a container can stay disconnected from a Durable Object is 6 hours". The supervisor alarm keeps refreshing it.

## Costs incurred

- **Spectrum: $0.** No Spectrum app was ever created, so no Spectrum bytes were transferred. The zone's `spectrum_bytes_transferred` usage stays at zero, and nothing was bought.
- Containers: three `basic` instances for about 2.6 h, on the account's Workers Paid plan, inside its included usage [inference: the B0-16 to B0-38 containers ran on the same allowance].
- Tunnel, virtual network, route, and DNS record: no charge on any plan.
- Client git bytes over the Tunnel: about 40 (a clone's pack is 30 KB; 879 clones, 880 fetches, 878 pushes) MB.

## Proposed design edits (not made; `design/` and `decisions/` are out of scope for this package)

1. `design/DESIGN.md`, "Open questions and risks". Current: "Whether an SSH-type app may use a Tunnel virtual-network origin on Pro or Business is unstated, and Milestone 0's SSH row decides it [unverified]." Proposed: "An SSH-type app cannot use a Tunnel virtual-network origin on Pro: the Spectrum API refuses `virtual_network_id` on a pay-as-you-go zone (error 10012), and a virtual-network origin needs the TCP application type, which only Enterprise has [measured, `tests/m0/results/m0-wp7.md`]. SSH needs Enterprise for BYOC."
2. `design/DESIGN.md`, "Open questions and risks". Current: "Can cloudflared run reliably inside Containers that "Cloudflare does not guarantee" to keep running (...), with Spectrum reaching it through a Tunnel virtual network, on Pro and Business as well as Enterprise?" Proposed: "Does Spectrum reach cloudflared in Containers through a Tunnel virtual network on Enterprise? Without Spectrum, three cloudflared replicas in `durable_object` containers served 99.886% of 2,637 SSH git sessions over 2.2 hours with a container killed every 10 minutes. The supervisor restarted each one in a median 0.72 s, and its tunnel was ready in a median 2.6 s [measured, `tests/m0/results/m0-wp7.md`]; Enterprise must still run the full row."
3. `design/DESIGN.md`, "Deployment modes", the `front door` row, BYOC column. Current: "Spectrum SSH is a paid add-on on Pro and Business; if Milestone 0 finds Spectrum cannot reach a Tunnel origin there, SSH needs Enterprise." Proposed: "Spectrum cannot reach a Tunnel origin on Pro [measured, `tests/m0/results/m0-wp7.md`], or on Business by the same plan table [doc], so SSH needs Enterprise; on Pro and Business an install serves HTTPS only."
4. `design/DESIGN.md`, "Install operations", the BYOC prerequisites. Current: "a zone on Pro or higher with the Spectrum add-on for SSH, and Enterprise for pushes over 200 MB;". Proposed: "a zone on Pro or higher for HTTPS, and Enterprise with Spectrum as a paid add-on for SSH and for pushes over 200 MB;".
5. `design/DESIGN.md`, "Tradeoffs accepted". Current: "We accept that SSH depends on an unverified Spectrum, Tunnel, and Container chain, and that it is dropped, or needs Enterprise, if Milestone 0 fails it on a plan." Proposed: "We accept that SSH needs Enterprise: on Pro the Spectrum API refuses a Tunnel origin [measured], so Pro and Business installs serve git over HTTPS only." Below Enterprise, the HTTPS push cap (100 MB on Pro, 200 MB on Business) then has no SSH escape, so the edge's 413 runbook and the refusal message that "names SSH" (journey 1, step 9; `decisions/DECISIONS.md`, "Pushes over the plan's body limit get Cloudflare's 413") must say "Enterprise" instead of "SSH" on those plans.
6. `design/DESIGN.md`, the Usage example. Current: "# SSH through Spectrum, if Milestone 0 passes on the plan". Proposed: "# SSH through Spectrum, on Enterprise zones".
7. `design/DESIGN.md`, "Cost". The line "Spectrum add-on fee on Pro or Business [unverified ...]" and `scripts/cost.py`'s `SPECTRUM_ADDON_USD`, `ssh_inc = 5 if ... pro` stop applying below Enterprise. The small and medium tiers on Pro should drop the Spectrum SSH data line (SSH share 0), and the Enterprise tier keeps it. Rerun `scripts/cost.py` and `scripts/costs_report.py` afterwards.
8. `design/DESIGN.md`, Shape, "SSH front" and the build: add finding 1 (`chown root:root /`, or `graft-sshd` doing its own key lookup without OpenSSH's path-ownership check) and finding 2 (an SSH front's code changes only when its container stops).
9. `decisions/DECISIONS.md`, "SSH goes through Spectrum, Tunnel, and the container outbound handler". Its flip condition has fired for Pro. Record it: "Flip fired on Pro, 2026-10-30 (`tests/m0/results/m0-wp7.md`); SSH is Enterprise-only for BYOC." Close `decisions/OPEN-QUESTIONS.md` C9 for Pro, and leave it open for Enterprise (Spectrum reaching cloudflared in Containers, measured only as the Tunnel half here).
10. `FRAME.md`, answer 7 ("Keep SSH, subject to Milestone 0's Spectrum-to-Tunnel row on each plan"). The operator should confirm SSH is Enterprise-only, or reopen the question. A design edit cannot settle that, because it is an operator decision.

## What stays deployed, and teardown

Kept for a rerun or an Enterprise follow-up, all idle and costing nothing while the fronts are stopped:
- Worker `graft-m0-ssh` (workers.dev only, no zone route) with its `SshFront` and `Registry` objects. All three fronts are **stopped** (`wanted: false`, no alarm, no container), and the tunnel shows 0 connectors.
- Tunnel `graft-m0-ssh` (`6dc68d9d-ab65-4767-9391-418b6bf46c0b`), virtual network `graft-m0-ssh-vnet` (`ab484908-ad4a-4d2b-a301-01992163411a`), route `10.222.0.22/32`.
- DNS record `ssh-m0.graftyard.dev` (CNAME to the tunnel, proxied, id `2ac4e36fc295184ec3e17cbb2154c0e2`). `m0.graftyard.dev` was not touched.
- No Spectrum app exists. The orb service `wp7-load` is stopped.

A rerun from `tests/m0` is `node run/wp7.ts deploy` (it restarts the fronts), then `node run/wp7.ts load --run <name> --hours N` under `amp orb service start`, then `node run/wp7.ts report --run <name>`.

Teardown, from `tests/m0`:

```
amp orb service stop wp7-load
node run/wp7.ts delete      # stops the three fronts; deletes graft-m0-ssh, the tunnel (after clearing connections),
                            # the 10.222.0.22/32 route, graft-m0-ssh-vnet, the ssh-m0.graftyard.dev record, and any
                            # Spectrum app named ssh.graftyard.dev (none exists)
rm -rf .gen/wp7             # local host and client keys, known_hosts, cloudflared binary
```

## Files

- `src/wp7/ssh.ts`, `wp7/graft-m0-ssh.jsonc`, `wp7/front/` (Dockerfile, `entry`, `sshd_config`, `akc`, `gitwrap`, `pre-receive`)
- `run/wp7.ts`: `spectrum`, `deploy`, `status`, `load`, `report`, `delete`
- Raw data in `results/raw/wp7/` (not committed): Spectrum requests and responses, zone subscription and entitlements, `b0-24/sessions.jsonl`, `b0-24/kills.jsonl`, `b0-24/summary.json`, and `NOTES.md` (the run log)

## B0-24a: 24-hour run

The operator decided how SSH works: HTTPS by default; on Pro and Business, SSH as an option through the Tunnel with `cloudflared access ssh` on the client; on Enterprise, native SSH through Spectrum. B0-24 is now split. B0-24a is the Pro and Business option, gated at 99.9% of sessions over a day with one container killed every 10 minutes, and failing under 99%. B0-24b is Enterprise Spectrum, which is not run because the account has no Enterprise zone.

### The two unexplained drops from the 2.2-hour run

**Sources.** With `observability` enabled, the container's stdout and stderr (sshd and cloudflared) reach Workers Logs under the container application's id, `a0d32e27-2720-43b9-bc32-a5370151429d` (dataset `containers`, kept 7 days). Each `SshOutbound` call is also logged there, and the client errors are in `sessions.jsonl`. The orb's clock runs about 4.46 s behind Cloudflare's, and the times below are on Cloudflare's clock. The raw extracts are in `results/raw/wp7/drops/`.

| Session | What the logs show |
| --- | --- |
| `n=123` clone, 00:05:09.7, failed after 1,251 ms with `Connection closed by UNKNOWN port 65535` | No replica's sshd accepted a connection and no `tokens.graft` lookup came in, so the client's stream closed before the SSH banner. 120 s later, `front-b` logged `Timeout before authentication for connection from 127.0.0.1` (pid 3859). That is a stream from its cloudflared to sshd opened at about 00:05:11.9, after the client had already given up at 00:05:11.0, which then carried no byte and was never closed. It was the only such event in the run. The edge opened the session's origin stream about a second too late, and closed the client side without closing the origin side |
| `n=177` clone, 00:07:51.75, failed after 1,063 ms with `closed by remote host`, `Broken pipe` | `front-a` (ORD) accepted the key 773 ms in and resolved it on `tokens.graft`, then logged no disconnect for that port, ever. The stream was cut between the client and the replica's cloudflared after authentication, with sshd never seeing a close. That replica's cloudflared logged a connection-retry error 1.5 s later |

**Ruled out.**
- **Replica restart.** No kill, start, or restart event is in any front's log in either window, and all three connectors were registered.
- **Idle timeout.** Sessions last 0.7 to 3.4 s, so nothing idles long enough to hit one.
- **Client-side reconnect.** `cloudflared access ssh` printed nothing for either session.
- **The kill path.** The third failure (`n=1000`) was the one in flight during kill 5.

**What remains is the Cloudflare Tunnel edge.** Both drops are a stream between the client's `cloudflared access` and the replica's cloudflared, opened late or cut, with every replica healthy. One pattern on our side was worth removing. With the default of 4 connections per replica, connections 2 and 3 never stayed registered: they looped on `already connected to this server, trying another address` 184 times in 2.2 hours. Two of the three failed sessions fell within ±3 s of such an error on the serving replica, against 16% of successful sessions (420 of 2,634). That is weak evidence, with n = 2.

**Changes for the 24-hour run.**

| Where | Change | Why |
| --- | --- | --- |
| `wp7/front/entry` | `cloudflared tunnel --ha-connections 2` (was the default 4) | Removes the endless re-registration loop. Each replica still holds two edge connections in two colos (checked: ord06 and ord11, cdg11 and cdg14, hkg12 and hkg09). Three replicas give six connections |
| `wp7/front/sshd_config` | `LoginGraceTime 30` (was 120), `ClientAliveInterval 15`, `ClientAliveCountMax 4` | Reaps an orphaned origin stream like `n=123`'s in 30 s, and a dead session in about 60 s |
| `wp7/ssh_config` (client) | `ServerAliveInterval 15`, `ServerAliveCountMax 4` | A stream cut during a long clone or push is detected in about 60 s instead of waiting on TCP. It cannot rescue a stream that is cut, so its effect on the success rate is expected to be zero |
| `wp7/front/akc`, after the second and third 24-hour attempts | Retry the key lookup for up to 25 s (5 s tries, 1 s apart) | See "A key-lookup failure found in the second attempt" |
| Not changed | `--protocol http2` kept | Both protocols pass the replica's own pre-check. The failures are not in the replica's connection to the edge |
| Not changed | `--retries` | Its default of 5 governs the replica's own edge reconnects, which never failed |
| Not changed | Client `ConnectionAttempts` | No effect with a ProxyCommand: measured, a failing ProxyCommand was invoked once with `ConnectionAttempts=3` |
| Client stanza, added after the first 24-hour attempt | `ConnectTimeout 30` | See the next subsection |

A client-side retry is the only change that would turn these edge drops into successes, and plain `git` over SSH has none. The docs should say a dropped session can simply be retried, and that a dropped push is safe to retry, because git applies a push only after receive-pack completes.

### A client hang found in the first 24-hour attempt

The first attempt (`b0-24a-24h`, 06:18 to 06:50 UTC) was stopped after 32 minutes with 341 sessions, 338 successful. At 06:33:38, a fetch and a push, 3 s apart, each opened a TCP connection from `cloudflared access` to the same edge address, 104.26.4.184:443. The edge acknowledged 1 byte on each and then nothing: `ss` showed 18 data segments sent, 20,962 bytes retransmitted, and a retransmission timeout backed off to 120 s. Neither session reached a replica. Neither the SSH client nor `cloudflared access` gives up on a TCP connection that is open but silent. `ServerAliveInterval` starts only after the SSH handshake, and `ConnectTimeout` covers only the banner exchange (both measured with a stub ProxyCommand). So the user's `git` would hang until TCP gave up, about 15 minutes, which is what happened here (`read: connection timed out` after 946 s). The runner's 90 s timeout killed `git` but not its `ssh` and `cloudflared` children, which kept the output pipes open. That stalled both workers for 15 minutes. Two fixes followed, and the 24-hour run was restarted with a fresh name, `b0-24a-r2`:
- The runner kills the session's whole process group on timeout.
- The client stanza adds `ConnectTimeout 30`. With a stub ProxyCommand that stays silent, `ConnectTimeout=5` failed in 5 s, against a hang with no option set.

### A key-lookup failure found in the second attempt

The second attempt (`b0-24a-r2`, 06:51 to 07:17 UTC, 536 sessions, 533 successful) was stopped after 25 minutes. Two of its failures came 6 s apart, on `front-c` (HKG), with `Permission denied (publickey)`. The logs show:
- At 07:15:18, sshd's `AuthorizedKeysCommand` failed with curl status 28, after 10 s with no reply from `tokens.graft`.
- At 07:15:20, the `SshOutbound` invocation for that lookup ended in `exception`, `internal error; reference = d7kdc8jf602ss7p7l26uj79j`, after 12.2 s of wall time.
- At 07:15:24, a second lookup ended in `internal error` after 4 ms, and curl got status 52 (empty reply).

Lookups on the same container a second either side succeeded, and `Registry` showed no error. It is a transient failure of the container outbound handler itself. sshd runs `AuthorizedKeysCommand` once per key and does not retry. `wp7/front/akc` therefore got three tries of 5 s each, and the run restarted as `b0-24a-r3`.

The third attempt (`b0-24a-r3`, 07:20 to 07:33 UTC, 267 sessions, 263 successful) was stopped after 13 minutes. Kill 1 came at 07:30:11. From 07:30:06 to 07:30:29 the `Registry` object `tokens` served no call at all, from any of the three containers. Three lookups from the restarted `front-b` failed with `internal error` and exhausted the three tries, and `front-c`'s lookups waited 4.5 to 10.6 s. At 07:30:29 the object answered all the queued calls at once. So the stand-in `Tokens` group, a single Durable Object, was unavailable for about 23 s. That is the kind of gap a real group's leader change or object restart produces, and with more than one member the real design reads keys from a group, not one object. `akc` now keeps retrying for up to 25 s, inside `LoginGraceTime 30`. That was the last change. The fourth attempt, `b0-24a-r4`, is the run of record, and it ran with no further changes whatever it found.

Two lessons for the build follow. `graft-sshd` must retry key resolution and writes through the outbound handler for longer than a group's failover, and it must hold the client's connection while it does. A single outbound-handler call can fail on its own with `internal error`, and the object behind it can be unreachable for tens of seconds.

### The client config

`wp7/ssh_config` is the stanza the install docs give, and the load runner uses it unchanged, through `ssh -F` with an `Include` of the file. The only lines the runner adds are the test key, `known_hosts`, and `LogLevel VERBOSE` for diagnosis:

```
Host ssh-m0.graftyard.dev
  User git
  ProxyCommand cloudflared access ssh --hostname %h
  ServerAliveInterval 15
  ServerAliveCountMax 4
  ConnectTimeout 30
```

- `ConnectTimeout 30` turns a silent edge connection, before the SSH banner, into a failure in 30 s instead of about 15 minutes.
- `ServerAliveInterval 15` with `ServerAliveCountMax 4` does the same for a stream that dies after the handshake, in about 60 s.
- Neither retries. The user runs the command again.


### The run of record: `b0-24a-r4`

**Status: in progress.** The run started at 2026-10-31 07:36:08 UTC and ends at 2026-11-01 07:36:08 UTC. The service `wp7-24h-r4` runs it, and the service `wp7-finish` writes the report and stops the three fronts at 07:41 UTC. This section gets its final numbers when the run ends. Interim at 08:54 UTC (1.3 h): 1,554 sessions, 4 failed, 7 kills. All 4 failures were the session in flight on the killed replica: each started 2.2 to 2.6 s before its kill and was cut by it.

**Setup.** This is the same bed as the 2.2-hour run. The changes are the ones listed above: replicas run `--ha-connections 2`; sshd runs `LoginGraceTime 30` and `ClientAliveInterval 15`; `akc` retries for 25 s; the client uses the documented stanza; and the runner kills a session's whole process group after 90 s. Two workers run one session every 6 s each, rotating clone, fetch, and push, for about 20 sessions a minute and about 28,800 a day. A kill every 10 minutes gives 144 kills. Each kill destroys the replica that served most recently.

**Results.** Pending until the run ends at 07:36 UTC on 2026-11-01. The final table (sessions, success rate against 99.9% and 99%, failure causes, reconnect times after kills, and latency p50 and p99 by operation) comes from `node run/wp7.ts report --run b0-24a-r4`, which writes `results/raw/wp7/b0-24a-r4/summary.json`.

### What is modeled and what is real (B0-24a)

| Part | Real or modeled |
| --- | --- |
| Client | Real: OpenSSH with the documented stanza and `cloudflared access ssh` 2026.10.7, from one orb in LAX. One client location only, so the edge colo and the nearest replica are fixed (mostly `front-b` or `front-a`) |
| Tunnel, edge, cloudflared replicas | Real: one tunnel, three replicas in three `durable_object` containers (ENAM, WEUR, APAC), 2 edge connections each |
| SSH server | Modeled: OpenSSH with `AuthorizedKeysCommand` and a pre-receive hook stands in for `graft-sshd` |
| Outbound handler | Real `interceptOutboundHttp` virtual hosts on a `WorkerEntrypoint` with Durable Object bindings |
| `Tokens` and the repository group | Modeled: one SQLite Durable Object each, with no quorum. The 23-second gap in the third attempt was this single object |
| Packs | Modeled: each replica has its own bare repository. Pushes are acknowledged by the group log, not replicated across replicas |
| Kills | `container.destroy()`, a clean kill of one replica. Host loss and colo loss were not injected |
| Load | Steady, 2 concurrent sessions, about 30 KB per clone. No large packs and no long sessions, so `ServerAliveInterval` and `ClientAliveInterval` were never needed |

### Proposed design edits for B0-24a (the design editor owns `design/` and `build/`)

1. `design/DESIGN.md`, Shape, "SSH front", and the install docs: the client stanza, verbatim:
   "`Host <ssh host>` / `User git` / `ProxyCommand cloudflared access ssh --hostname %h` / `ServerAliveInterval 15` / `ServerAliveCountMax 4` / `ConnectTimeout 30`". Add the sentence: "Without `ConnectTimeout`, a connection the edge accepts but never answers hangs `git` for about 15 minutes [measured, `tests/m0/results/m0-wp7.md`]. A session that fails can be retried: git applies a push only after receive-pack completes."
2. Same section: "cloudflared runs with `--ha-connections 2`; the default of 4 left two connections per replica re-registering against 'already connected to this server' for as long as the replica ran [measured]."
3. Same section, on `graft-sshd`: "Key resolution and writes through `ssh_outbound` are retried for up to the group's failover time, holding the client's connection while it does. In Milestone 0 a single outbound-handler call failed alone with `internal error`, and the stand-in `Tokens` object was unreachable for about 23 s [measured]." Also: "sshd sets a short pre-authentication grace (30 s), because the Tunnel edge can open an origin stream after the client has gone and never close it."
4. Same section: the `chown root:root /` finding, or an equivalent ownership check in `graft-sshd` (from the first run).
5. `build/BUILD-PLAN.md`, B0-24a and B0-24b, with the result of this run once it ends. If the verdict is the expected one, add: "Failures are dominated by sessions in flight on the killed replica, so a 10-minute kill period puts a floor of about one failure per kill under the rate."

### What stays deployed after B0-24a

When `wp7-finish` has run:
- **Stopped:** the three fronts, `graft-m0-ssh`'s `SshFront` objects (no container, no alarm).
- **Kept:** the Worker `graft-m0-ssh`, the tunnel `graft-m0-ssh` (`6dc68d9d-ab65-4767-9391-418b6bf46c0b`), the virtual network `graft-m0-ssh-vnet` (`ab484908-ad4a-4d2b-a301-01992163411a`), the route `10.222.0.22/32`, and the DNS record `ssh-m0.graftyard.dev`.
- **No Spectrum app.**

Teardown, from `tests/m0`:

```
amp orb service stop wp7-24h-r4; amp orb service stop wp7-finish
node run/wp7.ts delete
rm -rf .gen/wp7
```
