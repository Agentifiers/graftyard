# Round 2 synthesis: the global namespace layer

## Base

Candidate 1, network-placed cells under a replicated global tier. The lead scored 26, 21, and 24 of 30. The cross-judge scored 25, 20, and 19 (`review/round2-judge.md`). Both picked candidate 1. It is the only candidate in which no request reads a remote store when the global tier or another cell is down: authentication and routing read a local projection, and authorization reads a replica in the same `core` snapshot as the refs.

## Convergence

All three runners, without coordination, placed every fork in its network's cell so cross-owner merges stay one transaction, served one public host that clients classify as GHES, made SSH keys unique service-wide with one service host key set, kept a local routing structure for static stability, and noted that Aurora DSQL multi-Region clusters cannot span continents. The judge confirmed that last fact against AWS documentation, and confirmed that DynamoDB multi-Region strong consistency can span continents (`review/round2-judge.md`, fact flags).

## Grafts into the base

| Graft | Source |
| --- | --- |
| A resident org's fork of an outside network is created detached inside its residency, and a pull request back upstream gets an explanatory 422 | candidate 2 |
| Placement slots carried in repository-scoped ids, so ids route by arithmetic and the projection holds names plus a slot map; slots, not single networks, move between cells | candidates 2 and 3 |
| `NameEpoch` checked inside the data cell's snapshot, so a stale route after a rename or reclaim is caught where the request is served | candidate 3 |
| A shadow `global` cluster fed by CDC with replayed production reads, as the schema canary for the one service-wide cluster | candidate 3 |
| Takedowns and private switches through short-lived signed CDN URLs that carry a `content_epoch`, with no CloudFront invalidation on the request path | candidate 3 |
| Projection delta guards on generation and on the share of entries removed | candidate 2 |
| An enterprise shortcode claim that reserves a whole managed-user login suffix | candidate 2 |

## Drops from the base

| Dropped | Reason |
| --- | --- |
| A `repo_card` replica in every data cell | Cell storage and apply writes would grow with the whole service. Cards go only to the index cells of the owner and of accounts with access. |
| Installation tokens verifiable from any cell's key | A compromised cell could mint tokens for every installation. A token is accepted only when its `kid` cell is the installation account's index cell. |
| Losing notifications after 3 days of destination outage | Park the destination's partition instead of expiring it. |

## Rejected

| Rejected | Source | Reason |
| --- | --- | --- |
| Owner-home cells with passports minted by the caller's home | candidate 2 | A user whose home is down and who has no cached passport cannot authenticate anywhere, and an authorizing mirror (`OrgAuthority`) has unbounded lag while its cell is cut off. |
| A `graph` cluster read on cold authentication, token minting, listings, and stars | candidate 3 | It puts one remote store back on the request path, and push and merge authorization moves to a cache up to 15 minutes stale. |
| A one-hour quarantine before a released login can be reclaimed | candidate 3 | A client-visible deviation. The base's freshness rule (`Lookup::Behind`) plus `NameEpoch` already make reclaims safe. |
| DynamoDB as the name authority | candidate 2 | Nothing requires surviving a continent loss. A DSQL cluster in the North American Region set survives a Region loss and keeps a claim, its account row, and its first membership in one transaction. |

## Flip condition

The base rests on `global`'s change log: 64 partition head rows on a two-Region North American DSQL cluster, appended in commit order with no gaps, with p99 apply lag under 1 s at signup-flood rates. Milestone 0 tests it. If appends can commit out of order or a head row caps at a few hundred appends per second, the base switches to candidate 2's thin claim registry.
