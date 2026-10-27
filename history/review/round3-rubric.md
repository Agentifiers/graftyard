# Round 3 rubric: worldwide global tier

Candidates never see this. Score each criterion 1 to 5 with one sentence of evidence citing the candidate's text. The baseline for comparison is the current `DESIGN.md` (one DSQL `global` cluster in North America).

1. **Worldwide identity writes.** Signup, token creation, renames, and membership changes keep working and stay fast on each continent when another continent's Regions are impaired or lost.
2. **Guarantees kept.** No request reads a remote store, negative answers stay safe under fleet-relative freshness, revocation coverage counts edges, cut-off cells fail write grants closed, and claims stay race-free. Any weakening is stated and bounded.
3. **Correct use of DynamoDB.** The design uses MRSC and MREC only for what their consistency, ordering, and transaction semantics support, and cites AWS documentation for each property it relies on.
4. **Residency honesty.** It states exactly which identity facts stay in an account's geography and which leave, with reasons.
5. **Simplicity.** Few new stores and protocols, one artifact, and BYOC and self-hosted collapse to their current shape.
6. **Feasibility.** The riskiest unknown has a cheap Milestone 0 test, and the cost is stated.

**Base selection.** Pick the candidate a future maintainer can extend most easily without breaking the guarantees in criterion 2. Ties go to fewer new stores.
