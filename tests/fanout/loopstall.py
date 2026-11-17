#!/usr/bin/env python3
"""loopstall.py A1_ROWS.jsonl [MISSES.txt] -> whether a B0-34 row's client-side times carry a1.mjs's own event-loop
stalls (T-01a1ccdd's B0-34 trace, 2026-11-17).

`tip:move` ran `git push` through `execFileSync` (tests/am1/hosted/lib.mjs, `Git.run`), which stops node's event loop
for the whole push, so every timer and fetch in a1 ends late meanwhile. It reads the summary row's `b034.misses`
(probe start `at`, first byte `ms`, `want`) and prints:
  - probe start gaps over 5 s against a Poisson schedule's expectation, and the gap at a want change (a tip move ended);
  - probe ends grouped within 300 ms (3 or more), against uniform times at the same rate;
  - each want change's preceding gap (over 3 s) as a blocked window, and first-byte p50/p90/p99 raw and with each
    probe's overlap with those windows taken out (an estimate: a stall shorter than a gap is not seen).
With MISSES.txt (tests/fanout/misses.py's output), it also prints, inside each group, how far apart the shard's own
first bytes were: groups whose server first bytes are seconds apart were released together by the client.
"""
import bisect, datetime, json, random, statistics, sys

ms = lambda s: datetime.datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000
row = [r for r in map(json.loads, open(sys.argv[1])) if isinstance(r, dict) and "b034" in r][-1]
pr = sorted(row["b034"]["misses"], key=lambda p: ms(p["at"]))
starts = [ms(p["at"]) for p in pr]
gaps = [b - a for a, b in zip(starts, starts[1:])]
mean = sum(gaps) / len(gaps)
print(json.dumps({"probes": len(pr), "gaps_over_5s": sum(g > 5000 for g in gaps),
                  "poisson_expects": round(len(gaps) * 2.718281828 ** (-5000 / mean), 1)}))
changes, blocks, prev = [], [], None
for i, p in enumerate(pr):
    if p["want"] != prev:
        if prev is not None:
            changes.append(starts[i] - starts[i - 1])
            if starts[i] - starts[i - 1] > 3000:
                blocks.append((starts[i - 1], starts[i]))
        prev = p["want"]
print(json.dumps({"gap_at_want_change_median_ms": statistics.median(changes), "gap_median_ms": statistics.median(gaps)}))


def groups(ends):
    out, cur = [], [ends[0]]
    for e in ends[1:]:
        if e - cur[-1] <= 300:
            cur.append(e)
        else:
            out.append(cur)
            cur = [e]
    out.append(cur)
    return [g for g in out if len(g) >= 3]


ends = sorted(ms(p["at"]) + p["ms"] for p in pr)
base = sorted(sum(map(len, groups(sorted(random.uniform(ends[0], ends[-1]) for _ in ends)))) for _ in range(200))
print(json.dumps({"probes_in_end_groups": sum(map(len, groups(ends))), "uniform_p50": base[100], "uniform_max": base[-1]}))
ov = lambda a, b: sum(max(0, min(b, y) - max(a, x)) for x, y in blocks)
pct = lambda xs, q: sorted(xs)[min(len(xs) - 1, int(q * len(xs)))]
raw = [p["ms"] for p in pr]
cor = [p["ms"] - ov(starts[i], starts[i] + p["ms"]) for i, p in enumerate(pr)]
slow = pct(raw, 0.9)
print(json.dumps({"blocked_windows": len(blocks), "blocked_s": round(sum(b - a for a, b in blocks) / 1000),
                  "raw": [pct(raw, q) for q in (0.5, 0.9, 0.99)], "less_blocked": [pct(cor, q) for q in (0.5, 0.9, 0.99)],
                  "slowest_10pct_overlapping": sum(ov(starts[i], starts[i] + p["ms"]) > 0 for i, p in enumerate(pr) if p["ms"] >= slow),
                  "slowest_10pct": sum(p["ms"] >= slow for p in pr)}))
if len(sys.argv) > 2:
    rows = [json.loads(l) for l in open(sys.argv[2]) if l.startswith('{"at"')]
    rows = [r for r in rows if r.get("edge_net") is not None]
    ev = sorted((ms(r["at"]) + r["probe_ms"], ms(r["at"]) + r["probe_ms"] - r["edge_net"], i) for i, r in enumerate(rows))
    spread, cur = [], [ev[0]]
    for e in ev[1:] + [(float("inf"), 0, -1)]:
        if e[0] - cur[-1][0] <= 300:
            cur.append(e)
            continue
        if len(cur) >= 3:
            spread.append(max(x[1] for x in cur) - min(x[1] for x in cur))
        cur = [e]
    print(json.dumps({"groups": len(spread), "server_first_byte_spread_median_ms": statistics.median(spread),
                      "groups_spread_over_2s": sum(s > 2000 for s in spread)}))
