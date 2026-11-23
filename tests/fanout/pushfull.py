#!/usr/bin/env python3
"""pushfull.py A1_PUSHES.jsonl [FROM_ISO TO_ISO] -> where each refused agent push (`full`) was refused, by stage, as
m3f read B0-32 (`tests/am1/results/lm1-rows-m3f.md`, "Why full grew from 20 to 190"), from the per-push
`server-timing` a1 writes (`timing`):
  - edge (advert): `advert;dur=<advertised wait ms>`, refused at `receive_pack` on the leader's advert before staging or
    any send (`decisions/DECISIONS.md`, "`edge-refusal`");
  - edge: `bound;dur=0`, refused at `bound_at_send` before any send;
  - admission: sent, and the last `commit` stage under 600 ms, so the leader answered `Full` on arrival;
  - expire: the last `commit` stage 600 ms or more, so the command queued and `expire_agents` answered `Full` when its
    `until` passed (`crates/raft/src/member/leader.rs`);
  - untimed: no `server-timing` (a production edge, or no answer).
Also the budget left at the first send (`budget;dur=`, the edge's AGENT_PUSH_BOUND minus its front) for ok and full
pushes, in the m3f table's bands, and how many full pushes carried an `ask` or staged first. The window, if given, is
on Cloudflare's clock (the rows' `start`)."""
import collections, json, re, sys
from datetime import datetime

path = sys.argv[1]
ms = lambda s: int(datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
lo, hi = (ms(sys.argv[2]), ms(sys.argv[3])) if len(sys.argv) > 3 else (0, 1 << 62)
ADMIT_MS = 600
BANDS = [(0, 1000, "under 1 s"), (1000, 2000, "1 to 2 s"), (2000, 3000, "2 to 3 s"), (3000, 5001, "3 to 5 s")]


def stages(t):
    out = collections.defaultdict(list)
    for name, d in re.findall(r"([a-z_]+);dur=(\d+)", t or ""):
        out[name].append(int(d))
    return out


where, marks, budget = collections.Counter(), collections.Counter(), collections.defaultdict(collections.Counter)
n = collections.Counter()
for line in open(path):
    r = json.loads(line)
    if r.get("kind") != "agent" or not lo <= r.get("start", 0) < hi:
        continue
    res = r.get("result", "")
    full = "full" in res
    k = "full" if full else "ok" if res == "ok" else "other"
    n[k] += 1
    s = stages(r.get("timing"))
    if s.get("budget"):
        b = s["budget"][0]
        band = next((label for a, z, label in BANDS if a <= b < z), "3 to 5 s")
        budget[band][k] += 1
    elif "bound" in s:
        budget["under 1 s"][k] += 1
    if not full:
        continue
    if not r.get("timing"):
        where["untimed"] += 1
    elif "advert" in s and "commit" not in s:
        where["edge (advert)"] += 1
    elif "bound" in s and "commit" not in s:
        where["edge (bound_at_send)"] += 1
    elif s.get("commit") and s["commit"][-1] < ADMIT_MS:
        where["admission"] += 1
    elif s.get("commit"):
        where["expire_agents (queued)"] += 1
    else:
        where["no commit stage"] += 1
    for m in ("ask", "stagefirst", "busy", "missed", "suspect"):
        marks[m] += 1 if m in s else 0
print(json.dumps({"agentPushes": dict(n), "fullBy": dict(where.most_common()), "fullMarks": dict(marks)}))
print("budget left at the first send | ok / full")
for _, _, label in BANDS:
    print(f"  {label:10s} | {budget[label]['ok']} / {budget[label]['full']}")
