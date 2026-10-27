# History

This folder is the record of how the design was reached. It holds the Tributary design workspace as it stood when
`history/scripts/make_graftyard.py` restructured it into Graftyard. Every file here except this README is verbatim. It
keeps the old name, Tributary, and the old paths, which are relative to the old workspace root. `scripts/check_names.py`
skips this folder, and `scripts/check_links.py` does not check the paths inside it. Paths in this README are relative
to the repository root.

## What it holds

- `history/DESIGN.md` is the AWS design. `design/SHARED.md` carries the sections of it that the Cloudflare base
  reuses, and `alternatives/AWS.md` summarizes it.
- `history/FRAME.md` is the original frame. The root `FRAME.md` is written from it.
- `history/arena/` holds every design arena: the task, the candidates, and the synthesis of each round.
- `history/review/` holds every rubric, judge verdict, adversarial review, walk, and trail review.
- `history/plans/` holds the run plans, including `history/plans/graftyard-plan.md`, which set this layout.
- `history/decisions.tsv` is the decision log.
- `history/scripts/` holds the workspace scripts that Graftyard does not carry, and `make_graftyard.py`, which did
  the restructure. `scripts/costs_report.py` loads `history/scripts/cost_small.py` and
  `history/arena/round4/candidate-3/cost.py` to write the AWS sections of `research/costs.md`. To run either one
  directly, run it from the repository root with `PYTHONPATH=scripts`, so that it finds `scripts/cost_model.py`.

## Path map

The restructure rewrote citations outside this folder with this map. Inside this folder, read a cited path through it.

| Old path | Graftyard path |
| --- | --- |
| `tributary-design/<path>` | `<path>` |
| `cloudflare/DESIGN.md` | `design/DESIGN.md` |
| `cloudflare/AGENT-LAYER.md` | `design/AGENT-LAYER.md` |
| `cloudflare/cost.py` | `scripts/cost.py` |
| `cloudflare/cost_agents.py` | `scripts/cost_agents.py` |
| `scripts/check_cloudflare.py` | `scripts/check_design.py` |
| `scripts/check_design.py` (the AWS design checker) | `history/scripts/check_design.py` |
| `scripts/audit_evidence.py`, `scripts/check_candidate.py`, `scripts/cost_small.py` | `history/scripts/<file>` |
| `DESIGN.md` | `design/SHARED.md` when the citation names a section SHARED.md carries, else `history/DESIGN.md` |
| `FRAME.md` | `history/FRAME.md` for the original; the root `FRAME.md` replaces it |
| `review/<file>` | `history/review/<file>` |
| `arena/<path>` | `history/arena/<path>` |
| `plans/<file>` | `history/plans/<file>` |
| `decisions.tsv` | `history/decisions.tsv` |
| `research/<file>` | `research/<file>` (unchanged) |
