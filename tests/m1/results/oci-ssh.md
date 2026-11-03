# Plain SSH through an Oracle Always Free proxy (2026-11-03)

The Pro zone has no Spectrum, so a git client cannot reach the Tunnel on port 22 without `cloudflared` on its own machine (`tests/m0/results/m0-wp7.md`). This adds one Oracle Cloud VM that takes plain SSH on public port 22 and carries each connection into the same Tunnel hostname, `ssh-m0.graftyard.dev`, in front of the `graft-m0-ssh` bed. The operator allowed it as an exception to Cloudflare-only until go-live.

## Verdict

| Check | Result |
| --- | --- |
| `git clone`, `fetch`, and `push` with plain `ssh`, no `ProxyCommand`, to `git@ssh.graftyard.dev:acme/app.git` | 249 of 249 sessions succeeded over 24.9 minutes. The `cloudflared access ssh` path in the same window also had 249 of 249 |
| Latency next to the `cloudflared` path | No measurable difference. The p50s are within 90 ms of each other, and the proxy's is lower for every operation |
| Pushes | All 166 acknowledged pushes in the run (83 per path) carry a `seq`, and every `seq` is in the bed's group log |
| Forwarder restart mid-run | Every session open at the stop was cut, and new connections worked again within about 0.4 s. See "Forwarder restart" |
| Admin port refuses passwords | sshd offers only `publickey`. A password-only client gets `Permission denied (publickey)` |
| Everything created is Always Free | Yes, by the doc and by the API's own billing label. See "Always Free evidence" |

## The account

