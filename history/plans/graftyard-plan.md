# Graftyard run plan

The workspace was `<vendor-workspace>/tributary-design/`. On 2026-10-26 the operator had it renamed to `<vendor-workspace>/graftyard/` ("graftyard" below), which is now the deliverable. It is restructured in place by `scripts/make_graftyard.py`, which moves files and never deletes them. "The archive" below means the record that moves into `history/`.

## Frame

**Done means all of these hold, checked by running the named command or reading the named file.**

1. Graftyard exists and holds everything an implementing agent needs, laid out as below.
2. The design includes leases and a task API in the agent layer's first release, and SSH through Spectrum (operator answers 2 and 7, `FRAME.md` "Operator decision, 2026-10-26, agents answered and graftyard").
3. The final adversarial pass over the base and the agent layer has run, its act-on items are applied, and the lead has read every applied fix.
4. From the graftyard root these all exit 0:
   - `python3 scripts/check_design.py`;
   - `python3 scripts/check_names.py`;
   - `python3 scripts/check_links.py`;
   - the cost scripts, whose output equals the tables in the documents.
5. A fresh agent that wrote none of it walks the folder as an implementer, and its blockers are fixed.
6. Nothing is deleted. Every pre-rename file is either in the layout below or in `history/`.

**Rigor.** High for design changes, because they are one-way doors for the implementers. Medium for the assembly, which is mechanical and gated by scripts.

## Layout

```
graftyard/
  README.md            index, reading order, glossary, status (hand-written)
  AGENTS.md            instructions for implementing agents (hand-written)
  FRAME.md             goals, quality attributes, constraints, every operator decision (hand-written from the archive FRAME.md)
  design/
    DESIGN.md          generated from the archive's cloudflare/DESIGN.md: the base, the design of record
    AGENT-LAYER.md     generated from the archive's cloudflare/AGENT-LAYER.md
    SHARED.md          hand-written: the platform-independent sections of the archive's DESIGN.md that the base reuses
  build/
    BUILD-PLAN.md      hand-written: milestones, Milestone 0 first, work packages, acceptance gates
  decisions/
    DECISIONS.md       hand-written: each decision with why, evidence, and the alternative it beat
    OPEN-QUESTIONS.md  hand-written: every open question and unverified claim, with the milestone that settles it
  alternatives/
    AWS.md             hand-written: the AWS design in summary, its costs, why Cloudflare is the design of record
  research/            generated: every archive research/ file
  scripts/             generated or adapted: cost_model.py, cost.py, cost_agents.py, costs_report.py, census.py,
                       check_design.py, check_names.py, check_links.py
  history/             verbatim copy of the archive's record, with old names and old paths, plus history/README.md
```

`history/` holds the archive's `DESIGN.md` (the AWS design), the original `FRAME.md`, `arena/`, `review/`, `plans/`, `decisions.tsv`, and the archive scripts that graftyard does not carry. It is the only place the old name may appear. `check_names.py` skips it, and `check_links.py` does not check paths inside it.

## Rename map

The rename applies to every generated and hand-written file outside `history/`. Apply the more specific rules first.

| Old | New |
| --- | --- |
| `tributary.dev` | `graftyard.dev` |
| `application/vnd.tributary.agent+json` | `application/vnd.graftyard.agent+json` |
| `X-Tributary-` (any header) | `X-Graftyard-` |
| `Tributary-Session` (trailer) | `Graftyard-Session` |
| `TributarySession`, `TributaryRun`, `TributaryHost`, any `Tributary` inside a CamelCase identifier | `GraftyardSession`, `GraftyardRun`, `GraftyardHost` |
| `refs/tributary/`, `/_tributary/`, `/api/tributary/`, `tributary/` check and status contexts | `refs/graftyard/`, `/_graftyard/`, `/api/graftyard/`, `graftyard/` |
| `tributary_task`, `tributary_trust`, `tributary_overlaps`, `tributary_agent`, `tributary_provenance`, any `tributary_` field | `graftyard_` |
| `tributary-agent`, `tributary-workspace`, `tributary-insights`, `tributary-plan`, `tributary-actions`, any `tributary-` name | `graftyard-` |
| `Tributary MCP` | `Graftyard MCP` |
| CLI `tributary` | `graftyard` |
| `trib-front`, `trib-ops`, `trib-mcp`, `trib-site-a/b/c`, `trib-sshd`, `trib-blob`, any `trib-` name at a word start | `graft-` |
| `.trib` virtual hosts (`tokens.trib`, `group.trib`, `pack.trib`, `git.trib`) | `.graft` |
| `trmcp_` | `gymcp_` |
| `Tributary` and `tributary` anywhere else | `Graftyard` and `graftyard` |

Words that merely contain "trib" (distribution, attribute, contributor) never change. The only allowed occurrence outside `history/` is a line that says the project was formerly called Tributary. `README.md` and `FRAME.md` each have one.

## Path map

The path map applies to citations in every file outside `history/`. Apply the more specific rules first.

| Archive path | Graftyard path |
| --- | --- |
| `cloudflare/DESIGN.md` | `design/DESIGN.md` |
| `cloudflare/AGENT-LAYER.md` | `design/AGENT-LAYER.md` |
| `cloudflare/cost.py` | `scripts/cost.py` |
| `cloudflare/cost_agents.py` | `scripts/cost_agents.py` |
| `scripts/check_cloudflare.py` | `scripts/check_design.py` |
| `DESIGN.md` alone, meaning the AWS design | `design/SHARED.md` when the citation names a section SHARED.md carries, else `history/DESIGN.md` |
| `review/<file>` | `history/review/<file>` |
| `arena/<path>` | `history/arena/<path>` |
| `decisions.tsv` | `history/decisions.tsv` |
| `research/<file>` | `research/<file>` (unchanged) |
| `FRAME.md` | `FRAME.md` (unchanged; the hand-written one) |

## Run order

1. Leases-and-tasks contract (`review/cloudflare-leases-tasks.md`), applied by a fresh writer and read by the lead.
2. Final adversarial pass by fresh seats over the whole design, then act-on items, a writer, and a lead read of every fix.
3. The generator (`scripts/make_graftyard.py` in the archive) and the graftyard check scripts are built and dry-run in parallel with steps 1 and 2. `SHARED.md` and `alternatives/AWS.md` are written in parallel too, because the archive's AWS design does not change.
4. Generate graftyard after step 2. Then the hand-written files: `FRAME.md`, `decisions/`, `build/BUILD-PLAN.md`, `README.md`, `AGENTS.md`.
5. Implementability walk by a fresh agent, then fixes made in graftyard directly.
6. Verify against the predicate, trail review, reply.
