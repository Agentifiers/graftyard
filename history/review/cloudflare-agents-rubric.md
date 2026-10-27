# Rubric: agent layer for Tributary on Cloudflare

Score each candidate 1 to 5 per criterion. Weighted total out of 50. A Cloudflare or GitHub fact a score rests on that you refute caps that criterion at 2.

| Criterion | Weight | 5 means |
| --- | --- | --- |
| Uptime and safety | 3 | Adds no shared dependency on any commit or read path that the base did not have, states each new component's blast radius, and A1 survives a gray regional incident with numbers. Agent writes can never bypass rulesets, and A4's blast radius is bounded by the host |
| Scale under agent load | 2 | A1 is traced with per-object rates and headroom, write contention between agents is removed or bounded with a measured gate, and costs grow sub-linearly with agent count |
| GHES compatibility | 2 | No 3.22 response, default, or path changes for a client that does not opt in; A2 works with no Tributary-specific client code; every new surface is tagged |
| Agent usefulness | 1.5 | A3, A5, A6, and A7 give agents and reviewers something GHES alone does not, and the layer answers the blog's questions (who works on what, conflicts, reviewing many changes, why) |
| Build and operate risk | 1 | Least new code and fewest new products for the value; Milestone 0 has numeric gates |
| Install fit | 0.5 | Same behavior in managed, BYOC, and self-hosted; evaluation on workerd stated |
| Cost | 0.5 | Lowest added monthly cost at the three sizes for the stated value |

Also answer: is the candidate's "why not Artifacts" correct against `research/artifacts-recheck.md`, and what would flip it?
