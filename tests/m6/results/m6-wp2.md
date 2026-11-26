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

## B0-24a on the Spectrum ingress: plan (2026-11-26, not run)

The plan for `build/BUILD-PLAN.md`, M6-WP2's last row: three SSH fronts, one killed every 10 minutes for a day, with sessions through Spectrum and the edge's `connect()`, and with writes. **Pass:** at least 99.9% of sessions succeed and no acknowledged push is lost. **Fail:** under 99%, or a lost acknowledged push. It also runs BF-52's revocation half. Nothing here has run. Nothing is deployed until the lead's GO.

### What the gate can reach with clean kills

A `destroy()` kill ends every session in flight on the killed front. The edge picks the first front to try by `now_ms % 3` (`workers/edge/src/ssh.rs`, `order`), so each front carries a third of the sessions in flight. With sessions starting at rate r and lasting d seconds on average, one kill costs r·d/3 sessions. 144 kills cost 144·r·d/3 out of r·86,400 sessions, so the failure rate from kills alone is **d / 1,800 s, whatever the load** [inference].

| Mean session time d | Failure rate from kills alone | Success before any background failure |
| --- | --- | --- |
| 1.5 s | 0.083% | 99.92% |
| 1.8 s | 0.100% | 99.90%, the pass line |
| 3 s | 0.167% | 99.83% |
| 5 s | 0.278% | 99.72% |

B0-24d's warm clones took 2.7 to 5.6 s end to end. B0-24a's cloudflared run had p50s of 2.6 to 3.0 s and lost 69 in-flight sessions over 143 kills (`tests/m0/results/m0-wp7.md`, "The run of record"). So the row is likely to land between the lines on the kills alone [inference]. The design gives the client no retry. The install docs only say that a failed session, a push included, is safe to retry. The lead decides whether to run the gate as written, knowing this, or to have the operator read it the way B0-24a's analysis did, with the failures in flight at a kill counted apart from the rest. The driver records every number both readings need.

### How a front is killed without a site day per kill

The deployed sites (`graft-m3f-site-a` `5b742740`, site day 2026-11-24 23:14Z) cannot kill a front from outside:
- `SshFront` answers only `GET /status` (`workers/site/entry.mjs`), and the Registry's `/ssh_front` only reads it (`workers/site/src/registry.rs`).
- The Containers API lists and reads instances but has no delete (`NOT_ENABLED` on 2026-11-20, above, "Spend and teardown").
- Changing the container application would be a container config change. That ships only through a site day (`AGENTS.md`, "Invariants never to break").

So the row ships one small change, test installs only, on **one** site day at its start. After that, every kill is an ordinary call and creates no script version:
- **`SshFront` `POST /kill`** (`workers/site/entry.mjs`): `ctx.container.destroy("b024a kill")`, the same clean kill B0-24a used. It answers `{at, running, open}`, where `open` is the number of sessions this object had in flight at that moment.
  - The object counts its open `connect()` calls. `GET /status` reports the same count.
  - It answers only when the site's `SSH_KILL` variable is `1`. `workers/site/deploy.mjs` sets that variable only with `--test-faults`.
- **Registry `POST /ssh_front_kill`** (`workers/site/src/registry.rs`), under `#[cfg(feature = "test-faults")]`. It forwards to this site's `ssh#<site>`, as `/ssh_front` does.
- **The tools edge's `/ssh_kill {site}`** (`tests/m1/wp7/edge.ts`). It binds `REGISTRY_<n>` already and is deployed by its own script, so this is no site day. The product edge does not change.
- **Tests:** a node test in `workers/site/tests/` for the kill, the open count and the variable gate, and a plant for each.

The front comes back the way it does after an idle stop: the next session routed to it starts the container and waits up to `BANNER_WAIT_MS` (45 s) for the banner before it forwards a byte (`workers/site/ssh.mjs`, `open`). A cold start took 2 to 6 s in B0-24d. The killer probes the killed front's `/status` every 500 ms. Restart time is measured from the kill's `at` to the first banner.

### The driver

A new resumable Node driver in `tests/m6/hosted/`, run as a supervised orb service, as B0-24a's `wp7-load` was. Its structure (load loop, killer, report) comes from `tests/m0/run/wp7.ts`, and its key, repository and client setup from `tests/m6/hosted/ssh-e2e.sh`.
- **Setup:**
  - A new ed25519 key added to `m3-member` with `POST /user/keys`.
  - A repository `m3-member/b024a-<tag>` with a README.
  - The hostname is `ssh.graftyard.dev`, the live Spectrum app, so no Spectrum app is made or changed.
