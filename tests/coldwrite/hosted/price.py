#!/usr/bin/env python3
"""price.py RAW.jsonl -> what each proposed cut would have made of a cell's cold first POSTs (`decisions/DECISIONS.md`,
"`cold-write`", amended 2026-11-23, cw3), from the cell's own `server-timing` and `x-graft-spread` marks.

Cold first POSTs are found as `judge.py` finds them (SINCE and the Workers Logs environment, as there). Per POST:
- `install` minus its first `authz:1` pass is the `Placement` read the install waited on before reading `authz:1`
  again (`link` drops a guess that is not the answer);
- the `authz:7100001`-style owner pass is `resolve`'s last read, and its `nl` tries are member answers that served
  nothing.

Levers, each a model, not a measurement:
- **1, the install guess kept on the same members:** `install` becomes max(Placement read, `authz:1` pass).
- **2, no `nl` try on the owner pass:** `resolve` loses its `nl` tries' ms (an upper bound for any routing change).
"""
import importlib.util
import json
import os
import sys

here = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("judge_mod", os.path.join(here, "judge.py"))
judge = importlib.util.module_from_spec(spec)
spec.loader.exec_module(judge)

rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
logged = judge.pushes(judge.ms(os.environ["SINCE"]), judge.ms(max(r["at"] for r in rows)) + 60_000)
first = {}
for client, (iso, _, at) in sorted(logged.items(), key=lambda kv: kv[1][2]):
    first.setdefault(iso, client)
cold_clients = set(first.values())
cold = [r for r in rows if r["step"].startswith("post") and r["client"] in cold_clients]


def passes(r):
    """The spread passes in order: (ms, chain, [tries])."""
    out = []
    for p in (r.get("spread") or "").split("; "):
        bits = p.split()
        if len(bits) >= 3 and bits[0].isdigit():
            out.append((int(bits[0]), bits[1], bits[2:]))
    return out


def cuts(r):
    st, ps = r["st"], passes(r)
    install_pass = next(ms for ms, ch, _ in ps if ch == "authz:1")
    owner = next(t for _, ch, t in ps if ch != "authz:1" and ch.startswith("authz:"))
    nl_ms = sum(int(t.split(":")[1]) for t in owner if t.endswith(":nl"))
    placement = st["install"] - install_pass
    install_1 = max(placement, install_pass)
    resolve_2 = st["resolve"] - nl_ms
    g = st["gate"]
    return {
        "now": st["front"] + g,
        "lever 1": max(st["resolve"], install_1) + g,
        "lever 2": max(resolve_2, st["install"]) + g,
        "1 and 2": max(resolve_2, install_1) + g,
        "placement": placement,
    }


each = [(r, cuts(r)) for r in cold]
print(f"cold first POSTs: {len(cold)}; front = max(resolve, install) on "
      f"{sum(r['st']['front'] == max(r['st']['resolve'], r['st']['install']) for r in cold)}, "
      f"= resolve + install on {sum(r['st']['front'] == r['st']['resolve'] + r['st']['install'] for r in cold)}")
print(judge.line("install's Placement read before its authz:1 read again", [c["placement"] for _, c in each]))
for arm, keep in (("all", lambda r: True), ("git", lambda r: r["arm"] == "git"),
                  ("fetch, other, ro", lambda r: r["arm"] != "git")):
    for k in ("now", "lever 1", "lever 2", "1 and 2"):
        print(judge.line(f"{arm}: front+gate, {k}", [c[k] for r, c in each if keep(r)]))
