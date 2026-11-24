#!/usr/bin/env python3
"""prep.py -> /tmp/shed/rows.pkl from m47's tsave, for the votershed replays (a1..b7). Run from the repo root:
  mkdir -p /tmp/shed && xz -dc tests/fanout/raw/tsave-m47b33.jsonl.xz > /tmp/shed/tsave.jsonl
  python3 tests/fanout/votershed/prep.py && python3 tests/fanout/votershed/a7.py"""
import os, pickle, sys
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), '..'))
from tracerows import load
pickle.dump({s: sorted(v) for s, v in load('/tmp/shed/tsave.jsonl').items()}, open('/tmp/shed/rows.pkl', 'wb'))
