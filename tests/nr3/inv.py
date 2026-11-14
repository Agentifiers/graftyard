#!/usr/bin/env python3
"""inv.py REPO_ID FROM_ISO TO_ISO TAG -> each member invocation of `repo:REPO_ID` in the window, from Workers Logs:
its CPU and wall (the invocation event) joined by `requestId` with what it caused (`workers/site/src/inv.rs`: the
`{"inv": path}` line and any `{"tail": id}` lines), one JSON line each in tests/nr3/raw/inv-TAG.jsonl; then, per
member, the split of the invocations of 50 ms of CPU or more by cause.

The split is a least-squares fit, per member, of CPU on the counts (entries applied, KB decoded, statements and KB
committed, answers built by kind, cached answers, fold SQL reads), over every invocation with counts. Each cause's
share of the heavy invocations' CPU is its coefficient times its count there, over their CPU; what the fit leaves is
`unexplained`. Needs CLOUDFLARE_ACCOUNT_ID and CLOUDFLARE_API_TOKEN (Observability read); the token goes in a header
only.
  python3 tests/nr3/inv.py 65564 2026-11-14T17:00:00Z 2026-11-14T17:05:00Z inv1
"""
import collections, json, os, sys, urllib.request
from datetime import datetime

acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
H = {"authorization": f"Bearer {tok}", "content-type": "application/json"}
rid, a, b, tag = sys.argv[1:5]
ms = lambda s: int(datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
P = os.environ.get("INSTALL", "graft-m3e")
URL = f"https://api.cloudflare.com/client/v4/accounts/{acct}/workers/observability/telemetry/query"
CAUSES = ["applied", "decodedKb", "rows", "rowKb", "dumps", "rest", "gql", "hits", "sql_reads"]
HEAVY = 50


def post(url, body):
    return json.load(urllib.request.urlopen(urllib.request.Request(url, data=json.dumps(body).encode(), method="POST",
                                                                   headers=H), timeout=90))


def names():
    q = """query($a:String!,$s:Time!,$e:Time!,$n:String!){viewer{accounts(filter:{accountTag:$a}){
     p: durableObjectsPeriodicGroups(limit: 100, filter:{datetime_geq:$s, datetime_lt:$e, name_like:$n}) {
     dimensions { name objectId } } }}}"""
    v = post("https://api.cloudflare.com/client/v4/graphql", {"query": q, "variables": {"a": acct, "s": a, "e": b,
                                                                                        "n": f"repo:{rid}#%"}})
    out = {}
    for r in v["data"]["viewer"]["accounts"][0]["p"]:
        n = r["dimensions"]["name"]
        # `repo:ID#1#f0` a voter, `repo:ID#2#l3` a learner.
        out[r["dimensions"]["objectId"]] = n.split("#")[-1] if "#" in n else n
    return out


def window(script, oid, lo, hi, out):
    body = {"queryId": f"inv-{lo}", "timeframe": {"from": lo, "to": hi}, "view": "invocations", "limit": 2000,
            "parameters": {"filters": [{"key": "$metadata.service", "operation": "eq", "type": "string", "value": script},
                                       {"key": "$workers.durableObjectId", "operation": "eq", "type": "string",
                                        "value": oid}]}}
    inv = post(URL, body)["result"].get("invocations", {})
    if len(inv) >= 1900 and hi - lo > 500:
        mid = (lo + hi) // 2
        return window(script, oid, lo, mid, out) + window(script, oid, mid, hi, out)
    for req, evs in inv.items():
        row = out.setdefault(req, {"c": collections.Counter()})
        for e in evs:
            w, src = e.get("$workers", {}), e.get("source", {})
            if e.get("$metadata", {}).get("type") == "cf-worker-event":
                row.update(cpu=w.get("cpuTimeMs", 0), wall=w.get("wallTimeMs", 0), at=e["timestamp"],
                           path=w.get("event", {}).get("request", {}).get("url", "").replace("https://member", ""),
                           outcome=w.get("outcome"))
            if "inv" in src or "tail" in src:
                for k, v in (src.get("c") or {}).items():
                    row["c"][k] += v
                if "inv" in src:
                    row.update(pend=src.get("pendStart", 0), pendEnd=src.get("pendEnd", 0))
                else:
                    row["tails"] = row.get("tails", 0) + 1
    return len(inv)


def solve(X, y):
    """Least squares by the normal equations with a small ridge, no numpy; coefficients clamped at 0 and refit."""
    k = len(X[0])
    active = list(range(k))
    for _ in range(k):
        A = [[sum(r[i] * r[j] for r in X) + (1e-6 if i == j else 0) for j in active] for i in active]
        v = [sum(r[i] * t for r, t in zip(X, y)) for i in active]
        n = len(active)
        M = [A[i] + [v[i]] for i in range(n)]
        for c in range(n):
            p = max(range(c, n), key=lambda r: abs(M[r][c]))
            M[c], M[p] = M[p], M[c]
            if abs(M[c][c]) < 1e-12:
                continue
            for r in range(n):
                if r != c:
                    f = M[r][c] / M[c][c]
                    M[r] = [x - f * z for x, z in zip(M[r], M[c])]
        sol = [M[i][n] / M[i][i] if abs(M[i][i]) > 1e-12 else 0 for i in range(n)]
        if all(s >= 0 for s in sol):
            out = [0.0] * k
            for i, s in zip(active, sol):
                out[i] = s
            return out
        active = [i for i, s in zip(active, sol) if s > 0 or i == k - 1]
    return [0.0] * k


ids = names()
rows = {}
for oid, nm in sorted(ids.items(), key=lambda kv: kv[1]):
    for sc in "abc":
        n = sum(window(f"{P}-site-{sc}", oid, t, min(ms(b), t + 10_000), rows) for t in range(ms(a), ms(b), 10_000))
        if n:
            for r in rows.values():
                r.setdefault("member", nm)
            break
os.makedirs("tests/nr3/raw", exist_ok=True)
with open(f"tests/nr3/raw/inv-{tag}.jsonl", "w") as f:
    for req, r in rows.items():
        if "cpu" in r:
            f.write(json.dumps({"req": req, **{k: v for k, v in r.items() if k != "c"}, "c": dict(r["c"])}) + "\n")


def feats(c):
    return [c.get("applied", 0), c.get("decoded", 0) / 1024, c.get("rows", 0), c.get("row_bytes", 0) / 1024,
            c.get("dumps", 0), c.get("rest", 0), c.get("gql", 0), c.get("hits", 0), c.get("sql_reads", 0), 1.0]


by = collections.defaultdict(list)
for r in rows.values():
    if "cpu" in r:
        by[r["member"]].append(r)
for m in sorted(by):
    rs = by[m]
    counted = [r for r in rs if sum(r["c"].values())]
    coef = solve([feats(r["c"]) for r in counted], [r["cpu"] for r in counted]) if len(counted) > 20 else [0] * 10
    heavy = [r for r in rs if r["cpu"] >= HEAVY]
    hc = sum(r["cpu"] for r in heavy) or 1
    share = {c: round(100 * coef[i] * sum(feats(r["c"])[i] for r in heavy) / hc) for i, c in enumerate(CAUSES)}
    share["base"] = round(100 * coef[-1] * len(heavy) / hc)
    share["unexplained"] = 100 - sum(share.values())
    tot = sum(r["cpu"] for r in rs)
    paths = collections.Counter(r["path"] for r in heavy)
    print(json.dumps({"member": m, "invocations": len(rs), "withCounts": len(counted), "cpuS": round(tot / 1000, 1),
                      "heavy": len(heavy), "heavyCpuS": round(hc / 1000, 1), "heavyPaths": dict(paths.most_common(4)),
                      "msPer": dict(zip(CAUSES + ["base"], [round(x, 2) for x in coef])), "heavyShare": share,
                      "heavyMean": {c: round(sum(feats(r["c"])[i] for r in heavy) / max(1, len(heavy)), 1)
                                    for i, c in enumerate(CAUSES)},
                      "heavyPendStartP50": sorted(r.get("pend", 0) for r in heavy)[len(heavy) // 2] if heavy else None}))
