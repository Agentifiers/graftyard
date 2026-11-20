# M6-WP2, the SSH front through a Spectrum Worker application

This file records M6-WP2's hosted rows on `graft-m3f` (`tests/am1/results/install-m3f.md`, the 2026-11-20 04:20Z site day), on branch `ssh-spectrum-worker` (main `f413f2ac` plus native SSH). The design is `design/DESIGN.md`, Shape, "SSH front", and `decisions/DECISIONS.md`, "SSH goes through a Spectrum Worker application to the SSH front (2026-11-19)". Raw logs are in `tests/m6/raw/b024d/`; the drivers are `tests/m6/hosted/ssh-e2e.sh` and `tests/m6/hosted/bf52-ssh-suspend.sh`.

Status: **B0-24d passes** (all four parts). **BF-51 passes** its client half; the per-replica fingerprint check was not run separately. **BF-52's suspension half passes** hosted; its revocation half with 1,000 queued commands is a unit test only. **B0-24a on the Spectrum ingress is not run.**

## B0-24d, part 1: the Durable Object and container hops (2026-11-20 01:41Z)

A throwaway Worker, `graft-sshprobe` (deleted afterwards), passed a `fetch` to a Durable Object, which opened `ctx.container.getTcpPort(22).connect(...)` to a socat echo in its container and wrote N bytes (`hops.json`).

| Run | Open | Echo | Result |
|---|---|---|---|
| 64 KiB, cold start | 198 ms | 2,047 ms | 0 bytes back (the container was still starting) |
| 1 MiB | 23 ms | 18 ms | byte-exact |
| 4 MiB, three runs | 20 to 137 ms | 839 to 991 ms | byte-exact |
| 64 KiB, cold after destroy | 17 ms | 223 ms | 0 bytes back |

A cold container accepts the TCP open before its process listens. That is why `SshFront` waits for the SSH banner, retrying every 500 ms for up to 45 s (`workers/site/ssh.mjs`, `open`), before it forwards any client byte [measured].

## B0-24d, part 2: end to end through Spectrum (2026-11-20 04:36Z to 04:55Z)

**Setup.**
- Site day and edge day from `ssh-spectrum-worker`, with `--ssh`, image `sshfront`, and host-key slots A and B in the install's Secrets Store. Sites a/b/c are `56a05191` / `3ab5c062` / `a4b629fc`.
- The edge is `7cb336fc`, then `44921364` with the colo fix below.
- `graftyard ssh spectrum` created a Spectrum TCP app on port 22, Worker origin `graft-m3f-edge`, hostname `sshtest.graftyard.dev`, on the Pro zone `graftyard.dev` at 04:36:56Z.
- The client was stock OpenSSH and stock git from the orb, with no `cloudflared`.

**First run (`e2e-m3fssh1-colo-panic.log`).** The clone was refused with "Permission denied (publickey)" after 26 s. The edge's key route panicked in `worker::Cf::colo`, because a service-binding request carries no `cf` object. The fix reads `cf.colo` through `Reflect` and falls back to none (`workers/edge/src/ssh.rs`, `colo`). The SSH handshake, host key, and banner path all worked in this run.

**Second run (`e2e-m3fssh2.log`, edge `44921364`).**

| Step | Result |
|---|---|
| Key add (`POST /user/keys`) | 201 |
| Repository create | 201, `m3-member/ssh-m3fssh2` (repo:3932161) |
| `git clone git@sshtest.graftyard.dev:m3-member/ssh-m3fssh2.git` | rc 0, 5.6 s; host key ED25519 `SHA256:TMEOnX…` (slot A) |
| Commit and `git push` | rc 0, 9.5 s; main read back over HTTPS matches the pushed commit |
| Fetch | rc 0, 2.7 s |
| Second push (main and a new branch), then branch delete | both rc 0 |
| Session held on one connection: v2 advertisement, then one `ls-refs` every 60 s | 633 s, 11 of 11 answered, ssh exit 0 |
| Key delete (`DELETE /user/keys/{id}`) | 204 |
| Clone, then push, with the deleted key | both rc 128 "Permission denied (publickey)"; main unchanged |

