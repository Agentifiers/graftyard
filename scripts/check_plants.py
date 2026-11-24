#!/usr/bin/env python3
"""Fail when a planted bug's anchor does not match its file exactly once.

Usage: python3 scripts/check_plants.py

A plant script swaps an anchor string in a source file for a mutant and runs the tests that should catch it. When a
later change rewrites the anchored code, the plant stops running, and nobody learns until it is next run. This check
reads every tracked plant's anchors and counts each in its file, without building or running anything, so drift fails
at merge time.

A plant is a tracked Python file under tests/ with a top-level `MUTANTS` list (and `tests/am1/hosted/plant.py`'s `GO`
list), or a `tests/m3/*/plant.sh` whose `plant NAME FILE OLD NEW` lines hold the anchors. A Python plant's
module-level imports, assignments and definitions are run to build the lists; its `main` is not. An entry's shape says
where its anchors are (`anchors` below); an unknown shape fails, so a new plant shape is added here rather than skipped.
Any drifted mutant fails the check: fix its anchor, or drop the mutant with a one-line reason in its plant file. Exit
code 1 lists each failure.

An anchor that matches once is necessary, not sufficient: the mutant may no longer compile (lm1mem #28, after `grant`
took a floor) or may match code the plant no longer means (lm1mem #95 matched a cat-file parser, not upload-pack's
stop), so only a plant run proves its mutants.
"""

import ast
import os
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
# Plant lists beyond `MUTANTS`, by file: their entries are (name, old, new) on the module's `GOFILE`.
EXTRA = {"tests/am1/hosted/plant.py": ("GO", "GOFILE")}


def tracked(*globs):
	out = subprocess.run(["git", "ls-files", "--", *globs], cwd=ROOT, capture_output=True, text=True, check=True)
	return [line for line in out.stdout.splitlines() if line]


def module(rel):
	"""The plant's top level without its statements, so `main` and `if __name__` blocks never run."""
	path = ROOT / rel
	tree = ast.parse(path.read_text(), rel)
	keep = (ast.Import, ast.ImportFrom, ast.Assign, ast.AnnAssign, ast.FunctionDef, ast.ClassDef)
	tree.body = [n for n in tree.body if isinstance(n, keep)]
	ns = {"__file__": str(path), "__name__": "check_plants"}
	sys.path.insert(0, str(path.parent))
	cwd = os.getcwd()
	try:
		os.chdir(ROOT)
		exec(compile(tree, rel, "exec"), ns)
	finally:
		os.chdir(cwd)
		sys.path.pop(0)
	return ns


def is_path(v):
	return isinstance(v, str) and "\n" not in v and (ROOT / v).is_file()


def olds(v):
	"""An anchor field: one string, a list of strings applied in turn, or a list of (old, new) pairs."""
	if isinstance(v, str):
		return [v]
	if isinstance(v, (list, tuple)) and all(isinstance(x, str) for x in v):
		return list(v)
	if isinstance(v, (list, tuple)) and all(isinstance(x, (list, tuple)) and len(x) == 2 for x in v):
		return [x[0] for x in v]
	return None


def anchors(m, default):
	"""(file, [anchor]) for one entry, or None for a shape this check does not know."""
	if not isinstance(m, (list, tuple)) or len(m) < 2 or not isinstance(m[0], str):
		return None
	if is_path(m[1]):
		f, rest = m[1], m[2:]
	elif default:
		f, rest = default, m[1:]
	else:
		return None
	first = olds(rest[0]) if rest else None
	if first is None:
		return None
	# (name, file, old, new, old2, new2): `tests/b034keep/plant.py`'s second edit, None when there is none.
	if len(m) == 6 and len(rest) == 4 and rest[2] is not None:
		second = olds(rest[2])
		if second is None:
			return None
		first += second
	return f, first


def default_file(ns):
	"""The one module-level constant naming a source file, for entries that name none."""
	files = {v for k, v in ns.items() if k.isupper() and is_path(v)}
	return files.pop() if len(files) == 1 else None


def python_plants():
	for rel in tracked("tests/*.py"):
		src = (ROOT / rel).read_text()
		if not re.search(r"^MUTANTS\s*=", src, re.M):
			continue
		try:
			ns = module(rel)
		except Exception as e:  # noqa: BLE001
			yield rel, None, None, [], f"does not load: {type(e).__name__}: {e}"
			continue
		lists = [(ns.get("MUTANTS"), default_file(ns))]
		if rel in EXTRA:
			name, file_var = EXTRA[rel]
			lists.append((ns.get(name), ns.get(file_var)))
		n = 0
		for entries, default in lists:
			for m in entries or []:
				got = anchors(m, default)
				name = m[0] if isinstance(m, (list, tuple)) and m and isinstance(m[0], str) else repr(m)[:60]
				if got is None:
					yield rel, n, name, [], "an entry shape scripts/check_plants.py does not know"
				else:
					yield rel, n, name, [(got[0], a) for a in got[1]], None
				n += 1


def shell_plants():
	"""`plant NAME FILE OLD NEW` calls, unquoted by bash itself with `plant` defined to print its arguments."""
	for rel in tracked("tests/m3/*/plant.sh"):
		lines = (ROOT / rel).read_text().splitlines()
		calls, i = [], 0
		while i < len(lines):
			if lines[i].startswith("plant "):
				j = i
				while lines[j].endswith("\\") or not complete("\n".join(lines[i:j + 1])):
					j += 1
				calls.append("\n".join(lines[i:j + 1]))
				i = j
			i += 1
		script = "plant() { printf '%s\\0' \"$@\"; printf '\\1\\0'; }\n" + "\n".join(calls)
		out = subprocess.run(["bash", "-c", script], cwd=ROOT, capture_output=True, check=True).stdout.decode()
		for n, call in enumerate(c for c in out.split("\1\0") if c):
			name, f, old = call.split("\0")[:3]
			yield rel, n, name, [(f, old)], None if is_path(f) else f"names no file {f}"


def complete(text):
	return subprocess.run(["bash", "-n", "-c", text], capture_output=True).returncode == 0


def main():
	bad, count = [], 0
	cache = {}
	for rel, n, name, pairs, err in [*python_plants(), *shell_plants()]:
		count += 1
		why = [err] if err else []
		for f, old in pairs:
			if f not in cache:
				cache[f] = (ROOT / f).read_text() if is_path(f) else ""
			c = cache[f].count(old)
			if c != 1:
				why.append(f"anchor found {c} times in {f}")
		if why:
			bad.append(f"drift (fix the anchor or drop the mutant): {rel}#{n} {name}: {'; '.join(why)}")
	for line in bad:
		print(line)
	print(f"check_plants: {'FAIL' if bad else 'OK'}, {count} mutants, {len(bad)} drifted")
	return 1 if bad else 0


if __name__ == "__main__":
	sys.exit(main())
