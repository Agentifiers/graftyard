#!/usr/bin/env python3
"""rounds.py TSAVE.jsonl [--from ISO] [--to ISO] [--gap 20] -> L0-12's read-round burst measure, per voter.

From a run's tests/nr3/tsave.mjs file: each voter's AppendQueued rows (trace kind 12: an AppendEntries handed to `call`,
`peer` its target) to the other voters (addresses under 128), counted per (voter, target, GAP ms window of `at`). A read
round is an AppendEntries to every voter, so a window's count bounds the rounds that voter sent that target in it; entry
appends count too, so it is an upper bound. `read-round-gap` allows one round per READ_ROUND_GAP (20 ms) while the last is
unanswered (`decisions/DECISIONS.md`, "`read-round-gap`"). Per voter: windows with any append, the max, p99, and the
windows over 5. m3fr6's L0-12 (no gap) read 99 / 61 / 84 max, 55 / 42 / 18 p99 and 213 / 45 / 270 windows over 5 for
sites 0 / 1 / 2 (`tests/am1/results/lm1-rows-m3f.md`, Rows m3fr7, L0-12); `--gap 1` counts per clock reading, as
`tests/m0/results/read-round-gap.md` did (max 99 / 61 / 84). tests/f74/rounds.py counts F74's CallSent (kind 23)."""
import collections, datetime, json, sys

args = sys.argv[1:]
if not args:
    sys.exit(__doc__)
opt = {args[i]: args[i + 1] for i in range(1, len(args) - 1, 2)}
iso = lambda s: int(datetime.datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
T0 = iso(opt["--from"]) if "--from" in opt else 0
T1 = iso(opt["--to"]) if "--to" in opt else 1 << 62
GAP = int(opt.get("--gap", 20))

# tsave saves each row once per voter save, and a reset voter's rows again from its new isolate: keep each row once.
rows = collections.defaultdict(set)
for line in open(args[0]):
    try:
        r = json.loads(line)
    except ValueError:
        continue
    if "rows" in r and "learner" not in r:
        rows[r["site"]].update(tuple(x) for x in r["rows"])

def pct(xs, p):
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(p * len(xs)))] if xs else None

out = {}
for site, rs in sorted(rows.items(), key=lambda kv: str(kv[0])):
    w = collections.Counter((peer, at // GAP) for at, kind, peer, *_ in rs if kind == 12 and peer < 128 and T0 <= at <= T1)
    ns = list(w.values())
    out[str(site)] = {"windows": len(ns), "max": max(ns, default=None), "p99": pct(ns, 0.99),
                      "over5": sum(1 for n in ns if n > 5)}
print(json.dumps({"gapMs": GAP, "from": opt.get("--from"), "to": opt.get("--to"), "bySite": out}))
