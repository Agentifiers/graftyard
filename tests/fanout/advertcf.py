#!/usr/bin/env python3
"""advertcf.py TAG A1_PUSHES.jsonl A1_ROW.jsonl EDGEFULL.jsonl.xz: edge-refusal-bound's counterfactual on a row B run
(`tests/am1/results/edge-refusal.md`, "The fail-open bound and the send check: design"). Run from the repository root.

It rebuilds the leader's advert from its own cuts and replays every agent push against two rules:
  rule 1 (the fail-open bound): once an isolate's advert that said full lapses, one Agent write a chain an isolate goes
         through per ADVERT_TTL (the probe) and the rest are refused, until ADVERT_STALE with no advert;
  rule 2 (the send check): the advert is read again at each commit send against what is left of the push's bound,
         not only at `receive_pack`, before push context, validate and any Stage round spend it.
and splits the leader's refusals the edge sent within budget by why the advert under-read them.

The advert at real time t (`wait`) is `expected_wait` from the cuts (`Trace::Cut`): the agent lane after the last cut,
interpolated to t by that interval's admitted arrivals, over the leader's `Drain` (saturated time only, DRAIN_WINDOW,
DRAIN_MIN), plus a round (a1's `/status` `roundMs`). The cuts are moved to Cloudflare's clock by the leader's measured
offset per 10 s (the voters' `CallLeft` against the leader's `AnswerArrived`, net of the idle wire, as
`b033probes/host/drift.py`). An edge reads the advert AGE ms (default 1,000) after the leader sent it.

Per push, from a1's `server-timing`: the check at `receive_pack` (its budget is the first `budget` plus context,
stagefirst and validate), each commit send (`budget` at the first, less the commit and stage stages before the last),
and the leader's answer. A push the leader refused with its last commit stage under 600 ms was refused at admission,
else it expired queued (`pushfull.py`). Fail-open pushes are matched to their `edgefull` lines by budget and time.

First order only: a refused push still leaves the queue the others met as it was, so counts of in-time pushes refused
are upper bounds [inference]. Pass AGE=2000 in the environment for an older advert.
"""
import bisect, collections, json, lzma, os, re, sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "b033probes", "host"))
import agentdrain  # noqa: E402
import common  # noqa: E402

AGE = int(os.environ.get("AGE", "1000"))
TTL, STALE = 2_000, 10_000
DRAIN_WINDOW, DRAIN_MIN, ROUND_MIN = 10_000, 1_000, 50.0
ADMIT_MS, PFA = 600, 8_463
pc = lambda xs, q: round(sorted(xs)[min(len(xs) - 1, int(q * len(xs)))]) if xs else None


