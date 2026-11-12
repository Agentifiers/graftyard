#!/usr/bin/env python3
"""summary.py ROWS.jsonl... : one line per learner-dumps run, from a1.mjs's b0-29 row: read:api and read:ref outcomes,
agent pushes, the leader's moves, and the feed's lagging answers."""
import json, sys

for path in sys.argv[1:]:
    for line in open(path):
        r = json.loads(line)
        if r.get("row") != "b0-29":
            continue
        k = r["byKind"]
        out = {"run": r["run"], "seconds": r["seconds"]}
        for kind in ("read:api", "read:ref", "push:agent"):
            if kind in k:
                c = k[kind]["codes"]
                ok = sum(v for x, v in c.items() if x in ("200", "ok"))
                shed = sum(v for x, v in c.items() if "503" in x)
                skipped = sum(v for x, v in c.items() if x.startswith("skipped"))
                answered = sum(c.values()) - skipped
                out[kind] = dict(n=k[kind]["n"], ok=ok, e503=shed, skipped=skipped,
                                 okShare=round(ok / answered, 3) if answered else None, p50=k[kind]["p50"],
                                 p99=k[kind]["p99"], perS=k[kind]["perS"])
        out["feedLagging"] = k.get("feed", {}).get("codes", {}).get("lagging", 0)
        out["leader"] = {x: r.get("leader", {}).get(x) for x in ("before", "after")}
        print(json.dumps(out))
