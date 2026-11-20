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
- **Containers.** The teardown sweep killed the three git shard containers. The three `SshFront` containers (`ssh#0`, `ssh#1`, `ssh#2`) were still running at 05:12Z. The Containers API refuses an instance delete (`NOT_ENABLED`), so they stop when their 1 h inactivity timeout runs out. Their alarm restarts a container only within an hour of the last session, so none restarts.
- **Left in place.** The secrets `SSH_HOST_KEY_A` and `SSH_HOST_KEY_B` stay in the install's Secrets Store, so a later site day with `--ssh` serves the same host keys.
