# Milestone 3's finish on graft-m1

This round works toward every M3-WP3 row Done hosted, on the graft-m1 install. Branch `m3-finish`, from `m3-rotate` (`74550b6`, on main `655fee3`). Everything ran on `graft-m1-*`. Nothing touched `graft-m2b-*` or `graft-m1-lin-*`.

## What was deployed

| Script | Version | What changed |
| --- | --- | --- |
| `graft-m1-edge` | `9339fb5b-aaa5-4d0e-b943-43e7ab44b446` | R20's `POST /api/graftyard/v1/release/tokens` and the test-faults `/_bed/precreate`, at `5938d1b`; was `8a824825`. Deployed 20:07:04Z |
| `graft-m1-site-a`, `-b`, `-c` | `7fe1aaa9`, `70748bab`, `438f715c` | Unchanged: no site code changed, so no site day |
| `graft-m1-ops` | `ca658d46` | Unchanged |

[measured, `tests/m3/raw/finish/edge1.log` and `wrangler deployments list`]

An edge version is not a site-script version, so it ships without a site day (AGENTS.md, "Invariants never to break").

## R20: install creates every `Tokens` shard. Done.

The operator's decision: install creates every `Tokens` shard ahead of time, and `Names` stays create-on-first-use.

### Why a first request took 13 s

The earlier guess was one `ELECTION_AFTER`. That guess was wrong. The cost is the members' `/join`.

- A `/create` with `campaign: false` on a fresh shard, so joins only, took 9.9 s and 9.9 s (`tokens:2994`, `tokens:2995`). The campaign after it took 1.2 s to a leader [measured].
- A join on a fresh member measures each peer: `RTT_SAMPLES` (20) sequential pings to each of the other two sites, in `workers/site/src/member.rs`, `join`. The rows read back were 120 ms (enam–weur) and 215 ms (enam–apac). The apac member's own pings run to two sites over 100 ms away, so 20 × (about 200 + about 270) ms is about 9.4 s [inference from the measured RTT rows].
- A full `/create` took 11.5 s to 13.1 s on four fresh shards, against 13.0 s median before [measured].

### What was built (`5938d1b`)

- `graftyard_identity::TOKEN_SHARDS` (3,072).
- `workers/edge/src/precreate.rs`, its own module, which keeps it apart from AM1-WP2's edge changes:
  - `POST /api/graftyard/v1/release/tokens?from=F&count=N` creates the groups of `Tokens(F .. F+N)`, at most 16 per request and two at once inside one invocation.
  - The body is the signed genesis, the same credential as the entries endpoint. The edge accepts it against its anchor and creates only if it is the genesis the `Release` group holds.
  - The answers are 200, 503 with each failed shard, and 422.
  - With test-faults, `/_bed/precreate` does the same under the `GRAFTYARD_GENESIS` stand-in's record, for a test install with no `Release` group.
- `graftyard install --genesis` calls `precreate_tokens` after the genesis commits: 192 ranges of 16, 8 requests at once. It reports failed ranges and tells the operator to run `install` again. `create` returns at once for a group that answers, so a rerun is idempotent.
- One invocation cannot create all 3,072, because of the six-connection and subrequest limits (`research/cloudflare.md`, Workers limits). That is why the CLI drives the ranges.
- Test: `install_creates_every_tokens_shard_and_reports_the_ranges_that_failed`, against a stub edge. Every shard is covered exactly once, the genesis bytes are the body, and a range answered 503 is reported.
- Planted bugs, both caught by that test:
  - ranges stepped by 15, so they overlap;
  - failed ranges dropped.

### Hosted (`eaff64b`)

- **Pre-create.** `tests/m3/hosted/precreate.mjs`, 12 ranges at once, from 20:08:32Z: 191 of 192 ranges answered 200, in 21 min. Range 784 had one shard, `tokens:791`, refused by all three members with "claim unanswered": the registry claim timed out at 5 s under load. Its rerun answered 200 in 14 s [measured, `tests/m3/raw/finish/precreate.jsonl`, `precreate-retry.txt`].
- **Check.** A second sweep found every group at once: 192 of 192 ranges answered 200 in 79 s, a median of 4.7 s per range of 16 probes. Every one of the 3,072 shards answers [measured, `precreate-verify.jsonl`].
- **Latency.** `tests/m3/hosted/device-latency.mjs`, 24 device flows as gh's client, approved as `m3-member` [measured, `tests/m3/raw/finish/device-latency.jsonl`]:

