#!/usr/bin/env python3
"""Check the base design and the agent layer for structure, shared text, and cost figures.

Usage:
	python3 scripts/check_design.py                                  # every check on both documents
	python3 scripts/check_design.py [--final | --layer] FILE [...]   # structure checks on the named files only

Checks structure, not quality. With --final, a file must have the base headings and at most 2,200 lines. Every product
in its `## Stack` table must appear in the first table of research/cloudflare.md with the same status, and the status
must not be `not found` or `deprecated`. Non-Cloudflare cloud services may appear only in the sections that compare.
With --layer, a file must have the agent layer headings, at most 1,100 lines, and a GHES compatibility table whose tags
are known. Both need a rust sketch in Shape, a failure-mode table that covers every failure class per component, a
deployment row per component, costs at 50, 300, and 2,000 developers, and a Milestone 0. A file with neither flag gets
the base headings and a 700-line cap.

With no arguments, the script checks design/DESIGN.md with --final and design/AGENT-LAYER.md with --layer, and runs two
more checks. The agent block in Shape, from the `// ---- Agent load and agent safety` line up to the next `// ----`
line, must be byte-identical in both documents. The figures that `scripts/cost.py` and `scripts/cost_agents.py` print
must appear in the Cost sections: each size's total per variant as one row of design/DESIGN.md, the `Added total` and
`Base total` rows verbatim in design/AGENT-LAYER.md, and the largest size's added total in the design/DESIGN.md row
for the agent layer. Exit code 0 means every check passed.
"""

import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BASE = ROOT / "design/DESIGN.md"
LAYER = ROOT / "design/AGENT-LAYER.md"
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
AGENT_BLOCK = "// ---- Agent load and agent safety"


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


def failure_and_deployment(s):
	fails, fm = [], {}
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
	return fails


def foreign(s, comparing):
	return [f"section '{name}' names non-Cloudflare service '{m}'"
		for name, body in s.items() if name not in comparing for m in sorted(set(FOREIGN.findall(body)))]


def check(path, final, layer=False):
	text = Path(path).read_text()
	lines = text.splitlines()
	fails = []
	cap = 1100 if layer else (2200 if final else 700)
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

	fails += foreign(s, COMPARING)
	if "```rust" not in s.get("Shape", ""):
		fails.append("Shape has no rust sketch")
	if "| Incident | Evidence | This design | Effect |" not in s.get("Downtime", ""):
		fails.append("Downtime lacks the '| Incident | Evidence | This design | Effect |' table")
	effects = [r[3].strip("`* ").lower() for r in rows(s.get("Downtime", ""), "Incident") if len(r) >= 4]
	if len(effects) < 12 or any(e not in EFFECTS for e in effects):
		fails.append(f"Downtime needs at least 12 rows with Effect in {sorted(EFFECTS)}, found {effects}")
	fails += failure_and_deployment(s)
	cost = s.get("Cost", "")
	if not all(re.search(p, cost) for p in (r"\b50\b", r"\b300\b", r"\b2,?000\b")):
		fails.append("Cost does not cover 50, 300, and 2,000 developers")
	if "Milestone 0" not in s.get("Build order from zero", ""):
		fails.append("Build order from zero names no Milestone 0")
	return fails


def check_layer(s):
	fails = foreign(s, LAYER_COMPARING)
	if "```rust" not in s.get("Shape", ""):
		fails.append("Shape has no rust sketch")
	ghes = rows(s.get("GHES compatibility", ""), "Surface")
	if not ghes:
		fails.append("GHES compatibility lacks the '| Surface | Path or field | Tag |' table")
	for r in ghes:
		tag = r[2].strip("`* ").lower() if len(r) >= 3 else ""
		if not tag.startswith(GHES_TAGS):
			fails.append(f"GHES compatibility row '{r[0]}' has tag '{tag}', want one of {list(GHES_TAGS)}")
	fails += failure_and_deployment(s)
	cost = s.get("Cost", "")
	if not all(re.search(p, cost) for p in (r"\b50\b", r"\b300\b", r"\b2,?000\b")):
		fails.append("Cost does not cover 50, 300, and 2,000 developers")
	if "Milestone 0" not in s.get("Build order", ""):
		fails.append("Build order names no Milestone 0")
	return fails