- **The client:** stock OpenSSH and stock git, with `ConnectTimeout 30`, `ServerAliveInterval 15`, `BatchMode`, and a pinned `known_hosts` holding slot A's ED25519 key. Each session runs in its own process group and is killed whole at 90 s.
- **Load:** the same mix as B0-24a, so the two compare directly. Two workers each run one session every 6 s, rotating clone, fetch and push. That is about 20 sessions a minute and about 28,800 a day.
  - A push commits a new file on `main`'s tip and pushes it to `refs/heads/b024a/<n>`. `main` never moves, so clone and fetch keep reading the same tip.
- **A session succeeds** when git exits 0. For a push, the remote's report must also name the ref as updated, and the push then counts as **acknowledged**.
- **Writes are checked against the log, not the refs:**
  - An acknowledgement checker tails the repository leader's `/log` every minute (the test route `tests/am1/hosted/lib.mjs` `logFrom` reads). Each entry's `refs` list gives `[name, new]`.
  - Every acknowledged (ref, commit) must appear in a committed entry. A push the client saw fail is looked up there too and reported, as B0-24a found five.
  - A deleter removes verified branches over HTTPS in batches of 50 every 5 minutes. That keeps the ref count near 100, so clone and `ls-refs` times, and with them d, do not grow over the day. Its pushes are not SSH sessions and do not count.
- **Kills:** one every 10 minutes, round robin over sites 0, 1 and 2, 144 in the day. Each kill row records `at`, `open`, the restart time, the first success after the kill on any front, and the orb's clock offset.
- **Readings, fixed before the run:**
  - sessions and success rate, by operation;
  - failures in three classes: in flight on the killed front (the `open` counts), started inside the 2 minutes after a kill, and the background outside every kill window, with the background's own rate;
  - the measured mean session time d, and the d / 1,800 s floor computed from it;
  - acknowledged pushes found in the log (it must be all of them), and failed pushes found anyway;
  - p50, p99 and max session time by operation, and restart and first-success times by kill.
- **Orb pauses:** time the orb was paused is left out of every count and reported. After a pause the next kill is one period away, not a burst of missed ones.

### BF-52's revocation half in the same row

It fits. It runs once, on its own repository, before the 24-hour load starts, so it does not touch the session count. It has the same shape as BF-15 and BF-27 hosted (`tests/m1/results/m1-wp7.md`, BF-15):
1. A key is added. A real SSH push with it is accepted, which is the control.
2. A new bin in `tests/m3/hosted/tools` builds 1,000 format-32.26 pushes. Each has its own request id, its own `refs/heads/q<i>` and the key's fingerprint principal, encoded with `graftyard_git`'s own encoder (`Push::to_wire`).
3. The tools edge's `/commit_many` sends them to the repository's leader with `hold_deps_ms` 8,000, so the first entry holds its dependency read and the other 999 queue behind it.
4. Two seconds into the hold, `DELETE /user/keys/{id}` removes the key.
5. **Pass:** each of the 1,000 outcomes is 401 and no ref moved. A real SSH push and clone with the key then fail with "Permission denied (publickey)".

The queued commands go through the test route, not 1,000 SSH sessions at once. The credential they carry is the SSH key's, and the leader's re-read of the key's `Tokens` row is what refuses them. That is the claim BF-52 makes. One thing to check in the smoke step: whether the tools edge's `/commit_many` finds a repository chain's leader as it found the `Authz` leader for BF-15. If it does not, the bin sends to the leader that `/leader` names.

### Order of work

1. At $0 after the GO: the kill route, the open count, the tools-edge route, the driver, the BF-52 bin, their tests and plants. These go out on one branch for review and merge.
2. **Site day and edge day** on `graft-m3f` from merged main, as `tests/am1/results/install-m3f.md`, "Site day", gives them:
   - with `--test-faults` and `--ssh`, since `ssh.graftyard.dev` depends on this install;
   - with `SHARD_INSTANCE=standard-1` for the row (see the cost below);
   - followed by the tools edge from main.
   This site day also puts main's `idle-stall` (`decisions/DECISIONS.md`) on m3f.
