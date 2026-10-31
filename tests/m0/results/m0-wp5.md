# M0-WP5: account-separated link (2026-10-29)

`src/link.ts` runs as three throwaway scripts, `graft-m0-link-a/b/c`, each with one SQLite-backed `Member` class placed by hint on enam, weur, or apac, and a public workers.dev hostname. They are separate from `graft-raft-*` and `graft-wp4-*`. `run/link.ts deploy` creates them, `run/link.ts run` measures, and `run/link.ts delete` removes them.

From a member on one site, the runner times the same member call (a ping with 1 KB or 16 KB that writes nothing) to a member on another site, alternating two ways:

- **Binding.** The cross-script Durable Object binding with `script_name`, which is what members use by default (`SiteLink::Binding`).
- **Signed HTTPS** (`SiteLink::Https`). The caller signs `{from, at, peer, body}` with its site's Ed25519 key and POSTs it to the other site Worker's public hostname. That Worker verifies the signature against the caller site's public key, refuses a timestamp more than 30 s off, and calls its own member over its binding. The scripts run with `global_fetch_strictly_public`, so the fetch leaves through the front door as a call from another account would. The hop leaving the account's runtime the same way a cross-account call does is [inference], as the build plan states.

| Row | Result | Gate | Verdict |
| --- | --- | --- | --- |
| B0-17 | 18 ordered site pairs over three runs, 50 calls each way per pair. Signed HTTPS minus binding at p50: 4 to 16 ms in 17 pairs, median 7 ms; one pair (apac to weur, 16 KB) was 37 ms faster over HTTPS because the binding took a longer route that run. At p99 HTTPS added 2 to 51 ms. Signing took under 1 ms (`results/raw/b0-17-1k-1.json`, `-16k-1.json`, `-1k-2.json`) | p50 overhead at most 20 ms; fail over 60 ms | **Pass**, three runs |

The overhead is the site Worker request and the public proxy on the receiving side; the caller's own signing is negligible. The p99 gap (up to 51 ms at apac) is what a commit round would add per hop at its tail, on top of B0-01's measured p99, if an install chose this option.

Not measured here: the link under a December 2025 class proxy failure on the ring hostnames, which the design says stalls commits for its duration, and a real second account, which the Milestone 0 account cannot provide.