def agent_block(text):
	lines = sections(text).get("Shape", "").splitlines(keepends=True)
	start = next((i for i, l in enumerate(lines) if l.startswith(AGENT_BLOCK)), None)
	if start is None:
		return None
	end = next((i for i in range(start + 1, len(lines)) if lines[i].startswith(("// ----", "```"))), len(lines))
	return lines[start:end]


def check_agent_block(base_text, layer_text):
	a, b = agent_block(base_text), agent_block(layer_text)
	if a is None or b is None:
		return [f"Shape of {name} has no '{AGENT_BLOCK}' block" for name, x in (("design/DESIGN.md", a),
			("design/AGENT-LAYER.md", b)) if x is None]
	if a == b:
		return []
	n = next((i for i, (x, y) in enumerate(zip(a, b)) if x != y), min(len(a), len(b)))
	return [f"agent block differs at its line {n + 1} ({len(a)} lines in design/DESIGN.md, {len(b)} in "
		f"design/AGENT-LAYER.md)"]


def run(script):
	done = subprocess.run([sys.executable, str(ROOT / "scripts" / script)], cwd=ROOT, capture_output=True, text=True)
	if done.returncode:
		raise RuntimeError(f"scripts/{script} exited {done.returncode}: {done.stderr.strip()}")
	return done.stdout.splitlines()


def cell_list(line):
	return [c.strip() for c in line.strip().strip("|").split("|")]


def check_costs(base_text, layer_text):
	try:
		base_out, agent_out = run("cost.py"), run("cost_agents.py")
	except RuntimeError as e:
		return [str(e)]
	fails = []
	totals = {}
	for line in base_out:
		cells = cell_list(line)
		if len(cells) == 4 and cells[1].isalpha() and cells[2].startswith("$") and cells[0][:1].isdigit():
			totals.setdefault(cells[0], []).append(cells[2])
	if not totals:
		fails.append("scripts/cost.py printed no per-variant totals")
	base_cost = {l.strip() for l in sections(base_text).get("Cost", "").splitlines()}
	for devs, figures in totals.items():
		row = "| " + " | ".join([devs, *figures]) + " |"
		if row not in base_cost:
			fails.append(f"design/DESIGN.md Cost has no row '{row}' from scripts/cost.py")

	layer_cost = {l.strip() for l in sections(layer_text).get("Cost", "").splitlines()}
	wanted = [l.strip() for l in agent_out if l.startswith(("| **Added total**", "| Base total"))]
	if len(wanted) != 2:
		fails.append(f"scripts/cost_agents.py printed {len(wanted)} of the Added total and Base total rows")
	for row in wanted:
		if row not in layer_cost:
			fails.append(f"design/AGENT-LAYER.md Cost has no row '{row}' from scripts/cost_agents.py")
	added = next((cell_list(l)[-1].strip("*") for l in wanted if l.startswith("| **Added total**")), None)
	agent_rows = [l for l in base_cost if l.startswith("| Agent layer ")]
	if added and not any(cell_list(l)[-1] == added for l in agent_rows):
		fails.append(f"design/DESIGN.md Cost has no agent layer row ending in {added} from scripts/cost_agents.py")
	return fails


def main(args):
	paths = [a for a in args if a not in ("--final", "--layer")]
	if paths:
		results = [(p, check(p, "--final" in args, "--layer" in args)) for p in paths]
	else:
		base_text, layer_text = BASE.read_text(), LAYER.read_text()
		results = [
			("design/DESIGN.md", check(BASE, True)),
			("design/AGENT-LAYER.md", check(LAYER, False, True)),
			("agent block", check_agent_block(base_text, layer_text)),
			("cost figures", check_costs(base_text, layer_text)),
		]
	for name, fails in results:
		print(f"{name}: " + ("OK" if not fails else "\n  " + "\n  ".join(fails)))
	return 1 if any(fails for _, fails in results) else 0


if __name__ == "__main__":
	sys.exit(main(sys.argv[1:]))
