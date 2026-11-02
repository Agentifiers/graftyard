#!/usr/bin/env python3
"""Find the project's old name outside history/.

Usage: python3 scripts/check_names.py

Scans every text file under the repository root except history/, __pycache__, and binary files. A hit is the old name
in any case, a name with the old short prefix and a hyphen at a word start, a virtual host under the old short suffix,
or the old MCP token prefix (PATTERNS below). The only allowed hit is a line in README.md or FRAME.md, at the root, that
says the project was formerly called by the old name. Exit code 1 lists each hit as file:line.
"""

import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
# The character classes keep this file from matching its own patterns.
PATTERNS = re.compile(r"t[r]ibutary|(?<![\w-])t[r]ib-|(?<=\w)\.t[r]ib\b|t[r]mcp_", re.I)
ALLOWED = re.compile(r"formerly called [T]ributary")
ALLOWED_FILES = {"README.md", "FRAME.md"}
SKIPPED = {"history", "__pycache__", ".git", "node_modules", ".wrangler"}


def ignored():
	"""Paths git ignores, such as build output, so a generated file never fails a check."""
	out = subprocess.run(["git", "ls-files", "--others", "--ignored", "--exclude-standard", "--directory"],
		cwd=ROOT, capture_output=True, text=True).stdout
	return tuple(line.rstrip("/") for line in out.splitlines() if line)


def files():
	skip = ignored()
	for path in sorted(ROOT.rglob("*")):
		rel = path.relative_to(ROOT)
		if any(rel.as_posix() == p or rel.as_posix().startswith(p + "/") for p in skip):
			continue
		if path.is_file() and not SKIPPED & set(rel.parts):
			try:
				yield rel.as_posix(), path.read_text()
			except UnicodeDecodeError:
				continue


def main():
	hits, scanned = [], 0
	for rel, text in files():
		scanned += 1
		for n, line in enumerate(text.splitlines(), 1):
			if PATTERNS.search(line) and not (rel in ALLOWED_FILES and ALLOWED.search(line)):
				hits.append(f"{rel}:{n}: {line.strip()[:160]}")
	print("\n".join(hits) if hits else f"check_names: OK, {scanned} files")
	return 1 if hits else 0


if __name__ == "__main__":
	sys.exit(main())