| | Before (`m3-hosted.md`) | After |
| --- | --- | --- |
| `POST /login/device/code`, median | 13.0 s (65 requests) | **0.83 s** (24) |
| `POST /login/device/code`, max | 14.4 s | 1.3 s |
| Token exchange, median | about 15 s | **1.9 s** (max 2.5 s) |
| `gho_` tokens issued | | 24 of 24 |

The edge's `commit_system` still calls `create`, whose probe costs one read, about 0.1 s to 1 s, on an existing group.

## `mint-key` kid 1 retire. Done.

Kid 1's retire-not-before time was 2026-11-07T21:36:36Z (`1794087396621`, `tests/m3/results/m3-close.md`).

- 22:33:58Z: `tokens:0` held both keys with `retired: null`, at index 34 [measured, `tests/m3/raw/finish/retire/before.txt`].
- `graftyard mint-key step --old-kid 1 --old-public 7ec36cd3… --new-kid 2 --new-published 1794076372120 --switched 1794080136621` answered "retire kid 1 at 1794087396621 ms". It wrote the `PutVerifyingKey { kid: 1, key: kid 1's public key, retired: 1794087396621 }` request [measured, `step.out`, `retire.request.json`].
- 22:34:26Z: the request committed as the operator on `tokens:0` at index 35 [measured, `commit.out`].
- After the commit, kid 1 reads `retired: 1794087396621` and kid 2 reads `retired: null` [measured, `after.txt`].

### Checks

No kid 1 token survives. The m3-close control token expired at 20:34:25Z, and its file was deleted. The `MintWrapKey` secret now holds kid 2, so nothing can mint a new kid 1 token. Kid 1 was therefore checked with a probe: a fresh kid 2 token whose JWS header was rewritten to `kid: "1"`. That token names kid 1, and `verify` looks the key up by the header's `kid` before it checks the signature.

- **Offline, with the edge's code** (`graftyard_identity::stateless::verify`, `tests/m3/hosted/tools` bin `verifyghs`), over the hosted `tokens:0` fold read through `fresh` after the commit [measured, `verify-offline.txt`]:

| Token | At | `verify` |
| --- | --- | --- |
| fresh kid 2 | now (22:37Z) | ok, kid 2 |
| kid 1 probe | `retired` − 1 ms | `Signature`: the key is still live, and the bytes are not kid 1's signature |
| kid 1 probe | `retired` | **`Retired`** |
| kid 1 probe | now | **`Retired`** |

  The fresh kid 2 token answers `NotYet` at the two earlier times, because its `iat` is later than them. That is expected.
- **Over HTTPS with stock git** (`tests/m3/hosted/retire-push.sh`, 5 pushes per token to `m3org/j4`'s main) [measured, `push.txt`]:
  - fresh kid 2 token: 5 of 5 accepted, ref moved;
  - kid 1 probe: 5 of 5 refused ("Authentication failed"), ref not moved.
- `GET /repos/m3org/j4/issues` answered 200 for both tokens and for no token at all, so a read cannot tell them apart. The push is the check.
- **What the probe does not show.** A genuine kid 1 signature refused as `Retired` was not observed. No kid 1 token exists to try, and no kid 1 signing key is kept. The code path is the same: `verify` refuses on `retired` before it checks the signature. The probe's `Retired` at `retired` and later, and its `Signature` 1 ms before, show the row is what refuses it [inference].
- Token scan: no `ghs_`, `ghp_`, or `gho_` text, and no `wrap` or `wrapped` field, in `tests/m3/raw/finish/` or on any process argv. The probe's token files were in a 0700 temp directory and were deleted [measured].

## Not done yet in this round

BF-30's transfer, BF-34's and BF-35's routes, BF-36's OIDC sign-in, and BF-33's "completes" branch are still to run.
