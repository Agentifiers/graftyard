#!/usr/bin/env python3
"""stepdown.py ALARMS RUN=DAY,FROM,TO [RUN=...] -> STEP_DOWN's confirm rule against candidate rules, offline, on the
B0-33 rows' logged trips (the orchestrator's ask after m54, 2026-11-19: site b tripped it 15 times in 184 s).

ALARMS is the sites' `member alarm` StepDown lines (with or without F108's group name) from Workers Logs, under `== <run>` headers
(`raw/stepdown-alarms-m50-m54.txt`). Each Confirm line carries the rule's own window: `slow` confirms over 500 ms of
`of`, and `to` (a transfer, or `None`, a hold). A candidate that only raises the slow count a trip needs is judged
exactly from those numbers: a logged trip it keeps, it would have made. A trip it drops starts no hold, so a later
check in that window could trip where today's did not; the kept counts are a lower bound on its trips [inference].

The trace cannot replay the rule itself. The rule times a confirm on the member clock from `confirm_begin` (the
input's step) to `confirm_end`, and the member clock is raised by every stamp, edge requests' included, which the
trace does not hold. Rebuilt from `Stepped` 15 and `Confirm` 10 with the peer stamps the trace has (`CallArrived` 20,
`AnswerArrived` 21, which ran p50 568 and p99 3,206 ms ahead of the raw clock on m54's site b), not one of m54's 11
logged Confirm windows came out at the logged slow count (replay 0 to 5 slow, logged 6 to 8). So a longer window
(20 or 30 s) is not judged here; it needs the rule's samples logged (`not measured`).

A real stall, for "missed": a leader second whose raw trace has a confirm of 2,000 ms or more or a ticker 1,000 ms or
more late, stall seconds within 10 s merged into one episode. Raw confirms overstate slow syncs on a lagging isolate
(`tests/am1/results/health-gate-lease.md`), so this is a stand-in. Per row and rule: Confirm trips kept, transfers
kept, and kept and dropped trips with a stall in the 10 s before; then each transfer with the leader's flushed
statements a second over the 10 s before it, for F92's "a confirm trip under write load holds".
"""
import bisect, collections, datetime, json, lzma, math, os, re, sys

iso = lambda s: int(datetime.datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
RULES = {
    "now": lambda n: 6,
    "share25": lambda n: max(6, math.ceil(0.25 * n)),
    "share33": lambda n: max(6, math.ceil(0.33 * n)),
    "share40": lambda n: max(6, math.ceil(0.40 * n)),
}
BUSY_STMTS = 100


def trace(tag):
    p = f"tests/fanout/raw/tsave-{tag}.jsonl"
    p = p if os.path.exists(p) else p + ".xz"
    rows = collections.defaultdict(set)
    for line in (lzma.open if p.endswith(".xz") else open)(p, "rt"):
        d = json.loads(line)
        if "rows" in d and d.get("learner") is None:
            rows[d["site"]].update(tuple(x) for x in d["rows"] if x[1] in (8, 10, 16))
    terms = [(iso(d["at"]), {m[0]: m[1] for m in d["members"]})
             for d in map(json.loads, open(f"tests/fanout/raw/terms-{tag}.jsonl")) if d.get("members")]
    return rows, terms


def main():
    alarms, run = collections.defaultdict(list), None
    for line in open(sys.argv[1]):
        if line.startswith("=="):
            run = line.split()[1]
            continue
        # Since F108 the host line is `member alarm <do name> <do id>: StepDown { ... }`; older lines lack the name.
        m = re.match(r"(\S+) (\w) (?:member alarm(?: \S+ \S+)?: )?StepDown \{ why: (\w+) \{ (.*?) \}, to: (.*) \}",
                     line.strip())
        if m:
            alarms[run].append({"at": m[1], "site": "abc".index(m[2]), "why": m[3], "to": m[5],
                                **{k: int(v) for k, v in re.findall(r"(\w+): (\d+)", m[4]) if m[3] == "Confirm"}})
    for arg in sys.argv[2:]:
        run, rest = arg.split("=")
        day, F, T = rest.split(",")
        F, T = iso(f"{day}T{F}Z"), iso(f"{day}T{T}Z")
        rows, terms = trace(run + "b33")
        st = [t for t, _ in terms]
        role = lambda t, s: terms[max(0, bisect.bisect_right(st, t) - 1)][1].get(s)
        stalls = {s: sorted(x[0] for x in V if (x[1] == 10 and x[5] >= 2000) or (x[1] == 8 and x[5] >= 1000))
                  for s, V in rows.items()}
        al = alarms[run]
        for a in al:
            a["t"] = iso(f"{day}T{a['at']}Z")
            a["stall"] = any(a["t"] - 10_000 <= x <= a["t"] + 1_000 for x in stalls.get(a["site"], []))
        conf = [a for a in al if a["why"] == "Confirm"]
        moved = lambda xs: sum(a["to"] != "None" for a in xs)
        eps = 0
        for s, V in rows.items():
            sec = sorted({x // 1000 for x in stalls[s] if F <= x < T and role(x, s) == "leader"})
            eps += sum(1 for i, x in enumerate(sec) if i == 0 or x - sec[i - 1] > 10)
        print(f"== {run}: {len(al)} trips (Confirm {len(conf)}, CommitErrors {len(al) - len(conf)}), "
              f"{moved(al)} transfers; {eps} stall episodes while leading in the load window")
        for name, need in RULES.items():
            kept = [a for a in conf if a["slow"] >= need(a["of"])]
            dropped = [a for a in conf if a not in kept]
            print(f"  {name:8} Confirm trips kept {len(kept):2}/{len(conf)}, transfers {moved(kept)}/{moved(conf)}; "
                  f"with a stall before: kept {sum(a['stall'] for a in kept)}, dropped {sum(a['stall'] for a in dropped)}")
        for a in al:
            if a["to"] == "None":
                continue
            fl = sorted((x[0], x[4]) for x in rows[a["site"]] if x[1] == 16)
            ft = [t for t, _ in fl]
            per_s = sum(n for _, n in fl[bisect.bisect_left(ft, a["t"] - 10_000):bisect.bisect_right(ft, a["t"])]) / 10
            print(f"  transfer {a['at']} site {a['site']} {a['why']} -> {a['to']}: {per_s:.0f} flushed stmts/s "
                  f"in the 10 s before; hold-busy (> {BUSY_STMTS}/s) {'holds' if per_s > BUSY_STMTS else 'transfers'}")


if __name__ == "__main__":
    main()