- **Tenancy** `<oci-tenancy>`. The home region is `us-phoenix-1` (PHX), which is the tenancy's only subscribed region.
- **Type: Free Trial.** `oci organizations subscription list` shows a single `ACTIVE` subscription with `payment-model: FREE_TRIAL`. It runs from 2026-11-02 to 2026-12-01T23:59:59Z. After that date the account becomes Always Free only unless someone upgrades it ([What Happens When the Promotion Expires](https://docs.oracle.com/en-us/iaas/Content/GSG/Tasks/signingup_topic-What_Happens_When_the_Promotion_Expires.htm)).
- In PHX-AD-1, `oci compute shape list` labels `VM.Standard.E2.1.Micro` `billing-type: ALWAYS_FREE`, and `VM.Standard.A1.Flex` `LIMITED_FREE` in all three ADs. The Micro shape is offered only in AD-1 (`vm-standard-e2-1-micro-count` is 2 in AD-1 and 0 in AD-2 and AD-3).

## Idle reclamation

The rule, from [Always Free Resources](https://docs.oracle.com/iaas/Content/FreeTier/freetier_topic-Always_Free_Resources.htm), "Idle Compute Instances" (updated 2026-06-12): "Idle Always Free compute instances may be reclaimed by Oracle. Oracle will deem virtual machine and bare metal compute instances as idle if, during a 7-day period, the following are true: CPU utilization for the 95th percentile is less than 20%; Network utilization is less than 20%; Memory utilization is less than 20% (applies to A1 shapes only)."

**Does it apply here?**
- **This VM meets the idle test whenever git traffic is light [inference].** It is an E2.1.Micro, so only the CPU and network conditions count. The hour that held this whole 25-minute load run averaged 8.5% CPU (`oci_computeagent` `CpuUtilization`, the 01:00 UTC hourly mean). With no load, CPU sits near zero. Network utilization is measured against the shape's bandwidth, and that hour moved 5.1 MB.
- **Which accounts it targets.** The doc does not name an account type. Oracle's 2023 notice, quoted on Oracle's community forum and not in the docs, said it reclaims idle compute "from Always Free customers only", and that upgrading the account prevents it (https://community.oracle.com/customerconnect/discussion/672339; https://community.oracle.com/customerconnect/discussion/663398, [unverified]). The same notice says a reclaimed instance is stopped and can be restarted if the shape has capacity.
- **Reading.** While the trial lasts (until 2026-12-01), the account is a Free Trial account. The doc gives no exemption for it, and the notice's wording suggests reclamation targets Always-Free-only accounts [inference]. After the trial ends without an upgrade, the account is Always Free only. The rule then applies, and a quiet week would mark this VM idle. Upgrading to Pay As You Go keeps Always Free resources at $0 ("Oracle doesn't charge for Always Free resources after you upgrade", same doc) and, by the notice, removes reclamation.
- No artificial load was added.

A separate rule from the [Free Tier FAQ](https://www.oracle.com/cloud/free/faq/): "Accounts left idle for 30 days or more may be deemed abandoned and become eligible for suspension or termination."

## What was built

```
git client ──ssh :22──▶ <ssh-proxy-ip> (ssh.graftyard.dev, DNS only)
                          graft-ssh-proxy.service: cloudflared access tcp --hostname ssh-m0.graftyard.dev --url 0.0.0.0:22
                          one WebSocket stream to the Cloudflare edge per accepted TCP connection
                        ──▶ Tunnel graft-m0-ssh ──▶ a front's cloudflared ──▶ sshd on the front's loopback (the M0 bed)
admin ──ssh :2222 (key only)──▶ the VM's own sshd
```

**The forwarder.** `cloudflared access tcp` with `--url` is a TCP listener. For each connection it accepts, it opens a stream to the Tunnel hostname and copies bytes both ways. It does not terminate SSH. The client's SSH session therefore runs end to end with the bed's sshd, so the client checks the bed's host key (`SHA256:GCDfbpG3…`) and the VM never sees git credentials. That is why the known-hosts entry for `ssh.graftyard.dev` is the bed's key, not the VM's. It is the client half of the M0 path, `cloudflared access ssh`, moved from each client to one server. A plain `socat` or `haproxy` relay cannot do this job, because the Tunnel hostname speaks WebSocket at the edge and not raw TCP, and `cloudflared` is the client for that.
- systemd unit `graft-ssh-proxy.service` runs with `Restart=always`, `RestartSec=1`, no start-rate limit, `DynamicUser`, only `CAP_NET_BIND_SERVICE`, and `ProtectSystem=strict`. It used 21 MB of memory at peak and 3.3 s of CPU over 13 minutes of load (journal).
- Admin SSH is the VM's OpenSSH on port 2222. `sshd -T` shows `passwordauthentication no`, `kbdinteractiveauthentication no`, `authenticationmethods publickey`, and `permitrootlogin no`. Ubuntu 24.04 starts sshd through `ssh.socket`, so the script disables the socket and runs `ssh.service` on 2222. That frees port 22 for the forwarder.
- **Automatic updates.** `unattended-upgrades` with `APT::Periodic::Unattended-Upgrade "1"` covers Ubuntu's `noble-security` and ESM pockets plus the `cloudflared` apt repository. When an update needs a reboot, it reboots at 10:00 UTC. A dry run listed the allowed origins and one pending package.
- **Cloudflare Access.** The tunnel hostname is not behind Access. Access is not enabled on the account: every `/access/*` API call returns `access.api.error.not_enabled`. So no service token was made and nothing was stored for one. See "Decisions for the operator".

### Files

- `deploy/oci-ssh/provision.sh <admin pubkey>`: finds or creates the VCN, internet gateway, subnet, route, security list, instance, and DNS record by display name, then prints their ids. A second run created nothing and printed the same ids (checked with a diff).
- `deploy/oci-ssh/cloud-init.sh`: the VM's user-data, which can also be rerun over admin SSH (`ssh -p 2222 ubuntu@<ip> sudo bash -s < deploy/oci-ssh/cloud-init.sh`). It installs cloudflared from Cloudflare's signed apt repository, sets up unattended upgrades, moves sshd to 2222 with keys only, opens 2222 in Oracle's image iptables, and installs and starts the forwarder. It exits non-zero unless cloudflared ends up listening on port 22. It ran twice on the live VM with exit 0 both times. A fresh VM booted from it alone reached `cloud-init status: done`, served a `git clone`, and was then terminated.
- `deploy/oci-ssh/teardown.sh`: deletes what `provision.sh` made. Syntax-checked only, **not run**, because the proxy stays deployed.
- `tests/m1/oci-ssh/load.ts`: the load runner and its report.

Two first-boot bugs were found by booting a throwaway VM from the script, and both are fixed in it:
1. `ssh.service` has `KillMode=process`, so the listener the socket had started survived the switch and kept port 22. The forwarder then crash-looped on `bind: address already in use`. The script now stops both units and kills any sshd still on 22.
2. On first boot `/run/sshd` does not exist yet, and `sshd -t` fails with "Missing privilege separation directory". Under `set -e` the script stopped there. The script now creates the directory first.

## Resources and Always Free evidence

Everything is in the root compartment of `us-phoenix-1`, tagged `project=graftyard`, and listed in `results/raw/oci-ssh-20261103/resources.json`.

| Resource | OCID or id | Why it is free |
| --- | --- | --- |
| Instance `graft-ssh-proxy`, `VM.Standard.E2.1.Micro` (1/8 OCPU with burst, 1 GB, PHX-AD-1), image `Canonical-Ubuntu-24.04-2026.10.27-0` | `<ocid>` | "All tenancies get up to two Always Free VM instances using the VM.Standard.E2.1.Micro shape", and Ubuntu is in its list of Always Free images ([Always Free Resources](https://docs.oracle.com/iaas/Content/FreeTier/freetier_topic-Always_Free_Resources.htm), Compute). The API labels the shape `ALWAYS_FREE` in AD-1. Used 1 of 2 (`resource-availability`) |
| Boot volume, 50 GB, 10 VPUs/GB | `<ocid>` | "200 GB total of combined boot volume and block volume" in the home region; 50 GB is the default and the minimum (same doc, Block Volume). `total-free-storage-gb` reads 50 used, 150 available |
| VCN `graft-ssh-proxy-vcn`, 10.0.0.0/16 | `<ocid>` | "Free Tier tenancies … can have up to 2 virtual cloud networks" (same doc, Networking). The [price list](https://www.oracle.com/cloud/networking/pricing/) has no line for VCNs, subnets, gateways, route tables, or security lists, and inbound data is free |
| Subnet `graft-ssh-proxy-subnet`, 10.0.0.0/24, public | `<ocid>` | As the VCN |
| Internet gateway `graft-ssh-proxy-igw` | `<ocid>` | As the VCN |
| The VCN's default route table (0.0.0.0/0 to the gateway) and default security list (TCP 22 and 2222 from anywhere, ICMP 3/4, all egress) | `<ocid>`, `<ocid>` | Created with the VCN |
| VNIC and **ephemeral** public IP <ssh-proxy-ip> | `<ocid>`, `<ocid>` | The Micro shape "Includes one VNIC with one public IP address and up to 50 Mbps network bandwidth via the internet" (same doc). Outbound data is free up to 10 TB a month in North America (price list). No reserved IP was made |
| No network load balancer, no A1 instance, no block volume, no reserved IP | | Not needed |
| Cloudflare DNS `ssh.graftyard.dev` A <ssh-proxy-ip>, DNS only, TTL 300, comment `graft-ssh-proxy (deploy/oci-ssh)` | `2b40ad7dd9ac58c736cf57b92c9dd879` | DNS records cost nothing on the zone's plan. `m0.graftyard.dev` and `ssh-m0.graftyard.dev` were not changed (their `modified_on` is still 2026-10-30) |

Made and terminated during the boot test, with no boot volume kept: `graft-ssh-proxy-bootcheck` instances `<ocid>` (the boot that exposed bug 2) and `<ocid>` (the clean boot). Each was a second Micro, which is within the limit of two, with a 50 GB boot volume inside the 200 GB.

**Why the Micro.** The forwarder needs about 21 MB of memory and a few seconds of CPU per hour of load. The Micro is the only shape the API labels `ALWAYS_FREE`. A1 is `LIMITED_FREE`, metered against 1,500 OCPU-hours a month. The doc now puts the A1 allowance at "2 OCPUs and 12 GB" for Always Free tenancies, not the older 4 and 24. The Micro's limit is 50 Mbps to the internet, so a 100 MB push takes at least 16 s through it [inference from the doc's figure].

**Billing check.** `oci usage-api usage-summary request-summarized-usages` for 2026-11-02 to 2026-11-04 returned no cost lines. Usage data can lag by up to a day, so this is weak evidence and needs a recheck tomorrow.

## Measurements (run `r1`, 2026-11-03 00:54:33 to 01:19:25 UTC, 24.9 minutes)

**Setup.** `node tests/m1/oci-ssh/load.ts load --run r1 --minutes 25 --restart-at 12.5` ran as the orb service `oci-ssh-load`. The orb is in LAX and is the only client. Two workers ran at the same time, one per path. Each worker ran one session every 6 s, rotating clone, fetch, and push. Each push wrote a new branch. The success rule is the M0 runner's: git exits 0, the front names itself, and a push reports the group's `seq`. The proxy path's ssh config is the M0 stanza without the `ProxyCommand` line, and it keeps `ConnectTimeout 30` and `ServerAliveInterval 15`. The control path is `tests/m0/wp7/ssh_config` unchanged. This run had no container kills. The bed's three fronts had been stopped since B0-24a, were started for this test, and were stopped again afterwards.

| Path | Operation | Sessions | Succeeded | p50 | p99 | Max |
| --- | --- | --- | --- | --- | --- | --- |
| Proxy, plain ssh | clone | 83 | 83 | 2.75 s | 3.65 s | 3.65 s |
| Proxy, plain ssh | fetch | 83 | 83 | 2.57 s | 3.60 s | 3.60 s |
| Proxy, plain ssh | push | 83 | 83 | 2.97 s | 3.85 s | 3.85 s |
| `cloudflared access ssh`, same window | clone | 83 | 83 | 2.84 s | 3.41 s | 3.41 s |
| `cloudflared access ssh`, same window | fetch | 83 | 83 | 2.65 s | 3.50 s | 3.50 s |
| `cloudflared access ssh`, same window | push | 83 | 83 | 3.03 s | 4.72 s | 4.72 s |
| M0 B0-24a `r4`, 24 h with kills (`m0-wp7.md`) | clone | 9,449 | 9,404 | 2.65 s | 3.49 s | 19.4 s |
| M0 B0-24a `r4` | fetch | 9,449 | 9,408 | 2.59 s | 3.41 s | 18.4 s |
| M0 B0-24a `r4` | push | 9,449 | 9,418 | 2.96 s | 3.89 s | 17.5 s |

- **Success.** 249 of 249 on each path. M0's rate without kills was 99.859%, about 1 failure in 700. At that rate, 249 sessions would be expected to produce 0.35 failures, so zero failures cannot tell this path apart from M0's floor. Telling 99.9% from 99.8% takes about a day of sessions, as `m0-wp7.md` notes. The proxy adds a hop of its own, with its own edge connection from PHX, so it can only add failure causes to the Tunnel path. It cannot remove the edge stalls M0 measured [inference].
- **Latency.** The proxy path is no slower: its p50 is 55 to 90 ms below the control's for every operation, which is within run-to-run noise. Both paths were routed to `front-b` in CDG for about 92% of sessions (proxy 228 of 249, control 229 of 249), as in M0's `r4`. The limiter is the same on both paths: the SSH handshake and git's round trips between the US and the replica in Paris. That is why both match `r4`, and why M0's 2.2-hour run, routed to ORD, was about 1 s faster [inference from the routing counts]. The extra LAX-to-PHX hop costs less than the noise.
- **Pushes.** All 166 acknowledged pushes in the run have a `seq` that is in the group log (10,869 entries, saved in `raw/…/r1/group-log.jsonl`).
- **Not counted.** The service restarted once after the run ended, at 01:19:32, before it was stopped. That second run's 18 sessions, all successful, are kept apart in `sessions-after-end.jsonl` and left out of every figure above.

### Forwarder restart

At 12.5 minutes the runner opened an SSH session through the proxy and held it after upload-pack's ref advertisement. It started a clone 1 s before the restart, then ran `systemctl restart graft-ssh-proxy` over admin SSH while it probed `git ls-remote` every 0.5 s. The times below are on the VM's clock, taken from the journal (`raw/…/r1/vm-restart-journal.txt`):

| VM time (UTC) | Event |
| --- | --- |
| 01:07:04.561 | systemd stops the forwarder |
| 01:07:04.575 | systemd starts the new process |
| 01:07:04.879 | cloudflared logs `Start Websocket listener host=0.0.0.0:22` |

So no process listened on port 22 for about 0.32 s. Clients saw this:
- **Every session open at the stop was cut.** The held session ended with `Connection to ssh.graftyard.dev closed by remote host`, and the five `ls-remote` probes that were mid-handshake or mid-advertisement ended with `Connection closed by <ssh-proxy-ip> port 22` (3) or `Connection reset` (2). The forwarder has no drain: its streams die with the process, as a container kill cuts a Tunnel stream in M0.
- **New connections recovered at once.** The first probe started after the stop succeeded, and so did the 34 probes after it. In the smoke run before `r1`, one probe landed inside the gap and got `Connection refused` in 58 ms.
- **The steady load saw nothing.** Neither load worker had a session in flight at the stop, and all their sessions succeeded. The clone started 1 s before the restart had finished, 2.7 s later, before the stop.
- `NRestarts=0`, because this was an operator restart and not a crash restart.

### Admin port

- `ssh -p 2222 -o PubkeyAuthentication=no -o PreferredAuthentications=password,keyboard-interactive ubuntu@<ssh-proxy-ip>` fails with `Permission denied (publickey)`, and `ssh -v` shows `Authentications that can continue: publickey`.
- The admin key from 1Password logs in. The key generated first was removed from `authorized_keys`, and a login with it fails.

## 1Password

- Vault `Cinematic`, item **`graftyard-ssh-proxy`**, category SSH Key, ed25519, tagged `graftyard`. Its notes hold the public IP, the DNS name, the region and AD, the instance OCID, and the admin port.
- The 1Password CLI cannot write notes on an SSH Key item. It ignores the notes field on create, `--template` is refused for the category, and `op item edit` answers "SSH Key item editing in the CLI is not yet supported". The notes were therefore written with the official 1Password JS SDK (`@1password/sdk` 0.5.0, `items.put`) under the same service account. A read back confirmed the key was unchanged.
- Three interim `graftyard-ssh-proxy` items made during that work were deleted, and only the final item remains. Two were template attempts with no key, and one held the first generated key, which never stayed on the VM.
- The private key is also kept at `~/.ssh/graftyard-ssh-proxy`, mode 600, in this orb only, because the runner's restart step uses it. The OCI API key and config are at `~/.oci/`, mode 600. Nothing secret is in the repository or in `raw/`.

## Failure modes

These rows are for the design editor, because this VM is a new failure domain (outside dependency: Oracle Cloud). It sits in front of the M0 Tunnel path and inherits all of that path's failure modes in `m0-wp7.md`.

| Class | What fails | Effect | Recovery |
| --- | --- | --- | --- |
| instance | Forwarder process exits or is restarted | Sessions in flight are cut; about 0.3 s with no listener [measured] | systemd restarts it after 1 s, with no start-rate limit |
| instance | The VM stops: host maintenance, a reboot after unattended upgrades (10:00 UTC), idle reclamation, or a crash | All SSH is down until the VM is back. There is one VM and no standby. A stop keeps the ephemeral IP | Start the instance; the units are enabled. Reclamation recovery depends on Micro capacity in AD-1 |
| instance | The VM is terminated or rebuilt | The ephemeral IP is lost, so `ssh.graftyard.dev` points at nothing | Rerun `provision.sh`, which updates the record; clients wait up to the 300 s TTL |
| data-center | PHX-AD-1 is lost | SSH is down. The Micro shape exists only in AD-1 (limit 0 in AD-2 and AD-3) | Wait for AD-1, or pay for a shape in another AD (operator decision) |
| region | `us-phoenix-1` is lost | SSH is down. Always Free compute exists only in the home region | Wait, or run a paid VM elsewhere (operator decision) |
| provider-wide | An Oracle Cloud outage, or the account is suspended (idle 30 days, the trial lapses into a special-offer suspension, or a policy action) | SSH is down; HTTPS git on Cloudflare is unaffected | Outside our control. This is the dependency the Cloudflare-only frame avoided |
| provider-wide | A Cloudflare Tunnel edge incident | As in M0: edge stalls about 1 in 700 sessions, or total loss | As in M0 |
| control-plane | The OCI API or console is down, or the Cloudflare DNS API is down | A running VM keeps serving, because no request path calls either. Rebuilding or moving the IP waits | Retry later |
| control-plane | 1Password is unavailable | The admin key cannot be fetched. Only this orb holds a copy | Wait; the console serial connection is the last resort |
| bad-deploy | A `cloud-init.sh` edit breaks sshd or the forwarder | The forwarder can go down, or admin access can be locked out. `sshd -t` runs before any restart, and the script fails unless port 22 is held by cloudflared. Two such bugs were found on first boot this session and fixed | Rerun the fixed script over 2222 if sshd lives; otherwise rebuild with `provision.sh` |
| bad-deploy | Unattended upgrades install a broken `cloudflared` or OpenSSH | The forwarder or admin access breaks the next day | Pin or roll back the package over 2222 |
| bad-config | The DNS record is proxied (orange cloud) or points at an old IP, `TUNNEL_HOSTNAME` is wrong, or the security list or iptables lose port 22 or 2222 | Clients get refused or time out, or reach the wrong place. With a wrong IP, clients refuse a mismatched host key (`StrictHostKeyChecking`) | `provision.sh` rewrites the record, the route, and the security list whole. `cloud-init.sh` rewrites the unit and the env file |
| noisy-tenant | The Micro shares a host. It has 1/8 OCPU with burst and 50 Mbps to the internet | Large packs are capped at 50 Mbps, and CPU steal adds latency. One client or a scanner flood can hold many Tunnel streams, which land on the bed's sshd; there is no rate limit on the VM | Move to a bigger or paid shape (operator decision), or add per-source limits (`iptables` connlimit) |

## Decisions for the operator

1. **Reclamation after 2026-12-01.** When the trial ends, the account is Always Free only. A quiet week (CPU p95 under 20% and network under 20%) makes this VM eligible to be stopped. The options: upgrade to Pay As You Go, which keeps it at $0 and, by Oracle's notice, ends reclamation; add synthetic load, which was not done here; or accept a stop and start it again.
2. **Cloudflare Access on `ssh-m0.graftyard.dev`.** Access is not enabled on the account, so anyone with `cloudflared` can reach the bed's sshd through the hostname. That is the same exposure as the proxy's public port 22. Enabling Zero Trust and putting a service token on the proxy is an account-level change that was not made. It would restrict the Tunnel hostname to the proxy and to clients holding tokens.
3. **Admin port 2222 is open to the internet,** key-only. It could be limited to known source ranges in the security list.
4. **Ephemeral IP.** If the instance is terminated, the IP changes. A reserved public IP would survive that, but it was not made, and its Always Free status was not checked.
5. **The bed is stopped.** The fronts were stopped after the test, as they were found. Until they are started, the proxy accepts connections and git fails. Running the three `basic` fronts all month would cost about $22 a month in Cloudflare Containers memory and disk past the included usage, plus CPU [inference from the Containers price list: 3 GiB × 730 h and 12 GB disk × 730 h, at the listed rates].
6. **Daily reboot window.** Unattended upgrades may reboot the VM at 10:00 UTC when an update needs it. That is about a minute of SSH downtime.

## Teardown

From the repository root, with the OCI CLI configured and `CLOUDFLARE_API_TOKEN` set:

```
deploy/oci-ssh/teardown.sh   # terminates graft-ssh-proxy and its boot volume, deletes the subnet, gateway, and VCN,
                             # and deletes ssh.graftyard.dev only if it carries provision.sh's comment
```

Then delete the 1Password item `graftyard-ssh-proxy`, and `~/.ssh/graftyard-ssh-proxy*` and `~/.oci/` in the orb. Teardown does not touch the M0 bed; `tests/m0/results/m0-wp7.md` has its own teardown.

## Raw data

`tests/m1/results/raw/oci-ssh-20261103/` (git-ignored): `resources.json` and `provision.log`; `r1/sessions.jsonl`, `r1/sessions-after-end.jsonl`, `r1/restart.jsonl`, `r1/restart-sessions.jsonl`, `r1/vm-restart-journal.txt`, `r1/group-log.jsonl`, `r1/summary.json`; and the runner's generated ssh configs and work clones.