3. **Smoke, about 15 minutes:** 50 sessions, one kill on each site, and a check that `open`, the restart and the log tail all read.
4. BF-52's revocation half, about 10 minutes.
5. The 24-hour load.
6. The report into this file, then teardown.

### Teardown

1. Stop the driver service.
2. `DELETE /user/keys/{id}` for the row's keys.
3. Retire and purge the row's repositories with `tests/fanout/retirenets.mjs` and `purge.py`.
4. Stop shards with `INSTALL=graft-m3f python3 tests/fanout/teardown.py --sweep`. It must name no running shard.
5. Kill each `SshFront` once more with the row's route. Each front stays stopped until `ssh.graftyard.dev`'s next session, so no front bills past the row.
6. The next m3f site day goes back to the install's own `--vars` (`standard-4`).
7. Remove the copied install state (`~/.graft-m3f`) and the tools-token link.
8. Add the row's line to `tests/am1/results/install-m3f.md`.

The Spectrum app, the host-key slots and the Secrets Store are not touched.

### Cost estimate and stops

The prices are the bill's overage rates (`research/cloudflare.md`, Containers and Durable Objects). The day is 86,400 s.

| Item | Basis | Estimate |
| --- | --- | --- |
| Three `SshFront` containers, `basic` (1 GiB, 4 GB disk), up all day | memory 3 × 1 × 86,400 × $0.0000025 = $0.65; disk 3 × 4 × 86,400 × $0.00000007 = $0.07; CPU about 0.1 vCPU-s a session [guess], 28,800 × 0.1 × $0.00002 = $0.06 | $0.78 |
| The network's git cache shard, 1 or 2 containers (warm on two sites) at `standard-1` (4 GiB, 8 GB) | memory 4 × 86,400 × $0.0000025 = $0.86; disk 8 × 86,400 × $0.00000007 = $0.05, each; push CPU 9,600 × 0.2 vCPU-s (DESIGN's push receive) = $0.04 | $0.95 to $1.85 |
| Durable Object rows written | about 10,000 entries × 3 voters × 20 rows [guess] | $0.60 |
| Durable Object duration and requests | members and fronts active about half the day [guess]; about 30 calls a session | $0.75 |
| Workers (edge, front, tools edge) | about 1 M requests and their CPU | $0.40 |
| Spectrum data | about 1.5 GB, inside Pro's 5 GB a month [unverified for Worker apps, K1] | $0 |
| Site day, edge day, smoke, BF-52's half | builds, about 15 minutes of sessions, 1,000 queued commands | $0.15 |
| **Row total** | | **about $3.6 to $4.5** |

At the install's own `standard-4` the shard line is $2.71 per container-day, and the row comes to **about $5.4 to $8.1**, close to the $10 cap. That is why the plan asks for `standard-1` on the row's site day. The shard's size does not bear on SSH reliability for a small repository [inference]. graft-m3f's idle spend was $1.50 on 2026-11-25 (`acctcost.py`, `prefix_usd`). The row's reading counts whatever idle spend falls in its window.

**Stops**, read with `PREFIX=graft-m3f python3 tests/fanout/acctcost.py`:
- every 10 minutes by the driver;
- every hour by the thread's follow schedule.

| Stop | Limit | What happens |
| --- | --- | --- |
| The row, from the site day's start | $10 (`FRAME.md`, the standing queue's per-row cap) | The load stops |
| graft-m3f per UTC day | $40 | The load stops; the row spans two UTC days, and each is checked |
| The account per day | $200 | The load stops |

A row stopped early is reported as stopped, with its hours and readings so far, not as a pass or a fail.

### Window

**Proposed: the site day at 2026-11-30 01:00Z, load from about 01:45Z to 2026-12-01 01:45Z.** This does not overlap the BM-6 drill on `graft-m6a` (00:05Z 2026-11-27 to 2026-11-29).
- **Why not overlap:**
  - BM-6 passes only if "no script version is created". Its checks read `graft-m6a`'s scripts. Still, a site day and the tools edge on `graft-m3f` would create versions in the same account during the drill.
  - The two rows would also share the account's day: the drill at most $20 a day, this row at most $10.
  - Neither would breach a cap, but keeping them apart keeps the drill's readings clean.
- **If the lead wants it sooner:** the row can run inside the drill. Separate installs keep separate caps, and the account takes at most $30 a day from the two of them. The only change is that the drill's "no script version" line must then be read for `graft-m6a` alone.
