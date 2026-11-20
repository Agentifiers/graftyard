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