**Verdict: pass [measured].** All four parts of the row pass: stock clone, push, a key refused after its removal, and a session held 10 minutes or more. Pushes went through the edge's receive-pack path to the repository group as format 32.26 with `CredentialKey::SshKey`. The first raw TCP banner after the deploy took 6.3 s, a cold container. Warm clones took 2.7 to 5.6 s.

## BF-51: host-key rotation (2026-11-20 04:50Z)

A client with a registered key and `UpdateHostKeys=yes` connected with slot A's ED25519 key in `known_hosts`. After one session, `known_hosts` held 6 keys: slot A's and slot B's ED25519, ECDSA P-256, and RSA. These match the six fingerprints that `GET /api/graftyard/v1/meta/ssh` lists. `graft-sshd` announced both slots with `hostkeys-00@openssh.com` and answered the client's `hostkeys-prove-00@openssh.com`. **Pass on the client half [measured].**

Every front reads the same two Secrets Store secrets, so the fingerprints are the same on every replica by construction [inference]. Each front was not dialled separately, because the edge chooses the site.

## BF-52: suspension over SSH (2026-11-20 04:56Z to 05:01Z, `bf52-ssh-suspend.log`)

The driver used a new member account and repository (`m6-ssh-suspend/bf52-1795150613`, repo:3997696, account 7290061) with an SSH key, and suspended the account through the test bed's identity route.

- Push before the suspension: ok.
- Suspended: 20 of 20 SSH pushes refused with "Bad credentials", with no ref moved, and none accepted.
- Push after the unsuspension: ok.

**Pass on the suspension half [measured].** The revocation half (1,000 queued commands, every command decided after the revocation gets 401) is in `crates/git/tests/push.rs`, the `bf52_*` tests, and was not run hosted.

## Spend and teardown

- **Spend.** $0.17 on `graft-m3f` from the site day's start, 04:21:12Z to 05:12:43Z (`PREFIX=graft-m3f tests/fanout/acctcost.py`), under the $5 stop. That covers containers, Durable Object rows, and requests. The Spectrum app's own billing line was not read [unverified]. The traffic was a few MiB, inside Pro's 5 GB allowance.
- **Spectrum app.** Deleted with `graftyard ssh spectrum --delete`. Afterwards the zone lists 0 Spectrum apps and no `sshtest` DNS record. `ssh.graftyard.dev`, `ssh-m0.graftyard.dev`, and the Oracle VM were not touched.
- **Repositories.** repo:3932160, 3932161, and 3997696 were retired and purged: 58 objects, then 0 on the second pass. The first pass refused 3997696 on a registry TLS timeout, and a rerun purged it. `joined.mjs` reports none joined of 3.
- **Containers.** The teardown sweep killed the three git shard containers. The three `SshFront` containers (`ssh#0`, `ssh#1`, `ssh#2`) were still running at 05:12Z. The Containers API refuses an instance delete (`NOT_ENABLED`), so they were left to their inactivity timeout. Site a's stopped at 05:17:28Z and sites b's and c's at 06:16:40Z and 06:17:02Z, later than the 1 h expected ("Idle stop" below).
- **Left in place.** The secrets `SSH_HOST_KEY_A` and `SSH_HOST_KEY_B` stay in the install's Secrets Store, so a later site day with `--ssh` serves the same host keys.

## Idle stop: why two fronts ran 1 h 15 min past their last session (2026-11-20)

The fronts were left to stop on their own after the teardown. `IDLE_MS`, the container's inactivity timeout, is 1 h, so all three should have stopped by about 06:02Z. Instead:

| Front | Last `connect()` began | Its wall time | It ended | Container stopped | Stop after that end |
|---|---|---|---|---|---|
| site a, `ssh#0` | 05:01:59Z | 904,404 ms | 05:17:03Z | 05:17:28Z | 25 s |
| site b, `ssh#1` | 05:01:41Z | 900,691 ms | 05:16:42Z | 06:16:40Z | 3,598 s |
| site c, `ssh#2` | 05:01:58Z | 904,936 ms | 05:17:03Z | 06:17:02Z | 3,599 s |

