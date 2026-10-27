# Round 2 rubric: global namespace layer

Candidates never see this. Score each criterion from 1 to 5 with one sentence of evidence citing the candidate's text.

1. **Names are correct.** Global uniqueness holds under concurrent claims, with case folding. Renames, transfers, and deletions keep GitHub's redirects for the API and git. The design states retirement rules and what happens when a claim and the owner's creation fail halfway.
2. **No synchronous global dependency on the hot path.** Git reads and writes, merges, and API reads in one cell keep working when the global tier, the directory, or another cell is down. Routing is statically stable. The design says exactly which operations need the global tier (signup, name claims, renames) and degrades only those.
3. **Cross-owner features keep the core invariants.** A pull request from a fork owned by someone else still merges in one transaction, or the design states precisely what weakens and why that is acceptable. Stars, follows, notifications, and `viewer` queries are idempotent and converge.
4. **Identity at global scale.** Authentication is cell-local and fast. Revocation is bounded. Org membership and GitHub App installations work across cells. The design avoids recreating GitHub's shared auth database failure (`research/uptime.md#47`), or says why its version is different.
5. **One artifact and operability.** BYOC and self-hosted run the same code with the global tier collapsed into one install. Rebalancing and moving units between cells is defined, fenced, and drillable.
6. **Simplicity and subtraction.** Few new stateful systems. Most of `DESIGN.md` survives. Obsolete parts are deleted, not wrapped.

**Base selection.** Pick the candidate a future maintainer can extend most easily without breaking invariants. Break ties toward the smaller public interface and fewer new stateful systems.
