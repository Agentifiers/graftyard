#!/usr/bin/env python3
"""dispatchsplit.py LINES.jsonl T0_MS -> where B0-35's site-side dispatch time goes, from dispatchlog.py's RAW lines.

Each line is one event's `actions dispatch` log from `ActionsTrigger` (`workers/site/src/actions_trigger.rs`): `ms` is
from the entry's `at` to the log, so `t - ms` is the entry's time. A round logs its events as each finishes, and
rounds run one at a time per leadership, so a site's lines fall in bursts (gaps over 3 s), and a burst that holds
`asks == 0` lines after one that did not starts a new round. For each event in a round whose previous round ended
less than 30 s before, the time splits into the wait for its round to start (the previous round's end less the
entry's time) and the time inside the round (the log less the round's start). Prints, for each event's first run:
entry-to-run percentiles, by asks and by entry minute from T0; the wait and inside-round split, inside by asks; and
the share of the slowest 1% and 5% that is each part. Nearest-rank percentiles, as the row's others."""
import collections, json, sys

lines = [json.loads(x) for x in open(sys.argv[1])]
t0 = int(sys.argv[2])


def pct(xs, q):
    s = sorted(xs)
    return s[min(len(s) - 1, int(q * len(s)))] if s else None


first = {}
for x in sorted(lines, key=lambda y: y["t"]):
    k = (x["index"], x["n"], x["action"])
    if x["runs"] > 0 and k not in first:
        first[k] = x
ev = list(first.values())
out = {"events": len(ev), "entryToRunMs": {q: pct([x["ms"] for x in ev], q) for q in (0.5, 0.9, 0.99)},
       "byAsks": {}, "byEntryMinute": {}}
for a in sorted({x["asks"] for x in ev}):
    s = [x["ms"] for x in ev if x["asks"] == a]
    out["byAsks"][a] = {"n": len(s), "p50": pct(s, 0.5), "p99": pct(s, 0.99)}
by_min = collections.defaultdict(list)
for x in ev:
    by_min[(x["t"] - x["ms"] - t0) // 60000].append(x["ms"])
out["byEntryMinute"] = {m: {"n": len(s), "p50": pct(s, 0.5), "p99": pct(s, 0.99)} for m, s in sorted(by_min.items())}

split = []
for site in sorted({x["site"] for x in lines}):
    xs = sorted((x for x in lines if x["site"] == site), key=lambda y: y["t"])
    bursts, cur = [], []
    for x in xs:
        if cur and x["t"] - cur[-1]["t"] > 3000:
            bursts.append(cur)
            cur = []
        cur.append(x)
    if cur:
        bursts.append(cur)
    rounds = []
    for b in bursts:
        new = not rounds or b[0]["t"] - rounds[-1][-1][-1]["t"] > 30000 or (
            any(x["asks"] == 0 for x in b) and max(y["asks"] for y in rounds[-1][-1]) > 0)
        rounds.append([b]) if new else rounds[-1].append(b)
    prev = None
    for r in rounds:
        start = prev if prev and r[0][0]["t"] - prev < 30000 else None
        if start:
            for b in r:
                for x in b:
                    if first.get((x["index"], x["n"], x["action"])) is x:
                        split.append({"ms": x["ms"], "wait": start - (x["t"] - x["ms"]), "inside": x["t"] - start,
                                      "asks": x["asks"], "round": r[-1][-1]["t"] - start})
        prev = r[-1][-1]["t"]
out["split"] = {"events": len(split), "waitForRound": {q: pct([s["wait"] for s in split], q) for q in (0.5, 0.9, 0.99)},
                "insideRound": {q: pct([s["inside"] for s in split], q) for q in (0.5, 0.9, 0.99)},
                "roundMs": {q: pct([s["round"] for s in split], q) for q in (0.5, 0.99)},
                "insideByAsks": {a: {"n": len(v), "p50": pct(v, 0.5), "p99": pct(v, 0.99)} for a, v in sorted(
                    collections.defaultdict(list, {a: [s["inside"] for s in split if s["asks"] == a]
                                                   for a in {s["asks"] for s in split}}).items())}}
for share in (0.01, 0.05):
    top = sorted(split, key=lambda s: -s["ms"])[:max(1, int(len(split) * share))]
    out["split"][f"slowest{int(share * 100)}pct"] = {
        "n": len(top), "minMs": min(s["ms"] for s in top),
        "waitMean": round(sum(s["wait"] for s in top) / len(top)), "insideMean": round(sum(s["inside"] for s in top) / len(top))}
print(json.dumps(out, indent=1))
