#!/usr/bin/env python3
"""clonelines.py SAMPLES [OUTAGES] -> B0-34's two clone lines as re-based on 2026-11-20 (`FRAME.md`, "Operator
decision, 2026-11-20", item 2; `build/BUILD-PLAN.md`, B0-34), with clone-cache hits and misses kept apart.

SAMPLES is a1's `a1-samples-b0-34-<run>.jsonl.gz` ([start ms on Cloudflare's clock, kind, ms, code] per sample), from
the orb's load or from Load34's clients. The gated probes are `read:cloneTtfb:{hit,miss}@<phase>` (client clock) and
`read:cloneTtfb:{hit,miss}:edge@<phase>` (the edge's `server-timing` total; Load34 writes these, a1 on the orb did not,
so the edge column is blank on its rows). OUTAGES, when given, is arrivals.py's summary line or a file ending in it: a
sample that started in a harness-outage window, or under 3 s before one, is removed (outagecut.py's rule), and each line
is printed as measured and with the windows removed.

The lines, judged on the client clock:
  hits      clone-cache hits' TTFB p99 <= 1 s in steady state (the j9 phase);
  every     every clone's TTFB <= 5 s through the kill and cold phases, read with the 90% rule as a count line: the share
            of probes (hits and misses) <= 5 s in each of kill and second, passing at >= 90% (AGENTS.md, "How to pick
            work", item 5; BUILD-PLAN B0-34 notes). The share is printed beside "every" as written (100%).
Per phase it also prints hits and misses each with n, p50, p99 and max on both clocks, and the count over 5 s.
"""
import gzip
import json
import sys

from outagecut import cut, ms

PHASES = ["j9", "kill", "second", "hostile"]
STEADY, COLD = ["j9"], ["kill", "second"]
HIT_P99_MS, EVERY_MS, SHARE = 1000, 5000, 0.90


def pct(xs, q):
    return sorted(xs)[min(len(xs) - 1, int(q * len(xs)))] if xs else None


def stat(xs):
    return {"n": len(xs), "p50": pct(xs, .5), "p99": pct(xs, .99), "max": max(xs) if xs else None,
            "over5s": sum(1 for x in xs if x > EVERY_MS)}


def lines(samples):
    """{phase: {hit, miss, hitEdge, missEdge}} and the two verdicts, from one sample list."""
    by = {}
    for _, k, v, _ in samples:
        if not k.startswith("read:cloneTtfb:") or "@" not in k:
            continue
        kind, phase = k.split("@", 1)
        part = kind[len("read:cloneTtfb:"):]
        if part in ("hit", "miss", "hit:edge", "miss:edge"):
            by.setdefault(phase, {}).setdefault(part, []).append(v)
    out = {p: {name: stat(by.get(p, {}).get(part, [])) for name, part in
               (("hit", "hit"), ("miss", "miss"), ("hitEdge", "hit:edge"), ("missEdge", "miss:edge"))} for p in PHASES}
    hits = [v for p in STEADY for v in by.get(p, {}).get("hit", [])]
    cold = {p: by.get(p, {}).get("hit", []) + by.get(p, {}).get("miss", []) for p in COLD}
    share = {p: (round(sum(1 for v in xs if v <= EVERY_MS) / len(xs), 4) if xs else None) for p, xs in cold.items()}
    verdict = {
        "hits": {"p99": pct(hits, .99), "n": len(hits), "line": HIT_P99_MS,
                 "pass": bool(hits) and pct(hits, .99) <= HIT_P99_MS},
        "every": {"share": share, "line": SHARE, "pass": all(s is not None and s >= SHARE for s in share.values()),
                  "asWritten": all(s == 1.0 for s in share.values())},
    }
    return out, verdict


def main(argv):
    with gzip.open(argv[0], "rt") as f:
        samples = [json.loads(x) for x in f if x.strip()]
    wins = []
    if len(argv) > 1:
        src = argv[1]
        text = src if src.lstrip().startswith("{") else open(src).read().strip().splitlines()[-1]
        outs = json.loads(text)["outages"]
        wins = [(ms(a), ms(b)) for a, b in outs] if isinstance(outs, list) else []
    for label, ss in (("measured", samples), ("outagesRemoved", cut(samples, wins))):
        by, v = lines(ss)
        for p in PHASES:
            print(json.dumps({"clone": label, "phase": p, **by[p]}))
        h, e = v["hits"], v["every"]
        print(json.dumps({"clone": label, "line": "hits p99 <= 1 s in steady state", "p99": h["p99"], "n": h["n"],
                          "verdict": "pass" if h["pass"] else "fail"}))
        print(json.dumps({"clone": label, "line": "every clone <= 5 s through kill and cold (>= 90% of clones)",
                          "share": e["share"], "verdict": "pass" if e["pass"] else "fail",
                          "asWritten": "pass" if e["asWritten"] else "fail"}))


if __name__ == "__main__":
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    main(sys.argv[1:])
