# Milestone 0 harness (throwaway, never shipped)

Prototype code for `build/BUILD-PLAN.md`, Milestone 0. Today it covers M0-WP1 to M0-WP6 and M0-WP11.

- `src/site.ts`: one site script source, deployed as `graft-site-a` (enam), `graft-site-b` (weur), and `graft-site-c` (apac). Each one defines three SQLite-backed member classes: `Member` for B0-12, and `IdleMember` and `ControlMember` for B0-15. It binds the other two sites' classes with `script_name`. A member keeps its own `Rtt` rows, and it rechecks them only on the first request it serves in a new period (`RTT_RECHECK_MS`).
- `src/edge.ts`: `graft-m0-edge`, the only public entry. It binds every member class and needs a bearer token.
- `src/placement.ts`: a prototype of `Placement::new` with `MIN_PAIR_RTT`.
- `src/probe.ts`: `graft-m0-probe`, which times R2 reads from Durable Objects in each region for B0-13. It is a separate script, so B0-13 never creates a site-script version.
- `src/container.ts` and `b0-16/`: `graft-m0-container`, a git image under the `durable_object` scheduling policy, used for B0-16 and B0-38.
- `src/tools.ts`, `src/faults.ts`, and `porcupine/`: `graft-m0-tools`, the M0-WP1 test tools, in their own script so they never create a site-script version. `FaultControl` holds the fault plan that members read through `src/faults.ts`: member calls that fail, hang, or lose their reply, and storage writes that reset the object. `LoadClient` is a load client pinned to one colo, because an object never moves once created, and it records every operation for the Porcupine checker in `porcupine/` (Go, one register per key). `Register` and `StaleRegister` are self-test targets. `QuorumProbe` and `RouteProbe` answer M0-WP11 (b) and (a).
- `src/raft/`: the M0-WP2 Raft prototype, deployed by `run/raft.ts deploy` as `graft-raft-a`, `graft-raft-b`, `graft-raft-c`, and `graft-raft-edge`, separate from the `graft-site-*` bed. `src/raft/local.ts` runs all three members in one `wrangler dev` for local runs (`RAFT_URL`, `RAFT_TOKEN`). Runners: `run/raft-bench.ts` (B0-01, B0-02; `--placement na3` for `Na3` hints), `run/raft-rto.ts` (B0-03), `run/raft-chaos.ts` (B0-11; `--remote` drives load from the pinned tools clients through their `RAFT` service binding, and `--remote-chaos` runs the resets and transfers from `graft-m0-chaos`, `src/chaos.ts` and `chaos/wrangler.jsonc`, an alarm-driven object, so neither stops when the orb sleeps; a `--resume` joins a remote chaos driver already running), `run/raft-partial.ts` (B0-25), and `run/raft-storm.ts` (B0-26). Results are in `results/m0-wp2.md`. One edge invocation runs at most 6 member calls at once and queues the rest, so every runner keeps 5 workers per `/bench` call.
- M0-WP4: `RAFT_PREFIX=graft-wp4 node run/raft.ts deploy` deploys the same source as `graft-wp4-a/b/c` and `graft-wp4-edge`, with `SiteHealth` (`src/raft/health.ts`), the release variables, and its own fault plan, so it never resets B0-11's 48-hour run on `graft-raft-*`. `run/site-day.ts` (`setup`, `day`, `guard`, `reset`) runs site days with `wrangler versions upload` and `versions deploy`. Results are in `results/m0-wp4.md`.
- M0-WP5: `src/link.ts`, deployed by `run/link.ts deploy` as `graft-m0-link-a/b/c`. Results are in `results/m0-wp5.md`.
- M0-WP6: `src/rehydrate.ts` and `b0-22/`, deployed with `npx wrangler deploy -c b0-22/wrangler.jsonc` as `graft-m0-rehydrate`, with its own bucket `graft-m0-rehydrate` and container class `Shard` (git and the Go server `b0-22/sink/main.go`). One script holds B0-22's rehydration (`op: seed`, then a POST with `net`, `hint`, `name`, `parallel`), B0-23's clone cache (`run/clone-cache.ts mono`, `run`, `collect`), and the B0-20 and B0-21 push probes (`/push/<cap MB>/r.git`, `/put`). `run/working-set.py` is B0-14. Results are in `results/m0-wp6.md`.
- M0-WP7: `src/wp7/ssh.ts` and `wp7/`, deployed by `run/wp7.ts deploy` as `graft-m0-ssh`. Three `SshFront` containers (enam, weur, apac hints) run OpenSSH and a cloudflared replica of the tunnel `graft-m0-ssh`. Their outbound handler `SshOutbound` serves `keys.graft`, `tokens.graft`, and `group.graft`. `run/wp7.ts spectrum` records the Spectrum API's answer on `graftyard.dev`, and `load --run X` and `report --run X` run B0-24 over the Tunnel. Results are in `results/m0-wp7.md`.
- `gen-config.ts` and `deploy.sh`: the first pass deploys each site's classes alone, and a second pass adds the cyclic bindings. **Do not redeploy the site scripts while B0-15 runs**, because a new version resets their members.

