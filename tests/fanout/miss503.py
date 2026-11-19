#!/usr/bin/env python3
"""miss503.py A1_ROWS.jsonl FROM_ISO TO_ISO -> B0-34's composed-miss waits by phase and the shard /upload-pack 503s by
cause, over every network of the row (B0-34 mt6's split, `tests/am1/results/b034-mt6.md`, "1. Where the clone TTFB
tail goes").

It reads two things from Workers Logs on `$INSTALL-site-{a,b,c}`: every `shard miss <net> <key>: {...}` line (each
miss's `ready_ms` chain wait, `caught_up_ms`, `headers_ms` and `answer`), and every GitShard `/upload-pack` invocation
that answered 503. Phases come from a1's samples file beside the rows file: each phase runs from its first clone
probe's end to its last, and a line falls in the phase where its headers came.

Per phase: misses by answer, the chain wait (`ready_ms`) p50/p99/max, the catch-up wait p99, and the container's own
time to headers p99 on composed answers. Then the 503s:
  cold      answered at once (under 2 s: `warming` while rehydrating, or `chain-wait`'s bound on a forwarded miss),
            or at the deadline with no chain (`ready_ms` null on a network whose chain composes elsewhere) or no
            catch-up (`caught_up_ms` null);
  capacity  at the deadline after the catch-up, queued for the container's stock slots;
  deadline  at the deadline with no miss line to say which.
The account and token come from the environment, the token only as a header.
"""
import collections, datetime, gzip, json, os, re, sys, urllib.request

acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
H = {"authorization": f"Bearer {tok}", "content-type": "application/json"}
P = os.environ.get("INSTALL", "graft-m1")
ms = lambda s: int(datetime.datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
rows, A, B = sys.argv[1], ms(sys.argv[2]), ms(sys.argv[3])
PHASES = ["j9", "kill", "second", "hostile"]


def query(svc, a, b, filters):
    out = []
    while a < b:
        e = min(b, a + 30_000)
        body = {"queryId": "q", "timeframe": {"from": a, "to": e}, "view": "events", "limit": 2000,
                "parameters": {"filters": [{"key": "$metadata.service", "operation": "eq", "type": "string",
                                            "value": svc}] + filters}}
        req = urllib.request.Request(
            f"https://api.cloudflare.com/client/v4/accounts/{acct}/workers/observability/telemetry/query",
            data=json.dumps(body).encode(), headers=H, method="POST")
        for i in range(4):
            try:
                ev = json.load(urllib.request.urlopen(req, timeout=120))["result"]["events"]["events"]
                break
            except Exception as x:
                err = x
        else:
            raise err
        if len(ev) >= 2000:
            print(f"truncated {svc} {a}", file=sys.stderr)
        out += ev
        a = e
    return out


line_re = re.compile(r"shard miss (\d+) ([0-9a-f]*): (\{.*\})")
lines, fails = [], []
for site in "abc":
    svc = f"{P}-site-{site}"
    for e in query(svc, A - 60_000, B + 180_000,
                   [{"key": "$metadata.message", "operation": "includes", "type": "string", "value": "shard miss "}]):
        m = line_re.search(str(e["source"].get("message", "")))
        if m:
            d = json.loads(m.group(3)); d.update(net=m.group(1), key=m.group(2), site=site); lines.append(d)
    for e in query(svc, A, B + 120_000, [{"key": "$workers.event.path", "operation": "eq", "type": "string",
                                          "value": "/upload-pack"},
                                         {"key": "$workers.event.response.status", "operation": "eq",
                                          "type": "number", "value": 503}]):
        w = e["$workers"]
        fails.append({"at": e["timestamp"], "wall": w.get("wallTimeMs") or 0, "site": site})
seen = set()
lines = [d for d in lines if (k := (d["net"], d["key"], d["arrived"], d["site"])) not in seen and not seen.add(k)]

samples = rows.replace("/a1-b0-34-", "/a1-samples-b0-34-") + ".gz"
span = {}
for s in map(json.loads, gzip.open(samples, "rt")):
    if s[1].startswith("read:cloneTtfb@"):
        p, end = s[1].split("@")[1], s[0] + s[2]
        a, b = span.get(p, (end, end)); span[p] = (min(a, end), max(b, end))


def phase(t):
    for p in PHASES:
        if p in span and t <= span[p][1]:
            return p
    return PHASES[-1]


pct = lambda xs, q: sorted(xs)[min(len(xs) - 1, int(q * len(xs)))] if xs else None
inrow = [d for d in lines if A <= d["arrived"] <= B]
by = collections.defaultdict(list)
for d in inrow:
    by[phase(d["arrived"] + (d.get("headers_ms") or 0))].append(d)
print(f"miss lines {len(inrow)} in the window, over {len({d['net'] for d in inrow})} networks")
for p in PHASES:
    v = by[p]
    chain = [d["ready_ms"] for d in v if d.get("ready_ms") is not None]
    cu = [d["caught_up_ms"] for d in v if d.get("caught_up_ms") is not None]
    cont = [d["headers_ms"] - max(d.get("ready_ms") or 0, d.get("caught_up_ms") or 0)
            for d in v if d.get("headers_ms") is not None and d["answer"] in ("composed", "tail")]
    print(json.dumps({"phase": p, "misses": len(v), "answers": dict(collections.Counter(d["answer"][:24] for d in v)),
                      "chain_wait_ms": {"p50": pct(chain, .5), "p99": pct(chain, .99), "max": max(chain, default=None)},
                      "catch_up_p99_ms": pct(cu, .99), "composed_container_to_headers_p99_ms": pct(cont, .99)}))

# A network composes when any of its lines was answered composed or tail; its 503s with no ready_ms waited for a chain.
composes = {d["net"] for d in lines if d["answer"] in ("composed", "tail")}
cls = collections.Counter()
for d in inrow:
    if d["answer"] != "status 503":
        continue
    if d.get("ready_ms") is None and d["net"] in composes and d.get("caught_up_ms") is None:
        cls["deadline: chain wait (cold)"] += 1
    elif d.get("caught_up_ms") is None:
        cls["deadline: catch-up wait (cold)"] += 1
    else:
        cls["deadline: stock slot queue (capacity)"] += 1
fast = sum(f["wall"] < 2_500 for f in fails)
slow = len(fails) - fast
print(json.dumps({"upload_pack_503": len(fails), "at once (cold)": fast, "at the deadline": slow,
                  "deadline split": dict(cls), "deadline with no miss line": max(0, slow - sum(cls.values()))}))
