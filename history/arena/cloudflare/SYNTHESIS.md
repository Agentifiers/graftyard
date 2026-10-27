# Cloudflare arena synthesis

## Base

Candidate 3, three-site quorum chains. Every chain (a repository, a fork network, an owner's `Authz` and `Identity`, a token shard, a name shard) is a Raft group of three SQLite-backed Durable Objects on three sites in three regions. A write is acknowledged once two members in two regions hold it and its pack is in two R2 buckets in two regions. Upstream git runs in Containers as a cache. Releases roll one site per day, and followers on the old release refuse entries they cannot decode.

## Scores

| Design | Lead | Cross-judge |
| --- | --- | --- |
| Candidate 1, one Durable Object per aggregate with a Paired standby | 38 | 35.5 |
| Candidate 2, Artifacts plus a confined container git engine | 24 | 22.5 |
| Candidate 3, three-site Raft groups | 37.5 | 41.5 |
| AWS design (`DESIGN.md`), reference only | not scored | 42.5 |

My first read put candidate 1 slightly ahead, because its single-writer object is the simplest correct commit point and its pairing protocol is smaller than Raft. The judge (`review/cloudflare-judge.md`) found a defect I missed and I accept it: when candidate 1 loses its standby's region, the primary can neither renew its lease nor go `Alone`, so every Paired object paired there stops reads and writes, and the design does not say so. With that, candidate 3 wins on downtime, which carries weight 3 and is the operator's first goal. Candidate 3's cost is owning a consensus protocol. Milestone 0 gates it.

## Grafts

1. **Sites as scripts in one account by default (from candidate 1).** Members reach each other through Durable Object bindings to classes in the other site scripts, not through public HTTPS, so a partial proxy failure like December 5, 2025 cannot cut replication. Each site is a separate Worker script pinned to its own region and rolled on its own day. Account-separated sites remain an option for customers who want account isolation, at the cost of replication over HTTPS.
2. **SSH writes through the container outbound handler (from candidate 1).** The SSH gateway container reaches Durable Object bindings directly (https://developers.cloudflare.com/containers/configuration/workers-connections/), so SSH does not loop back through the public zone.
3. **Non-voting learners for hot groups (from candidate 1's `AuthzReplica` and `RepoReader`).** A group whose read load passes 500 requests a second adds up to 16 learner members, placed by traffic, which serve read-index reads and never vote.
4. **A `Solo` placement (from candidate 1's Regional class).** One member, relying on the platform's 3-of-5 follower confirmation, for evaluation installs. It has no region-loss guarantee and no availability SLO, and the design says so.
5. **Merge-queue tips only fast-forward (from candidate 2).** A queue entry's `parent_tip` must equal the previous entry's tip, checked in `decide`.
6. **Honest control-plane row (from candidate 1 and the judge).** Whether new container instances start during a Cloudflare control-plane outage is unknown, so that row is `contained`, warm git caches are sized to carry a day, and Milestone 0 tests it.

## Rejected

- Candidate 1's pairing protocol and placement register. Raft over three sites replaces both.
- Candidate 2's use of Artifacts, its D1 directory, and its operator-signed region recovery. Artifacts is beta, closed, capped at 1 GB for new repositories, and keeps its tokens in Workers KV.
- Candidate 2's rule that a push commits while the R2 pack log fails. Candidate 3 keeps two bucket copies before the ack, plus packs up to 1 MiB inline in the entry.

## Flip condition

If Milestone 0's consensus gate fails (any linearizability violation in the 48-hour run, or `Tri` commit p99 above 400 ms), candidate 1 becomes the base, after its standby-loss stall is fixed by letting the primary compare-and-swap itself to `Alone` through the register.

## Verification

`scripts/check_cloudflare.py` passes all three candidates. The judge reran each cost script. The integrated `cloudflare/DESIGN.md` is checked with `scripts/check_cloudflare.py --final`, then reviewed by three adversarial seats that may attack the foundation itself.