| Row | Runner | Result |
| --- | --- | --- |
| B0-12 | `run/b0-12.ts --groups 1000 --intra 50` | `results/b0-12.json` |
| B0-13 | `run/b0-13.ts`, then `run/b0-13-edges.ts` | `results/b0-13.json`, `results/b0-13-edges.json` |
| B0-15 | `run/b0-15.ts setup`, then `check` during the window and `final` after it | `results/b0-15-<run>.json`; estimate in `results/b0-15-estimate.md` |
| B0-16 | `wrangler deploy -c b0-16/wrangler.jsonc` with a token holding only Workers Scripts Write and Workers Containers Write | `results/b0-16.md` |
| Tools | `run/tools.ts deploy`, then `run/tools.ts selftest` | `results/tools-selftest.json`: clients pinned in ATL, LHR, and SIN; the checker accepts a correct register with and without faults and rejects a stale one |
| M0-WP11 | `run/wp11-quorum.ts`; `/route` and `/hop` on `graft-m0-tools` | `results/wp11.md`, `results/wp11-quorum.json` |
| B0-38 | `run/container-placement.ts`; then `?op=keep`, a redeploy with a changed image, and `?op=peek` | `results/container-placement.json`, `results/container-survival.json` |
| B0-14 | `python3 run/working-set.py sizes`, `events 2026-07-14 2026-07-15`, `report 2026-07-14 2026-07-15` | `results/m0-wp6.md`, `results/raw/b0-14.json` |
| B0-19 | `run/container-placement.ts --per 34 --no-summary`, with the account API token active, then disabled | `results/m0-wp6.md` |
| B0-22 | seed, then rehydrate on new object names | `results/m0-wp6.md` |
| B0-23 | `run/clone-cache.ts mono`, `run --run X [--lag --lag-max --prefill --claim]`, `collect --run X` | `results/m0-wp6.md` |
| M0-WP2, M0-WP3 | `run/raft.ts deploy`, then `run/raft-bench.ts`, `run/raft-rto.ts`, `run/raft-chaos.ts`, `run/raft-partial.ts`, `run/raft-storm.ts`, `run/raft-gray.ts` (B0-06), and `run/raft-learners.ts` (B0-18) | `results/m0-wp2.md` |

Raw rows go to `results/raw/`, which is not committed.

A member's region is judged by the colo it runs in, mapped to a continent through the Cloudflare status page. That separates `Tri`'s three hints but cannot tell enam from wnam.

## Teardown

After B0-15's `final`, delete the B0-15 groups by deleting the `IdleMember` and `ControlMember` classes in a site-script migration. Run `node run/b0-13.ts --delete` to remove the 120 test buckets and the probe script. After M0-WP2, delete `graft-m0-tools`, `graft-raft-edge`, and `graft-raft-a`, `-b`, and `-c` with `npx wrangler delete <name>`, and after M0-WP4 the same four `graft-wp4-*` scripts. After M0-WP5, `node run/link.ts delete`. After B0-11, delete `graft-m0-chaos`. After M0-WP6, delete `graft-m0-rehydrate` and empty and delete its bucket. After M0-WP7, `node run/wp7.ts delete` removes `graft-m0-ssh`, the tunnel, its route and virtual network, and the `ssh-m0.graftyard.dev` record.
