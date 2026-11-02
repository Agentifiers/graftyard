#!/usr/bin/env python3
"""Check that every repository path cited outside history/ exists.

Usage: python3 scripts/check_links.py

The script reads every text file under the repository root except history/ and __pycache__. A citation is a token
that starts a word and either begins with one of the directories in ROOTS followed by a slash, or is one of the root
files in ROOT_FILES. A token starts a word when the character before it is not a letter, digit, underscore, slash,
dot, or hyphen. That rule skips paths inside other repositories, such as `deployments/scripts/platform-deploy/x.ts`,
and customer repository paths that start with a dot, such as `.github/workflows/` and `.mcp.json`. The agent layer's
protected-path list writes `**/AGENTS.md`, which the same rule skips.

Before the lookup, the script drops a `#anchor`, a `:N` or `:N-M` line suffix, and trailing punctuation. It does not
check these tokens:

- A token with a glob or placeholder character (`*`, `<`, `>`, `{`, `}`, `$`), which names a pattern, not a file.
- README.md and AGENTS.md cited in research/, design/DESIGN.md, and design/AGENT-LAYER.md. Those files predate this
  repository's own README.md and AGENTS.md, so a bare mention there names another project's README or a customer
  repository's AGENTS.md.

The script does not read history/, whose paths are relative to the old workspace. A citation of a history/ path from
outside history/ is checked like any other. Exit code 1 lists each missing path as file:line: path.
"""

import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ROOTS = ("design", "research", "scripts", "build", "decisions", "alternatives", "history")
ROOT_FILES = ("README", "AGENTS", "FRAME")
CITE = re.compile(r"(?<![\w./-])((?:" + "|".join(ROOTS) + r")/[^\s`'\"()\[\]|,;]*|(?:" + "|".join(ROOT_FILES)
	+ r")\.md(?![\w/-]))")
PATTERN_CHARS = set("*<>{}$")
FOREIGN_ROOT_FILES = {"README.md", "AGENTS.md"}
SKIPPED = {"history", "__pycache__", ".git", "node_modules", ".wrangler"}


def predates_root_files(rel):
	return rel.startswith("research/") or rel in ("design/DESIGN.md", "design/AGENT-LAYER.md")


def clean(token):
	token = token.split("#", 1)[0]
	token = re.sub(r":\d+(-\d+)?$", "", token.rstrip(".:"))
	return token.rstrip(".:")


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
	missing, cited = [], 0
	for rel, text in files():
		for n, line in enumerate(text.splitlines(), 1):
			for m in CITE.finditer(line):
				token = clean(m.group(1))
				if PATTERN_CHARS & set(token):
					continue
				if token in FOREIGN_ROOT_FILES and predates_root_files(rel):
					continue
				cited += 1
				if not (ROOT / token).exists():
					missing.append(f"{rel}:{n}: {token}")
	print("\n".join(missing) if missing else f"check_links: OK, {cited} citations")
	return 1 if missing else 0


if __name__ == "__main__":
	sys.exit(main())
