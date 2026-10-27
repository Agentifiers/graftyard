#!/usr/bin/env python3
"""Check a Cloudflare-native Tributary design against arena/cloudflare/TASK.md.

Usage: python3 scripts/check_cloudflare.py [--final | --layer] arena/cloudflare/candidate-1/DESIGN.md [...]

Checks structure, not quality. Every product in the `## Stack` table must appear in the first table of
research/cloudflare.md with the same status, and the status must not be `not found` or `deprecated`.
Non-Cloudflare cloud services may appear only in the sections that compare. --final raises the line cap
for cloudflare/DESIGN.md. --layer checks cloudflare/AGENT-LAYER.md against arena/cloudflare-agents/TASK.md
instead: its headings, a GHES compatibility table whose tags are known, and the same failure-mode, deployment,
cost, and Milestone 0 rules. Exit code 0 means every file passed.
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
HEADINGS = [
	"Summary", "Usage (caller's view)", "Stack", "Shape", "Commit and read paths", "Scale", "Downtime", "Failure modes",
	"Deployment modes", "Cost", "Build order from zero", "Compared with the AWS design", "Tradeoffs accepted",
	"Alternatives considered", "Open questions and risks",
]
LAYER_HEADINGS = [
	"Summary", "Artifacts' role", "Shape", "Journeys", "Scale", "GHES compatibility", "Failure modes", "Deployment modes",
	"Cost", "Build order", "Tradeoffs accepted", "Alternatives considered", "Open questions and risks",
]
LAYER_COMPARING = {"Alternatives considered", "Open questions and risks"}
GHES_TAGS = ("wire-compatible", "additive", "opt-in", "none")
FAILURES = ["instance", "data-center", "region", "provider-wide", "control-plane", "bad-deploy", "bad-config", "noisy-tenant"]
EFFECTS = {"removed", "contained", "mitigated", "exposed"}
COMPARING = {"Compared with the AWS design", "Alternatives considered", "Downtime", "Open questions and risks"}
FOREIGN = re.compile(r"\b(AWS|Amazon|DynamoDB|Aurora|DSQL|EC2|Lambda|OpenSearch|Route 53|CloudWatch|KMS|SES|Fargate|ECS|EKS|"
	r"S3 Standard|GCP|Google Cloud|Azure)\b")
UNUSABLE = {"not found", "deprecated"}


def sections(text):
	out, name = {}, None
	for line in text.splitlines():
		if line.startswith("## "):
			name = line[3:].strip()
			out[name] = []
		elif name:
			out[name].append(line)
	return {k: "\n".join(v) for k, v in out.items()}


def rows(section, first):
	table, inside = [], False
	for line in section.splitlines():
		if not line.strip().startswith("|"):
			inside = False
			continue
		cells = [c.strip() for c in line.strip().strip("|").split("|")]
		if cells[0].strip("`* ").lower() == first.lower():
			inside = True
			continue
		if inside and not set("".join(cells)) <= set("-: "):
			table.append(cells)
	return table


def norm(cell):
	return re.sub(r"\[[^\]]*\]", "", cell).strip("`* ").strip().lower()


def catalog():
	"""Product name to status, with aliases: the name without its parenthetical, and any 'now X' name inside it."""
	text = (ROOT / "research" / "cloudflare.md").read_text()
	out = {}
	for r in rows(text, "Product"):
		if len(r) < 2:
			continue
		name, status = norm(r[0]), norm(r[1])
		out[name] = status
		out.setdefault(re.sub(r"\s*\(.*\)", "", name).strip(), status)
		now = re.search(r"\(now ([^)]+)\)", name)
		if now:
			out.setdefault(now.group(1).strip(), status)
	return out


def check(path, final, layer=False):
	text = Path(path).read_text()
	lines = text.splitlines()
	fails = []
	cap = 850 if layer else (1400 if final else 700)
	if len(lines) > cap:
		fails.append(f"{len(lines)} lines, cap {cap}")
	for ch, name in (("\u2014", "em dash"), ("\u2013", "en dash"), ("\u201c", "curly quote"), ("\u201d", "curly quote"),
			("\u2018", "curly quote"), ("\u2019", "curly quote")):
		if ch in text:
			fails.append(f"contains {name}")
	found = [l[3:].strip() for l in lines if l.startswith("## ")]
	extra = {"Review resolutions", "Synthesis decision"} if (final or layer) else set()
	want = LAYER_HEADINGS if layer else HEADINGS
	if [h for h in found if h not in extra] != want:
		fails.append(f"headings {found} differ from {want}")
	s = sections(text)
	if layer:
		return fails + check_layer(s)

	known = catalog()
	stack = rows(s.get("Stack", ""), "Product")
	if not stack:
		fails.append("Stack has no '| Product | Status | Used for |' table")
	for r in stack:
		name, status = norm(r[0]), (norm(r[1]) if len(r) > 1 else "")
		if name not in known:
			fails.append(f"Stack product '{r[0]}' is not in research/cloudflare.md")
		elif known[name] in UNUSABLE:
			fails.append(f"Stack product '{r[0]}' is '{known[name]}' in research/cloudflare.md")
		elif status != known[name]:
			fails.append(f"Stack product '{r[0]}' status '{status}' differs from research '{known[name]}'")

	for name, body in s.items():
		if name in COMPARING:
			continue
		for m in sorted(set(FOREIGN.findall(body))):
			fails.append(f"section '{name}' names non-Cloudflare service '{m}'")

	if "```rust" not in s.get("Shape", ""):
		fails.append("Shape has no rust sketch")
	if "| Incident | Evidence | This design | Effect |" not in s.get("Downtime", ""):
		fails.append("Downtime lacks the '| Incident | Evidence | This design | Effect |' table")
	effects = [r[3].strip("`* ").lower() for r in rows(s.get("Downtime", ""), "Incident") if len(r) >= 4]
	if len(effects) < 12 or any(e not in EFFECTS for e in effects):
		fails.append(f"Downtime needs at least 12 rows with Effect in {sorted(EFFECTS)}, found {effects}")

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


def check_layer(s):
	fails = []
	for name, body in s.items():
		if name in LAYER_COMPARING:
			continue
		for m in sorted(set(FOREIGN.findall(body))):
			fails.append(f"section '{name}' names non-Cloudflare service '{m}'")
	if "```rust" not in s.get("Shape", ""):
		fails.append("Shape has no rust sketch")
	ghes = rows(s.get("GHES compatibility", ""), "Surface")
	if not ghes:
		fails.append("GHES compatibility lacks the '| Surface | Path or field | Tag |' table")
	for r in ghes:
		tag = r[2].strip("`* ").lower() if len(r) >= 3 else ""
		if not tag.startswith(GHES_TAGS):
			fails.append(f"GHES compatibility row '{r[0]}' has tag '{tag}', want one of {list(GHES_TAGS)}")
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
	if "Milestone 0" not in s.get("Build order", ""):
		fails.append("Build order names no Milestone 0")
	return fails


if __name__ == "__main__":
	args = sys.argv[1:]
	final = "--final" in args
	layer = "--layer" in args
	paths = [a for a in args if a not in ("--final", "--layer")]
	bad = 0
	for p in paths:
		fails = check(p, final, layer)
		bad += bool(fails)
		print(f"{p}: " + ("OK" if not fails else "\n  " + "\n  ".join(fails)))
	sys.exit(1 if bad else 0)
