#!/usr/bin/env python3
"""prep.py -> /tmp/shed/rows.pkl from m47's tsave, for the votershed replays (a1..b7). Run from the repo root:
  mkdir -p /tmp/shed && xz -dc tests/fanout/raw/tsave-m47b33.jsonl.xz > /tmp/shed/tsave.jsonl
  python3 tests/fanout/votershed/prep.py && python3 tests/fanout/votershed/a7.py"""
import collections, json, pickle
rows = collections.defaultdict(set)
for line in open('/tmp/shed/tsave.jsonl'):
    r = json.loads(line)
    if 'rows' in r and 'learner' not in r:
        rows[r['site']].update(tuple(x) for x in r['rows'])
pickle.dump({s: sorted(v) for s, v in rows.items()}, open('/tmp/shed/rows.pkl', 'wb'))
