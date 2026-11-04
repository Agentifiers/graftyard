#!/usr/bin/env python3
"""sum3.py RATE [RAW]: BF-20's apply clause per phase of bf20c.sh (a = baseline before, s = snapshot running, z =
baseline after) and of bf20d.sh (t = snapshot run to its commit). Pools every sample of the phase's runs: the apply step (`applySteps`, ms per applying input, every
member), the entry age at apply on the leader (`applyMs` of the member with `roundMs`) and on each follower, and the
commands' outcome latency (`latMs`). Prints n, p50, p99, and a 95% bootstrap interval of p99 for each, then the
snapshot phases' p99 against the pooled baselines'."""
import json, random, sys

rate, raw = sys.argv[1], (sys.argv[2] if len(sys.argv) > 2 else "/tmp/bf20/raw")


def q(a, p):
    a = sorted(a)
    return a[min(len(a) - 1, int(len(a) * p))] if a else None


def ci(a, p=0.99, n=200):
    """95% bootstrap interval of the p-quantile: n resamples of the sorted sample, drawn as order statistics."""
    if not a:
        return (None, None)
    s, r = sorted(a), random.Random(1)
    b = sorted(s[min(len(s) - 1, int(sorted(r.randrange(len(s)) for _ in s)[min(len(s) - 1, int(len(s) * p))]))] for _ in range(n))
    return (b[int(n * 0.025)], b[int(n * 0.975)])


def phase(tag):
    out = {"runs": 0, "ok": 0, "sent": 0, "refused": 0, "step": [], "leader": [], "follower": [], "cmd": []}
    for k in range(1, 100):
        n = f"c{rate}{tag}{k}"
        try:
            L = json.load(open(f"{raw}/{n}.load.json")).get("v", {})
            st = [json.load(open(f"{raw}/{n}.stats{s}.json"))["v"] for s in range(3)]
        except (OSError, ValueError):
            continue
        if not isinstance(L, dict) or "ok" not in L:
            continue
        out["runs"] += 1
        out["ok"] += L["ok"]
        out["sent"] += L["sent"]
        out["refused"] += sum((L.get("refused") or {}).values()) + sum((L.get("other") or {}).values())
        out["cmd"] += L.get("latMs", [])
        lead = max((x for x in st if x.get("roundMs")), key=lambda x: len(x["roundMs"]), default=None)
        out.setdefault("leaders", []).append(lead["site"] if lead else None)
        for x in st:
            out["step"] += [s[0] for s in x.get("applySteps", [])]
            (out["leader"] if x is lead else out["follower"]).extend(x.get("applyMs", []))
    return out


ph = {t: phase(t) for t in "aszt"}
base = {k: ph["a"][k] + ph["z"][k] for k in ("step", "leader", "follower", "cmd")}
print(f"rate {rate}/s")
names = (("a", "baseline before"), ("s", "snapshot running"), ("z", "baseline after"), ("t", "snapshot to commit"))
for t, name in names:
    p = ph[t]
    print(f"{name}: {p['runs']} runs, {p['ok']}/{p['sent']} ok, {p['refused']} refused or failed, leaders {p.get('leaders')}")
    for k in ("step", "leader", "follower", "cmd"):
        lo, hi = ci(p[k])
        print(f"  {k:8} n={len(p[k]):6} p50={q(p[k], .5)} p99={q(p[k], .99)} p99 95% CI [{lo}, {hi}]")
for t, name in names[1::2]:
    print(f"{name} p99 against both baselines pooled:")
    for k in ("step", "leader", "follower", "cmd"):
        b, s = q(base[k], .99), q(ph[t][k], .99)
        lo, hi = ci(base[k])
        d = f"{(s - b) / b * 100:+.1f}%" if b and s is not None else ("0 vs 0" if s == 0 else "n/a")
        print(f"  {k:8} baseline p99={b} (CI [{lo}, {hi}], n={len(base[k])}) {name} p99={s} (n={len(ph[t][k])}) {d}")
