#!/usr/bin/env python3
"""Check a Tributary design document against the mechanical predicates in FRAME.md.

Usage: python3 scripts/check_design.py DESIGN.md [--final]

Checks structure, not quality. P1 to P6 and P8 always run. --final adds P7, which needs
review/interrogate.md. P8 requires a Deployment modes table with one row per stateful
component from the Failure modes table, each with a non-empty Managed, BYOC, and
Self-hosted cell. Exit code 0 means every predicate held.
"""

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TIERS = {"T0", "T1", "T2", "T3", "Out"}
FAILURES = ["instance", "az", "region", "control-plane", "bad-deploy", "bad-config", "noisy-tenant"]
SHAPE_SECTIONS = [
	"Chains and commits",
	"Namespace and cell placement",
	"Repository chains and packs",
	"Identity and tokens",
	"Issues and pull requests",
	"Folds",
	"Actions runs and jobs",
	"Event fan-out",
]


def sections(text, level):
	marker = "#" * level + " "
	out = {}
	current = None
	for line in text.splitlines():
		if line.startswith(marker) and not line.startswith(marker + "#"):
			current = line[len(marker):].strip()
			out[current] = []
		elif re.match(r"^#{1,%d} " % (level - 1), line):
			current = None
		elif current is not None:
			out[current].append(line)
	return {k: "\n".join(v) for k, v in out.items()}


def table_rows(body):
	rows = []
	for line in body.splitlines():
		s = line.strip()
		if not s.startswith("|") or re.match(r"^\|[\s:|-]+\|$", s):
			continue
		rows.append([c.strip() for c in s.strip("|").split("|")])
	return rows


def check(path, final):
	text = Path(path).read_text()
	h2 = sections(text, 2)
	h3 = sections(text, 3)
	failures = []

	surfaces = json.loads((ROOT / "research" / "surfaces.json").read_text())
	ids = [s["id"] for s in surfaces]
	compat_rows = table_rows(h2.get("Wire compatibility", ""))
	by_id = {}
	for row in compat_rows:
		m = re.search(r"`([^`]+)`", row[0]) if row else None
		if m and len(row) >= 4:
			by_id[m.group(1)] = row
	for sid in ids:
		row = by_id.get(sid)
		if row is None:
			failures.append(f"P1 surface `{sid}` has no row in the Wire compatibility table")
		elif row[1] not in TIERS:
			failures.append(f"P1 surface `{sid}` tier '{row[1]}' is not one of {sorted(TIERS)}")
		elif row[1] != "Out" and not row[2]:
			failures.append(f"P1 surface `{sid}` has no conformance oracle")

	slo_rows = [r for r in table_rows(h3.get("SLOs", "")) if any("%" in c for c in r)]
	if len(slo_rows) < 4:
		failures.append(f"P2 SLOs table has {len(slo_rows)} journey rows with a percentage, needs at least 4")
	fm = {}
	for row in table_rows(h3.get("Failure modes", ""))[1:]:
		if len(row) >= 4 and row[0]:
			fm.setdefault(row[0].strip("`* "), set()).add(row[1].strip("`* ").lower())
	if len(fm) < 4:
		failures.append(f"P2 Failure modes table names {len(fm)} components, needs at least 4")
	for comp, seen in fm.items():
		missing = [f for f in FAILURES if f not in seen]
		if missing:
			failures.append(f"P2 component '{comp}' lacks failure rows: {', '.join(missing)}")

	modes = h2.get("Deployment modes", "")
	header_ok = any(
		all(m in " ".join(r) for m in ("Managed", "BYOC", "Self-hosted")) for r in table_rows(modes)[:1]
	)
	if not header_ok:
		failures.append("P3 Deployment modes needs a table whose header names Managed, BYOC, and Self-hosted")

	per_mode = {}
	for r in table_rows(modes):
		if r and r[0].strip("`* ").lower() != "component":
			per_mode[r[0].strip("`* ").lower()] = r[1:]
	has_component_table = any(
		r and r[0].strip("`* ").lower() == "component" and all(m in " ".join(r) for m in ("Managed", "BYOC", "Self-hosted"))
		for r in table_rows(modes)
	)
	if not has_component_table:
		failures.append("P8 Deployment modes needs a table with header '| Component | Managed | BYOC | Self-hosted |'")
	for comp in fm:
		cells = per_mode.get(comp.lower())
		if cells is None:
			failures.append(f"P8 stateful component '{comp}' has no row in the Deployment modes component table")
		elif len(cells) < 3 or any(not c.strip() for c in cells[:3]):
			failures.append(f"P8 stateful component '{comp}' has an empty Managed, BYOC, or Self-hosted cell")

	for name in SHAPE_SECTIONS:
		if "```rust" not in h3.get(name, ""):
			failures.append(f"P4 section '### {name}' has no rust type sketch")

	alts = h2.get("Alternatives considered", "")
	n_alts = len(re.findall(r"^(?:### |- \*\*|\d+\. \*\*)", alts, re.M))
	if n_alts < 2:
		failures.append(f"P5 Alternatives considered lists {n_alts} alternatives, needs at least 2")

	build = h2.get("Build order", "")
	if "Milestone 1" not in build or "Gate" not in build:
		failures.append("P6 Build order needs 'Milestone 1' and its 'Gate'")

	if final:
		review = ROOT / "review" / "interrogate.md"
		if not review.exists():
			failures.append("P7 review/interrogate.md does not exist")
		else:
			act_ids = sorted(set(re.findall(r"(?<![\w-])(?:[A-Z]{2}-)?ACT-\d+\b", review.read_text())))
			resolved = h2.get("Review resolutions", "")
			if not act_ids:
				failures.append("P7 review/interrogate.md lists no ACT-n findings")
			for aid in act_ids:
				if not re.search(r"(?<![\w-])" + re.escape(aid) + r"\b", resolved):
					failures.append(f"P7 {aid} has no entry under '## Review resolutions'")

	for ch, name in (("\u2014", "em dash"), ("\u2013", "en dash"), ("\u201c", "curly quote"), ("\u201d", "curly quote")):
		count = text.count(ch)
		if count:
			failures.append(f"style {count} {name} characters")

	return failures


def main():
	args = [a for a in sys.argv[1:] if not a.startswith("--")]
	if len(args) != 1:
		print(__doc__)
		return 2
	failures = check(args[0], "--final" in sys.argv)
	for f in failures:
		print("FAIL", f)
	print(f"{args[0]}: {'OK' if not failures else str(len(failures)) + ' failures'}")
	return 0 if not failures else 1


if __name__ == "__main__":
	sys.exit(main())
