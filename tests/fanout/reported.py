#!/usr/bin/env python3
"""reported.py TSAVE.jsonl[.xz] -> each voter's `/suspect` reports from `Trace::Reported` (28, colo-unknown):
[at, 28, about, term, colo, verdict code], with verdict codes from member.rs `VERDICTS` (append-only, same list here).
Per voter: reports by colo (LAX and the like decoded from the packed IATA, 0 as UNKNOWN) and by verdict; then per term,
the colos that reported its leader and how many reports a promise answered (`promised`) or started a campaign
(`started`).
  python3 tests/fanout/reported.py tests/fanout/raw/tsave-TAG.jsonl.xz"""
import collections, json, lzma, os, sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from tracerows import load

VERDICTS = ["leader", "resumed", "started", "bootstrapping", "promised", "busy", "won", "lost", "mayNotLead"]
colo = lambda c: "UNKNOWN" if c == 0 else c.to_bytes(4, "big").lstrip(b"\0").decode("ascii", "replace")
path = sys.argv[1]
rows = load(path, keep=lambda x: x[1] == 28)
for s in sorted(rows):
    rs = sorted(rows[s])
    print(f"voter {s}: {len(rs)} reports; by colo {dict(collections.Counter(colo(r[4]) for r in rs))}; "
          f"by verdict {dict(collections.Counter(VERDICTS[r[5]] if r[5] < len(VERDICTS) else r[5] for r in rs))}")
    by_term = collections.defaultdict(list)
    for r in rs:
        by_term[r[3]].append(r)
    for t in sorted(by_term):
        g = by_term[t]
        print(f"  term {t}: {len(g)}; about {dict(collections.Counter(r[2] for r in g))}; colos "
              f"{dict(collections.Counter(colo(r[4]) for r in g))}; verdicts "
              f"{dict(collections.Counter(VERDICTS[r[5]] if r[5] < len(VERDICTS) else r[5] for r in g))}")
