#!/usr/bin/env python3
"""Restructure the Tributary design workspace in place into the Graftyard layout.

Usage: python3 scripts/make_graftyard.py --root DIR [--dry-run]

The layout, the rename map, and the path map come from plans/graftyard-plan.md. A run has four steps, and each is
idempotent, so a second run moves nothing, rewrites nothing, and refuses nothing.

1. Move. Each MOVES entry is an os.rename, made when its source exists and its destination does not. A move whose
   destination exists is done. If its source also exists and is not a path the layout owns, the run prints a refusal
   and leaves both. Nothing is deleted. The last move puts this script in history/scripts/.
2. Rewrite. Every file outside history/, except the hand-written ones (HAND_WRITTEN), gets CODE_FIXES, then
   PATH_RULES, then the bare `DESIGN.md` rule, then RENAME_RULES. The run prints every bare `DESIGN.md` citation and
   where it went, so the lead can audit it.
3. history/README.md is written when absent. If it exists with other content, the run prints a refusal.
4. Directories the moves emptied (EMPTIED) are removed.

--dry-run prints what steps 1 to 4 would do and changes nothing under DIR. The exit code is 1 when anything was refused.
"""

import argparse
import os
import re
import sys
from pathlib import Path

HISTORY_SCRIPTS = ("audit_evidence.py", "check_candidate.py", "check_cloudflare.py", "check_design.py", "cost_small.py")
TEMPLATES = ("check_design.py", "check_names.py", "check_links.py")
MOVES = (
	("cloudflare/DESIGN.md", "design/DESIGN.md"),
	("cloudflare/AGENT-LAYER.md", "design/AGENT-LAYER.md"),
	("cloudflare/cost.py", "scripts/cost.py"),
	("cloudflare/cost_agents.py", "scripts/cost_agents.py"),
	("DESIGN.md", "history/DESIGN.md"),
	("FRAME.md", "history/FRAME.md"),
	("arena", "history/arena"),
	("review", "history/review"),
	("plans", "history/plans"),
	("decisions.tsv", "history/decisions.tsv"),
	*((f"scripts/{name}", f"history/scripts/{name}") for name in HISTORY_SCRIPTS),
	*((f"scripts/graftyard_templates/{name}", f"scripts/{name}") for name in TEMPLATES),
	("scripts/make_graftyard.py", "history/scripts/make_graftyard.py"),
)
EMPTIED = ("cloudflare", "scripts/graftyard_templates")
HAND_WRITTEN = ("README.md", "AGENTS.md", "FRAME.md", "design/SHARED.md", "build", "decisions", "alternatives")
OWNED = HAND_WRITTEN + tuple(dst for _, dst in MOVES)

# Exact replacements in carried scripts, so they run from the graftyard root. Each pair applies once; a file that
# already holds the new text is left alone, and a file that holds neither stops the run.
CODE_FIXES = {
	"scripts/cost_agents.py": (
		('ROOT / "cloudflare" / "cost.py"', 'ROOT / "scripts" / "cost.py"'),
	),
	"scripts/costs_report.py": (
		('sys.path.insert(0, str(ROOT / "scripts"))\n',
			'sys.dont_write_bytecode = True  # loading history/ scripts must not write __pycache__ there\n'
			'sys.path.insert(0, str(ROOT / "scripts"))\n'),
		('import cost_small as small  # noqa: E402\n',
			'_sspec = importlib.util.spec_from_file_location("cost_small", ROOT / "history" / "scripts" / "cost_small.py")\n'
			'small = importlib.util.module_from_spec(_sspec)\n'
			'_sspec.loader.exec_module(small)\n'),
		('ROOT / "cloudflare" / "cost.py"', 'ROOT / "scripts" / "cost.py"'),
		('ROOT / "cloudflare" / "cost_agents.py"', 'ROOT / "scripts" / "cost_agents.py"'),
	),
}

