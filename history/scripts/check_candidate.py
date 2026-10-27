#!/usr/bin/env python3
"""Check a round-4 foundation candidate against arena/round4/TASK.md.

Usage: python3 scripts/check_candidate.py arena/round4/candidate-1/DESIGN.md [...]

Checks structure, not quality. Exit code 0 means every file passed.
"""

import re
import sys
from pathlib import Path

HEADINGS = [
	"Summary", "Usage (caller's view)", "Shape", "Commit and read paths", "Scale", "Downtime", "Failure modes",
	"Deployment modes", "Cost", "Build order from zero", "Changes to DESIGN.md", "Tradeoffs accepted",
	"Alternatives considered", "Open questions and risks",
]
FAILURES = ["instance", "az", "region", "control-plane", "bad-deploy", "bad-config", "noisy-tenant"]
MAX_LINES = 700


def sections(text):
	out, name = {}, None
	for line in text.splitlines():
		if line.startswith("## "):
			name = line[3:].strip()
			out[name] = []
		elif name:
			out[name].append(line)
	return {k: "\n".join(v) for k, v in out.items()}


def rows(section, header_first):
	table, inside = [], False
	for line in section.splitlines():
		cells = [c.strip() for c in line.strip().strip("|").split("|")] if line.strip().startswith("|") else None
		if cells is None:
			inside = False
			continue
		if cells[0].strip("`* ").lower() == header_first.lower():
			inside = True
			continue
		if inside and not set("".join(cells)) <= set("-: "):
			table.append(cells)
	return table


def check(path):
	text = Path(path).read_text()
	fails = []
	lines = text.splitlines()
	if len(lines) > MAX_LINES:
		fails.append(f"{len(lines)} lines, cap {MAX_LINES}")
	for ch, name in (("\u2014", "em dash"), ("\u2013", "en dash"), ("\u201c", "curly quote"), ("\u201d", "curly quote"),
			("\u2018", "curly quote"), ("\u2019", "curly quote")):
		if ch in text:
			fails.append(f"contains {name}")
	found = [l[3:].strip() for l in lines if l.startswith("## ")]
	if found != HEADINGS:
		fails.append(f"headings {found} differ from {HEADINGS}")
	s = sections(text)
	if "```rust" not in s.get("Shape", ""):
		fails.append("Shape has no rust sketch")
	if "| GitHub incident | Evidence | This design | Effect |" not in s.get("Downtime", ""):
		fails.append("Downtime lacks the incident table header")
	effects = [r[3].strip("`* ").lower() for r in rows(s.get("Downtime", ""), "GitHub incident") if len(r) >= 4]
	if len(effects) < 9 or any(e not in ("removed", "contained", "mitigated") for e in effects):
		fails.append(f"Downtime table needs at least 9 rows with Effect removed, contained, or mitigated, found {effects}")
	fm = {}
	for r in rows(s.get("Failure modes", ""), "Component"):
		if len(r) >= 4:
			fm.setdefault(r[0].strip("`* ").lower(), set()).add(r[1].strip("`* ").lower())
	if not fm:
		fails.append("Failure modes table missing or empty")
	for comp, seen in fm.items():
		missing = [f for f in FAILURES if f not in seen]
		if missing:
			fails.append(f"Failure modes '{comp}' lacks {missing}")
	modes = {r[0].strip("`* ").lower(): r[1:] for r in rows(s.get("Deployment modes", ""), "Component")}
	for comp in fm:
		cells = modes.get(comp)
		if cells is None:
			fails.append(f"Deployment modes has no row for '{comp}'")
		elif len(cells) < 3 or any(not c.strip() for c in cells[:3]):
			fails.append(f"Deployment modes row '{comp}' has an empty cell")
	cost = s.get("Cost", "")
	if not all(re.search(p, cost) for p in (r"\b50\b", r"\b300\b", r"\b2,?000\b")):
		fails.append("Cost does not cover 50, 300, and 2,000 developers")
	if "Milestone 0" not in s.get("Build order from zero", ""):
		fails.append("Build order from zero names no Milestone 0")
	return fails


if __name__ == "__main__":
	bad = 0
	for p in sys.argv[1:]:
		fails = check(p)
		bad += bool(fails)
		print(f"{p}: " + ("OK" if not fails else "\n  " + "\n  ".join(fails)))
	sys.exit(1 if bad else 0)