def offsets(tag):
    """{10 s window of the leader's clock: leader minus Cloudflare ms}, and the idle default 0."""
    a, _, rows = common.load(tag, sites=(0, 1, 2), pad=60_000)
    I = common.I
    sent = {(r[2], I(r[3])): r[0] for r in rows[0] if r[1] == 23 and r[2] < 192}
    ans = {(r[2], I(r[3])): r[0] for r in rows[0] if r[1] == 21 and r[2] < 192}
    pts = []
    for f in (1, 2):
        left = {I(r[3]): r[0] for r in rows[f] if r[1] == 24 and r[2] == 0}
        pts += [(t, ans[(p, c)] - left[c]) for (p, c), t in sent.items() if p == f and c in left and (p, c) in ans]
    idle = pc([b for t, b in pts if t < a], .5)
    by = collections.defaultdict(list)
    for t, b in pts:
        by[t // 10_000].append(b - idle)
    return {k: pc(v, .5) for k, v in by.items()}


class Advert:
    def __init__(self, tag, row):
        off = offsets(tag)
        cs = agentdrain.cuts(tag)
        # (Cloudflare time, agent taken, agent left, leader time)
        self.c = sorted((at - off.get(at // 10_000, 0), tk[3], lf[3], at) for at, tk, lf, _, _ in cs)
        self.t = [x[0] for x in self.c]
        self.runs, last = [], None
        for _, tk, lf, tl in self.c:
            if last is not None and last[1]:
                self.runs.append((tl, tk, tl - last[0]))
            last = (tl, lf > 0)
        self.rt = [r[0] for r in self.runs]
        adm = next(x for x in reversed([json.loads(l) for l in open(row)]) if "admission" in x)["admission"]
        t0 = common.ms(common.RUNS[tag][0])
        self.adm = [(t0 + x["s"] * 1000, x["roundMs"]) for x in adm]

    def i(self, t):
        return bisect.bisect_right(self.t, t) - 1

    def queue(self, t):
        i = self.i(t)
        if i < 0:
            return 0
        if i + 1 >= len(self.c):
            return self.c[i][2]
        a, b = self.c[i], self.c[i + 1]
        return a[2] + max(0, b[1] + b[2] - a[2]) * (t - a[0]) / max(1, b[0] - a[0])

    def drain(self, t):
        i = self.i(t)
        if i < 0:
            return None
        tl = self.c[i][3]
        j, k = bisect.bisect_left(self.rt, tl - DRAIN_WINDOW), bisect.bisect_right(self.rt, tl)
        n, ms = sum(r[1] for r in self.runs[j:k]), sum(r[2] for r in self.runs[j:k])
        return n * 1000 / ms if ms >= DRAIN_MIN else None

    def round(self, t):
        return self.adm[max(0, bisect.bisect_right([a[0] for a in self.adm], t) - 1)][1]

    def wait(self, t, q=None, d=None):
        if self.i(t) < 0:
            return None
        d = self.drain(t) if d is None else d
        q = self.queue(t) if q is None else q
        return self.round(t) if d is None else q / max(d, 1) * 1000 + max(self.round(t), ROUND_MIN)


def pushes(path):
    out = []
    for line in open(path):
        p = json.loads(line)
        s = [(n, int(d)) for n, d in re.findall(r"([a-z_]+);dur=(\d+)", p.get("timing") or "")]
        if p.get("kind") != "agent" or "budget" not in [n for n, _ in s]:
            continue
        commits = [v for n, v in s if n == "commit"]
        stage = sum(v for n, v in s if n == "stage")
        total = next((v for n, v in s if n == "total"), 0)
        rtt = (p["end"] - p["start"]) - total
        b1 = next(v for n, v in s if n == "budget")
        pre = sum(v for n, v in s if n in ("context", "validate", "stagefirst"))
        last = p["end"] - rtt / 2 - commits[-1]
        p.update(commits=commits, b1=b1, bcheck=b1 + pre, tcheck=last - sum(commits[:-1]) - stage - pre,
                 tfirst=last - sum(commits[:-1]) - stage, tlast=last,
                 blast=b1 - (sum(commits[:-1]) + stage) if len(commits) > 1 else b1,
                 o="ok" if p["result"] == "ok" else ("adm" if commits[-1] < ADMIT_MS else "exp")
                 if "full" in p["result"] else "other", parts={n: sum(v for m, v in s if m == n) for n in
                                                                ("context", "validate", "stage")}, fo=False)
        out.append(p)
    return out


def main(tag, path, row, ef):
    adv = Advert(tag, row)
    P = [p for p in pushes(path) if p["o"] != "other"]
    lines = sorted((json.loads(x) for x in lzma.open(ef, "rt")), key=lambda x: x["t"])
    # Fail-open pushes: each `failOpen` push line to the sent push with its budget, within 1.5 s.
    byc = sorted(P, key=lambda p: p["tcheck"])
    tc, used = [p["tcheck"] for p in byc], set()
    for x in lines:
        if x["edgefull"] != "failOpen" or not x["push"]:
            continue
        lo, hi = bisect.bisect_left(tc, x["t"] - 1500), bisect.bisect_right(tc, x["t"] + 1500)
        best = min(((abs(byc[k]["bcheck"] - x["budgetMs"]) + abs(byc[k]["tcheck"] - x["t"]) / 10, k)
                    for k in range(lo, hi) if k not in used), default=None)
        if best and best[0] < 300:
            used.add(best[1])
            byc[best[1]]["fo"] = True
            x["push_"] = byc[best[1]]
    # Rule 1: per isolate, a fail-open is the probe if TTL has passed since that isolate's last probe, and is held
    # (refused) otherwise, unless the isolate's last refusal (a lower bound on its advert's age) is over STALE old.
    held = set()
    last_ref, last_probe = {}, {}
    for x in lines:
        i = x["isolate"]
        if x["edgefull"] == "refused":
            last_ref[i] = x["t"]
            continue
        if x["t"] - last_probe.get(i, -1e18) >= TTL or x["t"] - last_ref.get(i, -1e18) > STALE:
            last_probe[i] = x["t"]
        elif "push_" in x:
            held.add(id(x["push_"]))

    def send_check(p, margin=0):
        pts = [(p["tfirst"], p["b1"])] + ([(p["tlast"], p["blast"])] if len(p["commits"]) > 1 else [])
        return any((w := adv.wait(t - AGE)) is not None and w > b - margin for t, b in pts)

    def run(r1, r2, margin=0, strict=False):
        lead, intime, edge = collections.Counter(), 0, 0
        for p in P:
            # strict: no credit for a push the replay refuses at the check while the edge sent it (replay error).
            miss = strict and (w := adv.wait(p["tcheck"] - AGE)) is not None and w > p["bcheck"]
            if (r1 and p["fo"] and id(p) in held) or (r2 and not p["fo"] and not miss and send_check(p, margin)):
                edge += 1
                intime += p["o"] == "ok"
            elif p["o"] != "ok":
                lead[p["o"]] += 1
        return sum(lead.values()), lead, intime, edge

    ok = sum(1 for p in P if p["o"] == "ok")
    print(f"{tag}: agent pushes sent {len(P)} (ok {ok}), fail-open pushes matched {sum(p['fo'] for p in P)}, "
          f"advert age {AGE} ms, ADVERT_TTL {TTL}, ADVERT_STALE {STALE}")
    # Replay against the edge: the edge's own refusals and sends.
    refused = [x for x in lines if x["edgefull"] == "refused" and x["push"]]
    agree_r = sum(1 for x in refused if (adv.wait(x["t"] - AGE) or 0) > x["budgetMs"])
    sent = [p for p in P if not p["fo"]]
    agree_s = sum(1 for p in sent if (adv.wait(p["tcheck"] - AGE) or 0) <= p["bcheck"])
    print(f"replay check: refuses {agree_r} of the edge's {len(refused)} advert-refused pushes, sends {agree_s} of "
          f"its {len(sent)} sent ones")
    # Why the leader refused pushes the edge sent within budget.
    why = collections.Counter()
    spent = collections.defaultdict(list)
    for p in sent:
        if p["o"] == "ok":
            continue
        ta = p["tlast"] + (150 if p["commits"][-1] >= ADMIT_MS else min(p["commits"][-1], 1200) / 2)
        wc = adv.wait(p["tcheck"] - AGE)
        if wc is None:
            why["no advert"] += 1
        elif wc > p["bcheck"]:
            why["0 replay refuses at the check (replay error)"] += 1
        elif wc > p["blast"]:
            why["1 its own budget spent after the check, at the same advert"] += 1
            for k in ("context", "validate", "stage"):
                spent[k].append(p["parts"][k])
            spent["all"].append(p["bcheck"] - p["blast"])
        elif adv.wait(ta, d=adv.drain(p["tcheck"] - AGE)) > p["blast"]:
            why["2 the lane grew before it arrived (commands already sent)"] += 1
        elif adv.wait(ta) > p["blast"]:
            why["3 the trailing drain fell before it arrived"] += 1
        else:
            why[f"4 within budget at arrival ({p['o']})"] += 1
    n = sum(why.values())
    print(f"leader-refused pushes the edge sent on a live advert: {n}")
    for k in sorted(why):
        print(f"  {k}: {why[k]} ({why[k] / n:.0%})")
    print("  budget spent after the check in cause 1, ms p50 / p90: "
          + ", ".join(f"{k} {pc(v, .5)} / {pc(v, .9)}" for k, v in spent.items()))
    print(f"counterfactual, leader refusals (admission + expire) against pfA's {PFA:,}:")
    for name, args in [("as run", (False, False)), ("rule 1, the fail-open bound", (True, False)),
                       ("rule 2, the send check", (False, True)), ("rules 1 and 2", (True, True)),
                       ("rules 1 and 2, no credit where the replay disagreed", (True, True, 0, True)),
                       ("rules 1 and 2, send check 500 ms stricter", (True, True, 500)),
                       ("rules 1 and 2, send check 500 ms looser", (True, True, -500))]:
        m, lead, intime, edge = run(*args)
        print(f"  {name}: {m} ({m / PFA:.0%}; admission {lead['adm']}, expire {lead['exp']}); edge refuses {edge} "
              f"more, {intime} of them in-time (ok) pushes")


if __name__ == "__main__":
    if len(sys.argv) != 5:
        raise SystemExit(__doc__)
    main(*sys.argv[1:])