# A path citation starts a word: the character before it is not a letter, digit, underscore, slash, dot, or hyphen.
START = r"(?<![\w./-])"
PATH_RULES = tuple((re.compile(START + pattern), new) for pattern, new in (
	(r"tributary-design/", ""),
	(r"cloudflare/DESIGN\.md", "design/DESIGN.md"),
	(r"cloudflare/AGENT-LAYER\.md", "design/AGENT-LAYER.md"),
	(r"cloudflare/cost\.py", "scripts/cost.py"),
	(r"cloudflare/cost_agents\.py", "scripts/cost_agents.py"),
	(r"scripts/check_cloudflare\.py", "scripts/check_design.py"),
	(r"scripts/(audit_evidence|check_candidate|cost_small)\.py", r"history/scripts/\1.py"),
	(r"review/(?=[\w.-]+\.md)", "history/review/"),
	(r"arena/", "history/arena/"),
	(r"plans/(?=[\w.-]+\.md)", "history/plans/"),
	(r"decisions\.tsv", "history/decisions.tsv"),
))
BARE = re.compile(START + r"DESIGN\.md")
SHARED_SECTIONS = (
	"Wire compatibility", "Known deviations from GitHub", "Chains and commits", "Repository chains and packs",
	"Identity and tokens", "Issues and pull requests", "Folds", "Actions runs and jobs", "Event fan-out", "Change safety",
	"Deployment modes", "Public service concerns", "Actions", "Frontend and native app",
)
SHARED_MARKERS = ("two-release rule", "RestoredTo")
SHARED = set(SHARED_SECTIONS) | set(SHARED_MARKERS)
COMPARISON = "Compared with the AWS design"
RENAME_RULES = tuple((re.compile(pattern), new) for pattern, new in (
	(r"trmcp_", "gymcp_"),
	(r"(?<![\w-])trib-", "graft-"),
	(r"(?<=\w)\.trib\b", ".graft"),
	(r"TRIBUTARY", "GRAFTYARD"),
	(r"Tributary", "Graftyard"),
	(r"tributary", "graftyard"),
))

HISTORY_README = """# History

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
"""


def walk(root):
	"""Every file and directory under root, relative and in posix form, mapped to itself."""
	out = {}
	for dirpath, dirnames, filenames in os.walk(root):
		rel = Path(dirpath).relative_to(root).as_posix()
		for name in dirnames + filenames:
			path = name if rel == "." else f"{rel}/{name}"
			out[path] = path
	return out


def under(path, prefixes):
	return any(path == p or path.startswith(p + "/") for p in prefixes)


def relocate(layout, src, dst):
	return {(dst + k[len(src):] if under(k, (src,)) else k): v for k, v in layout.items()}


def plan_moves(layout):
	"""The moves to make, the refusals, and the layout after the moves: final path to current path."""
	moves, refusals = [], []
	for src, dst in MOVES:
		if src in layout and dst not in layout:
			moves.append((src, dst))
			layout = relocate(layout, src, dst)
		elif src in layout and not under(src, OWNED):
			refusals.append(f"refused: {src} and {dst} both exist; left both in place")
		elif src not in layout and dst not in layout:
			refusals.append(f"missing: {src}, and {dst} does not exist")
	return moves, refusals, layout


def context(lines, i, pos):
	"""The table cell and sentence around a citation; a line ending in ':' adds the list that follows it."""
	line = lines[i]
	start, end = 0, len(line)
	if line.lstrip().startswith("|"):
		start = line.rfind("|", 0, pos) + 1
		end = line.find("|", pos)
		end = len(line) if end < 0 else end
	cell, at = line[start:end], pos - start
	cuts = [m.end() for m in re.finditer(r"\.\s+(?=[A-Z`*])", cell)]
	text = cell[max([c for c in cuts if c <= at], default=0):min([c for c in cuts if c > at], default=len(cell))]
	if line.rstrip().rstrip("*").endswith(":") and not any(c > at for c in cuts):
		j = i + 1
		while j < len(lines) and not lines[j].strip():
			j += 1
		while j < len(lines) and re.match(r"\s*(- |\d+\. )", lines[j]):
			text += "\n" + lines[j]
			j += 1
	return text