The begin times and wall times are from Workers Logs, entrypoint `SshFront`, event type `connect`. The stop times are each instance's `status.updated_at` in the Containers API. Every earlier `connect()` on these objects ended within seconds of its session (the 632,712 ms one on site b is the 633 s held session), so only the last invocation on each object ran about 900 s past its socket [measured].

**Cause.** `up()` left a `monitor()` pending to arm a restart alarm when the container stopped. A pending `monitor()` "prevents eviction until the container exits or for up to 15 minutes, whichever comes first" [doc: https://developers.cloudflare.com/containers/api/durable-object-container/, `monitor`]. That is the 900 s on each last invocation. The inactivity timeout runs only once the object is inactive [doc, same page, `setInactivityTimeout`], so b and c stopped 1 h after the hold ended, to within 2 s. Site a stopped 25 s after its hold. Its object had been reset since its container started (an object that restarts "starts without" a timeout, same page), so the platform stopped its container "shortly after" the object went inactive [inference].

**Fix** (`decisions/DECISIONS.md`, "An SSH front holds nothing past its session"). `SshFront` keeps no alarm and calls no `monitor()`. A stopped container is started again by the next session, which already waits for the banner. A test models the stop as the latest of the session's end, a pending `monitor()`'s 15 minutes, and a pending alarm, plus the inactivity timeout (`workers/site/tests/ssh.test.mjs`, "a front's container stops within IDLE_MS of its last session's end"). It reports 75 minutes on the code before the change and 60 after. Not yet hosted: it ships on the next `graft-m3f` site day.

## Cutover: `ssh.graftyard.dev` moves to the Spectrum Worker app, and the Oracle VM retires (2026-11-20)

The operator approved it on 2026-11-20: "Yes you can take out the oracle vm and go pure cloudflare for ssh" (`FRAME.md`, "Operator decision, 2026-11-20, SSH on Cloudflare only"). The orchestrator chose `graft-m3f` as the install behind the hostname.

**Before.**
- `graftyard.dev` (Pro) held two DNS records:
  - `ssh.graftyard.dev`: A to the VM's ephemeral IP, DNS only, TTL 300, comment `graft-ssh-proxy (deploy/oci-ssh)`;
  - `ssh-m0.graftyard.dev`: a proxied CNAME to Tunnel `graft-m0-ssh`, whose ingress was `ssh://localhost:22`, with one private-network route on virtual network `graft-m0-ssh-vnet` (M0-WP7).
- The zone had no Worker routes, custom domains or Spectrum apps, and no Graftyard install served it.
- The Tunnel had 0 connectors, because the M0 bed was stopped. So the hostname served nothing: `ssh -v` connected, then got `kex_exchange_identification: read: Connection reset by peer` before any banner.
- The VM path never terminated SSH. Clients that reached the bed saw its Ed25519 key, `SHA256:GCDfbpG3KG4ODrXC4LI4nA1yUYacmDBWuzGJA1xH4u4` (`tests/m0/results/m0-wp7.md`).
- The rollback copies are kept outside the repository on the building orb: the A record, the `ssh-m0` record, the Tunnel's configuration, its route, and the virtual networks.

**Deploy.**
- Site day m3fcut from main `e5590f36` plus the SSH-off guard, started 06:58:36Z.
- It took three tries. The first two failed on site a with wrangler 4.153.0's "Container application settings were not fully applied"; the third (`--skip-build`) ran both phases and exited 0 at 07:13:15Z.
- Sites a/b/c are `a266ac23` / `0af66f09` / `8631a3b6`, with the shard image `sha256:076a16ee…` and sshfront `sha256:fc6eda91…` reused.
- Edge day: `c736b6e8` at 07:15:49Z, with `--ssh` and FORMATS through 32.26.

**Cutover.**
1. 07:16:05Z: deleted the A record.
2. 07:16:07Z: `graftyard ssh spectrum --hostname ssh.graftyard.dev --edge graft-m3f-edge` created app `1c5b8da1…` (`tcp/22`, `traffic_type: worker`, origin the edge's tag).
3. From 07:17:15Z, `ssh-keyscan` returned the ED25519 key `SHA256:TMEOnXhyIwFeNgR39+c4dKwYPTRbXGjPtP2AM3twOoU`.

**Verification** (`tests/m6/raw/cutover/e2e-m3fcut.log`; stock OpenSSH and git, no `cloudflared`):

| Step | Result |
|---|---|
| `git clone git@ssh.graftyard.dev:m3-member/ssh-m3fcut.git` | rc 0, 8.7 s |
| Host key | ED25519 `SHA256:TMEOnX…`, slot A. It is one of the six fingerprints `GET /api/graftyard/v1/meta/ssh` lists (both slots' ED25519, ECDSA, and RSA) |
| Commit and `git push` | rc 0, 9.3 s; main read back over HTTPS matches the pushed commit `284be970` |
| Fetch, a second push with a new branch, and the branch delete | all rc 0 |
| A session held 60 s with `ls-refs` | 1 of 1 answered |
| Key deleted, then clone and push | both refused "Permission denied (publickey)", main unchanged |

**The host key changed.**
- Clients that once reached `ssh.graftyard.dev` through the VM saw the M0 bed's key, `SHA256:GCDfbpG3KG4ODrXC4LI4nA1yUYacmDBWuzGJA1xH4u4`. They now see `graft-sshd`'s slot-A ED25519 key, `SHA256:TMEOnXhyIwFeNgR39+c4dKwYPTRbXGjPtP2AM3twOoU`.
- OpenSSH warns "REMOTE HOST IDENTIFICATION HAS CHANGED" for anyone with the old entry. Fix it with `ssh-keygen -R ssh.graftyard.dev` and check the new key against `/api/graftyard/v1/meta/ssh`.
- The operator accepted the change.

**Retired, after the verification passed.**
- **Oracle:** `deploy/oci-ssh/teardown.sh` with the 1Password `Oracle` API key.
  - Instance `graft-ssh-proxy` TERMINATED (07:20 to 07:22Z), boot volume TERMINATED; subnet, internet gateway and VCN deleted; no reserved public IP existed.
  - The $1 budget `graftyard-oci-1usd` is kept, as the runbook says, and so is the tenancy.
  - The script's first run exited silently: its `oci()` wrapper hides stderr under `set -e`. The terminate was then issued by hand, and the script rerun with stderr shown finished the network.
- **Cloudflare:**
  - The `ssh-m0.graftyard.dev` record deleted.
  - Tunnel `graft-m0-ssh` deleted. The first delete was refused (1023, private network routes), so its route went first, then the tunnel, then its virtual network `graft-m0-ssh-vnet`.
  - The account now has 0 tunnels, 0 routes, and only the default virtual network.
- **1Password:** the SSH Key item `graftyard-ssh-proxy` is kept. The 1Password CLI refuses to edit SSH Key items ("SSH Key item editing in the CLI is not yet supported"), so a Secure Note, "graftyard-ssh-proxy RETIRED 2026-11-20", tagged `graftyard` and `retired`, marks it. The vault had no other retire convention.
- **Orb:** `~/.oci` removed.

**After.**
- The zone holds one record, the Spectrum app's own `ssh.graftyard.dev`, and one Spectrum app.
- Cloudflare is the only product in any serving path.
- **`ssh.graftyard.dev` now depends on `graft-m3f`.** Every later m3f site day and edge day must pass `--ssh`. Both deploy scripts list the account's Spectrum apps and refuse without it while an app's origin is the install (`workers/site/sshguard.mjs`); `--ssh-off` turns SSH off on purpose. Against the live app, both scripts refused a deploy without `--ssh`: "a deploy without --ssh turns native SSH off for ssh.graftyard.dev (origin graft-m3f-edge)".

**Spend.** $0.03 on `graft-m3f` from 06:58:36Z to 07:31:28Z (`acctcost.py`, `prefix_usd`), under the $5 stop. Oracle billed nothing: the VM was Always Free.

**Cleanup.**
- The test repository `m3-member/ssh-m3fcut` (repo:3932162) was retired (`retirenets.mjs`: 4 names, all not joined) and purged (28 objects, then 0 on the second pass).
- The three `SshFront` containers are left as they are. `ssh.graftyard.dev` is a live hostname now, and with the idle fix each stops 1 h after its last session.
