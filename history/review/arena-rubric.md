# Arena rubric for the Tributary design candidates

The picker and the cross-judge use this rubric. Candidates never see it. Score each criterion from 1 to 5 with one sentence of evidence that cites a section or line of the candidate. Do not score holistic feel.

1. **Availability.** Git reads, git writes, pull request merge, and the APIs survive the loss of one availability zone with no human action and no new capacity. Every request-path dependency is cell-local or regional, and no control-plane API (IAM, STS issuance, EC2 launch, Route 53 changes) sits on the request path. A mechanism, not a policy, bounds a bad deploy, a bad config push, and a noisy tenant to one cell or less. Region loss has a stated RPO and RTO and a drill.
2. **Durability and consistency.** The design names the exact point where a push is acknowledged, and at that point the data exists in at least two availability zones. Ref updates are linearizable per repository through a named mechanism. The design says whether pull request state can diverge from refs, and how the divergence is prevented or repaired.
3. **Wire compatibility realism.** The design uses the research facts correctly: toolkit hostname heuristics, first-party OAuth client ids, the GHES URL layout, ids below 2^53, node id formats, conditional requests, and pagination. Every oracle is a run of an unmodified client. Tiers are honest about effort and about the parts GitHub controls.
4. **One artifact and operability.** The mode table is short. A self-hoster knows what they run and what they repair at 2am. Upgrades are zero-downtime in every mode. The design reuses the existing BYOC machinery and names what it adds.
5. **Interface depth and simplicity.** Few distinct stateful systems and few AWS services on the request path. Deep modules with one owner per piece of state. No split ownership, no hand-synced lists, no two ways to do one task. Rust appears where it pays and the design says where it does not.
6. **Feasibility and sequencing.** The engineering cost of the core bet is stated honestly, for example the cost of reimplementing pack generation. Milestone 1 is small and verifiable. The riskiest unknown is retired first.

**Base selection.** Pick the candidate a future maintainer can extend most easily without breaking invariants. When two candidates tie, prefer the cleaner boundary and the smaller public interface.
