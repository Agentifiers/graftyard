#!/bin/bash
# B0-30 rerun (operator, 2026-11-16): sample site c's parent member and the leader's truncation every STEP_S (60) for
# N (120) samples, so a restore from the archive is caught with its start, duration and the lag after it.
# Rows: tests/am1/raw/b030r2-restore.jsonl (one per sample: each parent member's applied, role, SQLite, sqliteFull, and
# each member's installs, restores and truncatedTo from /stats). Read-only.
#   bash tests/am1/b030split/restore-watch.sh
cd "$(dirname "$0")/../../.."
export BED_TOKEN_FILE=$HOME/.graft-wp6/bed.token
OUT=tests/am1/raw/b030r2-restore.jsonl
for i in $(seq 1 "${N:-120}"); do
  node tests/am1/b030split/read.mjs child 2>/dev/null | grep '"chain":"repo:1048590"' > /tmp/rw-child.jsonl
  node tests/am1/b030split/read.mjs stats 2>/dev/null > /tmp/rw-stats.jsonl
  python3 - <<'EOF' >> "$OUT"
import json
c = [json.loads(l) for l in open("/tmp/rw-child.jsonl")]
s = [json.loads(l) for l in open("/tmp/rw-stats.jsonl")]
print(json.dumps({"at": (c or s or [{"at": None}])[0]["at"],
  "members": [{k: r.get(k) for k in ("site", "role", "term", "applied", "dbBytes", "sqliteFull", "error")} for r in c],
  "stats": [{k: r.get(k) for k in ("site", "installs", "restores", "truncatedTo", "error")} for r in s]}))
EOF
  sleep "${STEP_S:-60}"
done
exec sleep infinity
