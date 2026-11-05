#!/usr/bin/env python3
"""BF-24's log audit: audit.py RUN.jsonl [...]. Reads the leader's committed log over the runs' entries (test-faults
`/log`) and checks what the latency numbers cannot show:

- every head the runs pushed to refs/heads/pr got exactly one `TestMerged`, with the trial's outcome (nothing skipped,
  nothing applied twice, a redelivered or re-sent batch included);
- refs/pull/N/head moved to each head once;
- the committed `PrSync` cursor each trial read never went back.
"""
import json, sys
from collections import Counter

files = sys.argv[1:]
rows = [json.loads(l) for p in files for l in open(p) if l.strip()]
heads = {r["head"]: r["conflict"] for r in rows}
lo = min(r["server"]["push"][0] for r in rows if (r.get("server") or {}).get("push"))

# bf24.py's helpers, without running its main (its arguments are a run name and a trial count).
sys.argv = [sys.argv[0], "audit", "0"]
src = open(__file__.replace("audit.py", "bf24.py")).read().replace("\nmain()\n", "\n")
g = {"__file__": __file__}
exec(compile(src, "bf24.py", "exec"), g)
s = g["leader_site"]()
log = []
at = lo
while True:
    page = g["member"](s, f"/log?from={at}&n=200", None).get("v") or []
    log += page
    if len(page) < 200:
        break
    at = page[-1]["index"] + 1
log = [e for e in log if e.get("committed")]

merged = Counter()
outcome = {}
pull = Counter()
for e in log:
    for t in e.get("testMerged") or []:
        if t["head"] in heads:
            merged[t["head"]] += 1
            outcome[t["head"]] = t["clean"]
    for name, new in e.get("refs") or []:
        if name == f"refs/pull/{g['N']}/head" and new in heads:
            pull[new] += 1

bad = []
for h, conflict in heads.items():
    if merged[h] != 1:
        bad.append(f"{h[:10]}: {merged[h]} TestMerged")
    elif outcome[h] == conflict:
        bad.append(f"{h[:10]}: TestMerged clean={outcome[h]} on a {'conflicting' if conflict else 'clean'} head")
    if pull[h] != 1:
        bad.append(f"{h[:10]}: refs/pull/N/head moved {pull[h]} times")
cursors = [r["cursor"] for r in rows if r.get("cursor") is not None]
back = [(a, b) for a, b in zip(cursors, cursors[1:]) if b < a]
print(f"entries {log[0]['index'] if log else None}..{log[-1]['index'] if log else None} on site {s}; heads {len(heads)}; "
      f"TestMerged once each: {sum(1 for h in heads if merged[h] == 1)}; cursor reads {len(cursors)}, backwards {len(back)}")
for b in bad:
    print("BAD", b)
print("PASS" if not bad and not back else "FAIL")
