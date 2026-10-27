#!/usr/bin/env python3
"""Check that every file path cited in decisions.tsv evidence resolves.

Usage: python3 scripts/audit_evidence.py

Paths resolve against the design directory first, then the workspace root above it.
URLs, agent job names, and prose are skipped. Exit code 0 means every path resolved.
"""

import csv
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
WORKSPACE = ROOT.parent
PATH_TOKEN = re.compile(r"[\w.\-{},*/]+\.(?:md|py|tsv|json)(?::[\d\-,]+)?")


def expand(token):
	token = token.split(":")[0]
	match = re.search(r"\{([^}]+)\}", token)
	if not match:
		return [token]
	return [token[: match.start()] + part + token[match.end():] for part in match.group(1).split(",")]


def resolves(path):
	if "*" in path:
		return any(ROOT.glob(path)) or any(WORKSPACE.glob(path))
	return (ROOT / path).exists() or (WORKSPACE / path).exists()


def main():
	missing = []
	checked = 0
	with open(ROOT / "decisions.tsv", newline="") as f:
		for n, row in enumerate(csv.DictReader(f, delimiter="\t"), start=2):
			evidence = re.sub(r"https?://\S+", "", row["evidence"])
			for token in PATH_TOKEN.findall(evidence):
				for path in expand(token):
					checked += 1
					if not resolves(path):
						missing.append(f"row {n}: {path}")
	for m in missing:
		print("MISSING", m)
	print(f"checked {checked} paths, {len(missing)} missing")
	return 1 if missing else 0


if __name__ == "__main__":
	sys.exit(main())
