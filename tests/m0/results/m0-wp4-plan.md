# M0-WP4 prototype plan

Working notes for `build/BUILD-PLAN.md`, M0-WP4, on the M0-WP2 Raft prototype (`src/raft/`). Results go in `results/m0-wp4.md`.

## What the prototype adds

- **Release in the member.** Each site script carries a `RELEASE` variable (an integer). A member puts its release on every AppendEntries and every reply, and `verify` refuses an entry whose `format` is above the highest format its release reads (`READS`, also a variable). This stands in for "N cannot decode an N+1 shape". A `BREAK_KIND` variable makes `verify` throw on one event key prefix, which stands in for "N+1 followers throw while applying one event kind" (B0-05 and B0-09).
- **Refusal rules** (`design/DESIGN.md`, `verify` doc comment). The leader classifies each refusal of a follower:
  - The refuser runs an older release and the entry is uncommitted. Truncate the uncommitted tail, transfer to an undrained follower holding the last index, and persist `no_lead_until = own release`. A member with `no_lead_until` equal to its release refuses to campaign. It clears when its release changes.
  - Both followers on the leader's own release refuse the same uncommitted entry. The leader's own defect: truncate and alarm.
  - Any other refusal (same or newer release, or a committed entry). Keep leading, add the refuser to `cannot_follow`, alarm, and commit with the other follower. `cannot_follow` clears when the refuser's release changes.
- **`Release` group in the prototype.** One Raft group named `release`, on the same three sites, holding `SiteMode` entries as register writes (`mode:a` = `drain` or `normal`, plus `drainedBy` = `drain` or `customer`). The drain-guard rule from `decide` is a compare-and-set on a `drained-by-drain-key` key, so a second drain-key `Drain` loses. Members read the site modes from the `release` group through the edge's cached view (`POINTER_TTL` 5 s) instead of a real `Release` view.
- **Drain.** A leader whose own site is drained transfers within one lease to an undrained caught-up follower. A drained member refuses to campaign while pre-vote finds an undrained voter that can lead. When every voter is drained, pre-vote allows a drained candidate.
- **Site day runner** (`run/site-day.ts`), in place of `graft-ops`. It drains the site, uploads N+1 with `wrangler versions upload`, deploys it at 1%, 10%, 50%, and 100% with `wrangler versions deploy`, checks the gates at each step from the members' own counters, runs the rollback (`versions deploy` of N at 100%) on a breach, then undrains. Dwell is minutes, not hours. B0-04, B0-05, B0-08, B0-09, and B0-10 drive it.
- **`SiteHealth`** (B0-07). One object per (observer site, target site). The edge and members count calls to each target site and calls that missed `CALL_DEADLINE`. The runner reads both observers of a site and drains it when both show the burn.

## Not in the prototype

Signatures and keys (`Keys`, `SignedIntent`, `Bound`), `ReleaseUploaded`, the pre-signed `undo` as a signed entry (the runner holds the rollback command instead), `Pointers`, `FIRST_DAY`'s 1% lead limit beyond the day's own steps, and spares. These belong to Milestone 5. The rows above test the protocol rules, not the signing.

## Risks

- A gradual deployment assigns each Durable Object one version until the next deployment, and an object resets when its version changes (https://developers.cloudflare.com/workers/configuration/versions-and-deployments/gradual-deployments/, "Gradual deployments for Durable Objects"). So 1% means about 1% of a site's members, chosen by Cloudflare. The runner learns which members run N+1 from their `release` in `/status`.
- A site-script version change resets that site's members, so each step resets about the step's share of them. B0-04's zero-failed-writes gate depends on drain having moved leadership first.
