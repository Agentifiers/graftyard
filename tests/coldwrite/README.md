# coldwrite: what a write costs on a cold edge isolate (F112)

`run.sh` runs the edge's own wasm (test-faults) against the site script's own build (test-faults) in one local
`wrangler dev`, with no Cloudflare account: three site scripts, eight edge scripts of the same edge build (each one a
separate isolate; one that has served nothing is cold), and a front (`driver.mjs`) that routes `x-cw-edge: N` to edge
script N. Each site's `Member` waits `CALL_MS` (60, 90 and 150 ms for sites a, b and c) before every call that is not
member-to-member Raft, as a stand-in for the edge-to-member round trip to its region, and logs it (`cwcall`), so each
request's member calls show in arrival order. `seed.mjs` makes the owner, login, `Placement`, `Authz`, repository,
install account and PAT through the edge's `/_bed/identity`, as `tests/m1/integ/seed.ts` does hosted. `push.mjs` pushes
once from edge 0, so the repository's group exists and leads, then on each cold edge sends a receive-pack POST,
then the same POST again warm. FORMATS_ENABLED is graft-m1-edge's list, with 32.8.

No git cache shard runs, so each POST ends at validate (503). The stages before validate are the product's.

```
bash tests/coldwrite/run.sh                         # build both scripts, serve, seed, measure, stop
B=/tmp/cw bash tests/coldwrite/run.sh               # reuse builds already in /tmp/cw/{site,edge}
```

## Where a cold POST's front and gate go (base build, `raw/base.txt`)

The edge 2 cold POST, with member calls in arrival order (ms from the request's start; site a is 62 ms away):

| ms | Calls | Why |
| --- | --- | --- |
| 9 | `names:438` (login), `tokens:*` (token) | `resolve`'s login read and `auth` start together |
| 78 | `names:174` (owner `Placement`), `names:24` (install `Placement`) | the install read starts only once `auth` answers (`needs_install`) |
| 145 | `authz:7000001`, `authz:1` | `resolve`'s name read and the install's fold read |
| 210 to 456 | `/status` on `authz:7000001` and on `authz:1`, the latter on site c | the spread read's `refresh_config` holds the read until the answering member's `/status` names the group's learners |
| 460 | `repo:65536` rows, owner's `Authz` again | the write gate reads both fresh (`share` is false for a write) |
| 526 | `/status` on `repo:65536` | `refresh_config` again, on the gate's rows read |

So on a cold isolate:
- `install` is serial after `auth`: one more round trip.
- The first spread read of each group costs a second round trip: the `/status` that names the learners. Three groups
  pay it: the owner's `Authz`, the install's `Authz` and the repository.
- The gate reads members, since a write never matches the data center's gate entry.
- `resolve` runs its three reads in turn, since a write takes no link guess.

Wasm start is not where the time goes. `auth` is 67 to 75 ms against one 62 ms round trip, and the edge's deploy
reports a startup time of 8 ms (`tests/am1/raw/b034mi-edgeday.log`). Hosted, the same serial steps account for B0-34
mt18's install of 684 to 800 ms: three round trips at about 230 to 270 ms [inference].

## The levers, measured on this bed

Arms are cumulative. Each is 6 cold first POSTs, one per fresh edge script; front and gate in ms, median (range).

| Arm | front | install | resolve | gate | front + gate |
| --- | --- | --- | --- | --- | --- |
| base (main e9c9f619e) | 452 (331-457) | 292 | 307 | 148 (133-320) | 602 (583-651) |
| A: the install read starts beside `auth`, its `Placement` and fold chained on the genesis record | 334 (276-461) | 170 | 308 | 158 (131-329) | 478 (434-781) |
| A+B: `refresh_config` runs after the read answers, not before | 234 (199-305) | 131 | 234 | 159 (98-162) | 393 (331-467) |
| A+B+C: a receive-pack POST's gate matches the data center's `Allow` entry | 266 (199-299) | 98 | 266 | 2 (2-3) | 268 (201-302) |
| A+B+C+E: a receive-pack POST takes the data center's link guess for `resolve` | 160 (101-166) | 100 | 160 | 2 (2-3) | 163 (104-168) |

A fifth arm, D, let a POST take an install `Authz` and `Placement` up to `READ_CACHE` old. It changed only the
warm POST's `install` (to 0 ms), not its front, which `resolve` sets (`raw/abcd.txt`).

Warm POSTs stayed at front 160 to 171 ms in every arm.

The prototype of A, B, C and E is `proto.diff` (A as `install_authz`'s `link`, B as a spawned `refresh_config`, C as
`read_gate`'s `share`, E as the link guess); the decision is `decisions/DECISIONS.md`, "`cold-write`". Raw output for
each arm is in `raw/`.

## Limits

- Round trips are fixed waits. Hosted round trips vary, and members do their own work besides.
- One client, no load. The batch (`edge-read-batch`) and the shed retries never engage.
- In C and E, the data center's entries came from edge 0's earlier POST. On a real push, the advertisement just before
  the POST would put them; the design has the advertisement put and the POST match.
- One local machine: no colo boundaries, and every edge script shares one Cache API.