def bare_target(text, section, headings):
	if section == COMPARISON:
		return "history/DESIGN.md", f"in '{COMPARISON}', which cites the AWS design itself"
	named = {q for q in re.findall(r'"([^"]+)"', text) if q in headings} | {m for m in SHARED_MARKERS if m in text}
	if not named:
		return "history/DESIGN.md", "names no section"
	if named <= SHARED:
		return "design/SHARED.md", "names " + ", ".join(sorted(named))
	return "history/DESIGN.md", "names " + ", ".join(sorted(named - SHARED)) + ", which SHARED.md does not carry"


def resolve_bare(rel, text, headings, audit):
	lines = text.split("\n")
	section = None
	for i, line in enumerate(lines):
		if line.startswith("## "):
			section = line[3:].strip()
		if not BARE.search(line):
			continue

		def place(m):
			dst, why = bare_target(context(lines, i, m.start()), section, headings)
			audit.append(f"{rel}:{i + 1}: DESIGN.md -> {dst} ({why})")
			return dst

		lines[i] = BARE.sub(place, line)
	return "\n".join(lines)


def transform(rel, text, headings, audit):
	for old, new in CODE_FIXES.get(rel, ()):
		if new in text:
			continue
		if text.count(old) != 1:
			sys.exit(f"{rel}: code fix no longer applies, expected one {old!r}")
		text = text.replace(old, new)
	for pattern, new in PATH_RULES:
		text = pattern.sub(new, text)
	text = resolve_bare(rel, text, headings, audit)
	for pattern, new in RENAME_RULES:
		text = pattern.sub(new, text)
	return text


def rewritable(root, layout):
	for rel, current in sorted(layout.items()):
		parts = rel.split("/")
		if parts[0] == "history" or under(rel, HAND_WRITTEN) or "__pycache__" in parts or parts[-1] == ".DS_Store":
			continue
		if (root / current).is_file():
			yield rel, current


def main():
	ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
	ap.add_argument("--root", required=True, type=Path)
	ap.add_argument("--dry-run", action="store_true")
	args = ap.parse_args()
	root, dry = args.root.resolve(), args.dry_run
	verb = "would " if dry else ""

	moves, refusals, layout = plan_moves(walk(root))
	for src, dst in moves:
		print(f"{verb}move {src} -> {dst}")
		if not dry:
			(root / dst).parent.mkdir(parents=True, exist_ok=True)
			os.rename(root / src, root / dst)
	if not dry:
		layout = {k: k for k in layout}

	aws = layout.get("history/DESIGN.md")
	headings = set(re.findall(r"^#{2,4} (.+?)\s*$", (root / aws).read_text(), re.M)) if aws else set()
	audit, rewrites = [], 0
	for rel, current in rewritable(root, layout):
		try:
			text = (root / current).read_text()
		except UnicodeDecodeError:
			continue
		new = transform(rel, text, headings, audit)
		if new != text:
			rewrites += 1
			print(f"{verb}rewrite {rel}")
			if not dry:
				(root / rel).write_text(new)

	readme = root / "history" / "README.md"
	if "history/README.md" not in layout:
		print(f"{verb}write history/README.md")
		if not dry:
			readme.parent.mkdir(parents=True, exist_ok=True)
			readme.write_text(HISTORY_README)
	elif readme.read_text() != HISTORY_README:
		refusals.append("refused: history/README.md exists with other content; left it")

	for d in EMPTIED:
		left = [k for k in layout if k.startswith(d + "/")]
		if d in layout and not left:
			print(f"{verb}remove empty directory {d}")
			if not dry:
				os.rmdir(root / d)
		elif left:
			refusals.append(f"refused: {d} is not empty after the moves ({', '.join(left)}); left it")

	for line in audit:
		print(f"bare {line}")
	for line in refusals:
		print(line)
	print(f"moves {len(moves)}, rewrites {rewrites}, refusals {len(refusals)}, bare DESIGN.md citations {len(audit)}"
		+ (" (dry run, nothing changed)" if dry else ""))
	return 1 if refusals else 0


if __name__ == "__main__":
	sys.exit(main())
