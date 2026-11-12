#!/usr/bin/env python3
"""B0-02's offline counterfactual: commit p99 under standard Raft's commit rule (`tests/m0/results/carried-5.md`).

Usage: python3 tests/m0carried/majority.py RUN.stats.json [...]

Reads one bench run's `/stats` (`BENCH_RAW=1`, test-faults trace kinds 10 and 12 to 16) and rebuilds, for each entry k
the leader appended, three times on its clock:
  L(k): the leader's own durable copy, the end of the first storage confirm whose transaction began at or after the
        input that appended k (`Stepped`), since the Durable Object confirms in order;
  N(k), F(k): the first AppendEntries answer from the near or far follower whose call carried k (`AppendQueued`'s
        last index at or above k, ended ok), since a follower answers only after its own confirm.
Today's rule commits k at max(L, min(N, F)): the leader commits nothing above its own durable index. Standard Raft's
rule commits at the second of the three: any majority, the leader's copy included or not (Ongaro 10.2.1).

The modelled actual time is checked against the leader's `Applied` trace. Each command's commit time then moves by
its entry's gain, in two ways:
  static: only its own entry commits earlier (a floor on the gain);
  replay: one entry in flight, so the next entry starts when the previous commits; commands are replayed from their
        admission times, each round keeps its measured near/far/leader durations from its start.
"""
import bisect
import json
import sys


def pct(xs, p):
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(p * len(xs)))] if xs else None


def second(a, b, c):
    return sorted((a, b, c))[1]


def run(path):
    d = json.load(open(path))
    # The member that committed the run's commands: the leader, or before a step-down the old leader.
    lead = max((s for s in d if s["r"]), key=lambda s: len(s["r"]["commitMs"]))
    L = lead["r"]
    tr = L["trace"]
    peers = sorted({t[2] for t in tr if t[1] == 12})
    stepped = {}
    for t in tr:
        if t[1] == 15 and t[4] and t[4] not in stepped:
            stepped[t[4]] = t[0]
    confirms = sorted((t[0] - t[5], t[0]) for t in tr if t[1] == 10)
    began = [c[0] for c in confirms]
    queued = {(t[2], t[3]): t for t in tr if t[1] == 12}
    ended = [t for t in tr if t[1] == 14 and t[4] == 0]
    applied = {}
    for t in tr:
        if t[1] == 4 and t[4] not in applied:
            applied[t[4]] = t[0]
    # Each peer's answers: (time, last index carried).
    acks = {p: [] for p in peers}
    for e in ended:
        q = queued.get((e[2], e[3]))
        if q:
            acks[e[2]].append((e[0], q[4]))
    for p in acks:
        acks[p].sort()

    def ack(p, k, after):
        return min((t for t, last in acks[p] if last >= k and t >= after), default=None)

    rows = []
    for k in sorted(stepped):
        s = stepped[k]
        i = bisect.bisect_left(began, s)
        Lk = min((c[1] for c in confirms[i:]), default=None)
        acked = [ack(p, k, s) for p in peers]
        if Lk is None or None in acked or len(acked) != 2:
            continue
        near, far = acked if acks_rtt(L, peers[0]) <= acks_rtt(L, peers[1]) else acked[::-1]
        act = max(Lk, min(near, far))
        rows.append(dict(k=k, s=s, L=Lk, N=near, F=far, act=act, cf=second(Lk, near, far), seen=applied.get(k)))
    good = [r for r in rows if r["seen"] is not None]
    model_err = [r["seen"] - r["act"] for r in good]
    # Static: every command of entry k commits cf(k) - act(k) earlier. Commands map to entries through apply steps.
    steps = L["applySteps"]
    cms = L["commitMs"]
    app_order = [t[4] for t in tr if t[1] == 4]
    by_k = {r["k"]: r for r in rows}
    static = []
    pos = 0
    prev = app_order[0] - steps[0][1] if app_order else 0
    for (ms, n_entries, events), to in zip(steps, app_order):
        ks = range(prev + 1, to + 1)
        prev = to
        gain = min((by_k[k]["seen"] - by_k[k]["cf"] for k in ks if k in by_k and by_k[k]["seen"]), default=0)
        for c in cms[pos : pos + events]:
            static.append(c - max(0, gain))
        pos += events
    static += cms[pos:]
    return dict(
        run=path.split("/")[-1].split(".")[0],
        entries=len(rows),
        model_p50_err=pct([abs(x) for x in model_err], 0.5),
        model_p90_err=pct([abs(x) for x in model_err], 0.9),
        commit_p99=pct(cms, 0.99),
        static_p99=pct(static, 0.99),
        round_act_p99=pct([r["act"] - r["s"] for r in rows], 0.99),
        round_cf_p99=pct([r["cf"] - r["s"] for r in rows], 0.99),
        L_p99=pct([r["L"] - r["s"] for r in rows], 0.99),
        N_p50=pct([r["N"] - r["s"] for r in rows], 0.5),
        F_p50=pct([r["F"] - r["s"] for r in rows], 0.5),
        F_p90=pct([r["F"] - r["s"] for r in rows], 0.9),
        L_gated=sum(1 for r in rows if r["L"] > min(r["N"], r["F"])),
        cf_helps=sum(1 for r in rows if r["cf"] < r["act"]),
        replay_p99=replay(L, rows),
    )


def acks_rtt(L, p):
    a = L["appendMs"].get(str(p), [])
    return pct(a, 0.5) if a else 10**9


def replay(L, rows):
    """Commands from their admission times through one-in-flight rounds whose leader, near, and far durations are
    the measured ones; under the majority rule a round ends at the second of the three."""
    tr = L["trace"]
    cms = L["commitMs"]
    steps = L["applySteps"]
    app_times = [t[0] for t in tr if t[1] == 4]
    # Admission times: each command resolved at its apply step's time minus its commit_ms.
    adm = []
    pos = 0
    for (ms, n, events), t in zip(steps, app_times):
        adm += [t - c for c in cms[pos : pos + events]]
        pos += events
    adm.sort()
    durs = [(r["L"] - r["s"], r["N"] - r["s"], r["F"] - r["s"]) for r in rows]
    if not durs or not adm:
        return None

    def go(rule):
        out, i, t, j = [], 0, adm[0], 0
        while i < len(adm):
            t = max(t, adm[i])
            batch = bisect.bisect_right(adm, t) - i
            l, n, f = durs[min(j, len(durs) - 1)]
            end = t + (second(l, n, f) if rule == "cf" else max(l, min(n, f)))
            out += [end - a for a in adm[i : i + batch]]
            i += batch
            j += 1
            t = end
        return pct(out, 0.99)

    return dict(actual=go("act"), majority=go("cf"))


if __name__ == "__main__":
    for p in sys.argv[1:]:
        print(json.dumps(run(p)))
