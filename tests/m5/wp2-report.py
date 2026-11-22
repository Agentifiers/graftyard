#!/usr/bin/env python3
"""M5-WP2's hosted numbers from tests/m5/wp2-hosted.sh's raw rows (default tests/m5/results/raw/wp2).

    python3 tests/m5/wp2-report.py [raw dir] [--n <version N>]

BF-43: from the undo's 201, the time until every front request reports N; requests off N after that while pinned; the
pin each request carried after the next EdgeRoll. BF-8: requests during the blackhole (status, latency, pin), and the
first alarm after the pointer named a version outside the deployment. BF-48: per member, the first stale() answer after
each flag, the last one, and the flags' bounds.
"""
import json
import sys
from pathlib import Path

args = [a for i, a in enumerate(sys.argv[1:]) if not a.startswith("--") and not sys.argv[i].startswith("--")]
D = Path(args[0] if args else "tests/m5/results/raw/wp2")
OFFSET = int(sys.argv[sys.argv.index("--offset") + 1]) if "--offset" in sys.argv else 0
N = sys.argv[sys.argv.index("--n") + 1] if "--n" in sys.argv else "3ba676ce-5cc0-41d8-b7fe-db03e3febfa7"
# The status a sampled request answers when it reaches the edge: 200 on /api/v3, 405 on M5-WP3's
# /api/graftyard/v1/release/entries GET, which carries the edge version whatever the edge's HOST.
OK = int(sys.argv[sys.argv.index("--ok") + 1]) if "--ok" in sys.argv else 200


def rows(name):
    p, gz = D / name, D / (name + ".gz")
    if gz.exists():
        import gzip
        return [json.loads(l) for l in gzip.open(gz, "rt") if l.strip()]
    return [json.loads(l) for l in p.read_text().splitlines() if l.strip()] if p.exists() else []


events = rows("events.jsonl")
answers = {a["label"]: a for a in rows("answers.jsonl")}


def ev(name, after=0):
    return next((e["t"] for e in events if e["event"] == name and e["t"] >= after), None)


def pct(xs, q):
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(q * len(xs)))] if xs else None


def bf43():
    r = rows("bf43.jsonl")
    if not r:
        return
    undo = answers["bf43-undo"]["t1"]
    roll2 = answers["bf43-roll-n2"]["t1"]
    ok = [x for x in r if x["s"] == OK]
    print(f"BF-43: {len(r)} requests, {len(ok)} {OK}")
    before = [x for x in ok if x["t0"] < undo]
    off = [x for x in before if x["ran"] != N]
    print(f"  before the undo: {len(before)} requests, {len(off)} on N+1 ({100 * len(off) / max(1, len(before)):.2f}%)")
    pinned = [x for x in ok if undo <= x["t0"] < roll2]
    # The time until every request reports N: the start of the last request off N after the undo.
    late = [x for x in pinned if x["ran"] != N]
    last_off = max((x["t0"] for x in late), default=None)
    first_pin = min((x["t0"] for x in pinned if x["pin"] != "none"), default=None)
    print(f"  undo 201 at +0; first request carrying the pin at +{(first_pin - undo) if first_pin else None} ms")
    print(f"  requests off N after the undo: {len(late)}; last one started at +{(last_off - undo) if last_off else 0} ms")
    after_ttl = [x for x in pinned if x["t0"] >= undo + 5000 and (x["ran"] != N or x["pin"] == "none")]
    print(f"  requests from +5 s to the next roll: {sum(1 for x in pinned if x['t0'] >= undo + 5000)}, off N or unpinned: {len(after_ttl)}")
    blk = (ev("api-blocked"), ev("api-unblocked"))
    inb = [x for x in pinned if blk[0] and blk[0] <= x["t0"] < blk[1]]
    print(f"  with the API blocked ({(blk[1] - blk[0]) / 1000:.0f} s): {len(inb)} requests, {sum(x['ran'] == N for x in inb)} on N")
    after = [x for x in ok if x["t0"] >= roll2]
    still = [x for x in after if x["t0"] >= roll2 + 5000 and x["pin"] != "none"]
    lastpin = max((x["t0"] for x in after if x["pin"] != "none"), default=None)
    print(f"  after the next EdgeRoll's 201: {len(after)} requests; last pinned one at +{(lastpin - roll2) if lastpin else 0} ms;"
          f" pinned after +5 s: {len(still)}")
    split = ev("deployed", roll2)
    n2 = [x for x in after if split and x["t0"] > split + 30_000]
    print(f"  under the second 1% split: {len(n2)} requests, {sum(x['ran'] != N for x in n2)} on N+2")
    print(f"  alarms: {sum(x.get('alarm') == '1' for x in ok)}")


def bf8():
    r = rows("bf8.jsonl")
    if not r:
        return
    b0, b1 = ev("blackhole"), ev("blackhole-off")
    pin = answers["bf8-pin-n"]["t1"]
    print(f"BF-8: {len(r)} requests")
    inb = [x for x in r if b0 <= x["t0"] < b1]
    lat = [x["t1"] - x["t0"] for x in inb]
    print(f"  (A) blackhole {(b1 - b0) / 1000:.0f} s: {len(inb)} requests, {sum(x['s'] == 200 for x in inb)} 200,"
          f" {sum(x['pin'] == N for x in inb if x['s'] == 200)} carried the pin, {sum(x.get('ran') == N for x in inb)} ran N,"
          f" p50 {pct(lat, .5)} ms p99 {pct(lat, .99)} ms max {max(lat) if lat else None} ms")
    sent = [x for x in inb if x["s"] == 0]
    if sent:
        print(f"  not answered (the sampler's fetch failed, no front answer): {len(sent)}, from"
              f" +{(min(x['t0'] for x in sent) - b0) / 1000:.1f} s to +{(max(x['t0'] for x in sent) - b0) / 1000:.1f} s,"
              f" {sorted({x.get('err', '')[:40] for x in sent})}")
    views = [x["view"] for x in inb if x.get("view")]
    print(f"  views failing during it: {sum('failing=true' in v for v in views)} of {len(views)}")
    rr = [x for x in rows("bf8-repo.jsonl") if b0 <= x["t0"] < b1]
    rl = [x["t1"] - x["t0"] for x in rr]
    print(f"  repository reads during it: {len(rr)}, {sum(x['s'] == 200 for x in rr)} 200, p50 {pct(rl, .5)} ms max {max(rl) if rl else None} ms")
    refused = answers.get("bf8-roll-drops-undo")
    if refused:
        print(f"  (B) EdgeRoll whose undo names N+2: {json.dumps(refused['answer'])[:200]}")
    po = answers.get("bf8-pin-outside")
    if po:
        t = po["t1"]
        restored = ev("restored-n", t)
        al = [x for x in r if x["t0"] >= po["t0"] and x.get("alarm") == "1"]
        first = min((x["t1"] for x in al), default=None)
        out = [x for x in r if t <= x["t0"] < (restored or 1e18)]
        print(f"  pointer outside the deployment at +0: first alarm at +{(first - t) if first else None} ms;"
              f" {len(al)} alarmed of {len(out)} requests until N was back; silent pinned mismatches:"
              f" {sum(1 for x in out if x.get('pin') not in (None, 'none') and x.get('ran') != x.get('pin') and x.get('alarm') != '1')}")
        tail = D / "front-tail.jsonl.gz"
        if tail.exists():
            import gzip
            print(f"  ALARM lines in the front's tail: {gzip.open(tail, 'rt').read().count('ALARM graft-front')}")


def bf48():
    r = rows("bf48.jsonl")
    if not r:
        return
    a = answers["bf48-stale-a"]
    b = answers.get("bf48-stale-b")
    print(f"BF-48: {len(r)} member reads")
    for f in (a, b):
        if f:
            print(f"  {f['label']}: {json.dumps(f['answer'])[:300]}")
    for name, flag, nxt in [("A", a, b["t0"] if b else None), ("B", b, ev("blackhole-off"))]:
        if not flag:
            continue
        t = flag["t1"]
        seg = [x for x in r if x["t0"] >= flag["t0"] and (nxt is None or x["t0"] < nxt)]
        for site in (0, 1, 2):
            s = [x for x in seg if x["site"] == site]
            st = [x for x in s if x["kind"] == "stale"]
            first = min((x["t1"] for x in st), default=None)
            last = max((x["t1"] for x in st), default=None)
            until = st[0]["until"] if st else None
            # `until` is on the Workers clock, which ran OFFSET ms ahead of the orb's (/_front/status `now`).
            end = min((x["t0"] for x in s if last and x["t0"] > last), default=None)
            print(f"  flag {name} site {site}: first stale() at +{(first - t) if first else None} ms, last at"
                  f" +{(last - t) if last else None} ms, first non-stale read sent at +{(end - t) if end else None} ms;"
                  f" honored until +{(until - OFFSET - t) if until else None} ms on the orb's clock;"
                  f" after it: {sum(1 for x in s if last and x['t0'] > last)} reads, kinds {sorted({x['kind'] for x in s if last and x['t0'] > last})}")
    ctl = rows("bf48-control.jsonl")
    print(f"  control repository: {len(ctl)} reads, stale {sum(x['kind'] == 'stale' for x in ctl)}")
    c = D / "bf48-commit.jsonl"
    if c.exists():
        for l in c.read_text().splitlines():
            v = json.loads(l)
            print(f"  write probe flag {v.get('flag')} site {v['site']}: {json.dumps(v['commit'].get('error'))[:120]}")


def bf8_isolates():
    """BF-8 (A) by graft-front isolate: those that started before the blackhole (and so held a view) and those that
    started during it, which hold none until a member answers (decisions/OPEN-QUESTIONS.md, R20)."""
    # Rows the sampler could not send (status 0, no front answer) carry no view.
    r = [q for q in rows("bf8.jsonl") if q.get("view")]
    if not r:
        return
    b0, b1 = ev("blackhole"), ev("blackhole-off")
    iso = lambda q: dict(kv.split("=") for kv in q["view"].split(";"))["isolate"]
    # An isolate starts at its first request in any of the run's samplers: BF-43's ran before BF-8 on the same front.
    first = {}
    for q in sorted([q for q in rows("bf43.jsonl") if q.get("view")] + r, key=lambda q: q["t0"]):
        first.setdefault(iso(q), q["t0"])
    x = [q for q in r if b0 <= q["t0"] < b1]
    view = lambda q: dict(kv.split("=") for kv in q["view"].split(";"))
    for name, s in [("started before", [q for q in x if first[iso(q)] < b0]),
                    ("started during", [q for q in x if first[iso(q)] >= b0])]:
        lat = sorted(q["t1"] - q["t0"] for q in s)
        print(f"  isolates {name} the blackhole: {len({iso(q) for q in s})}, {len(s)} requests, pinned"
              f" {sum(q['pin'] != 'none' for q in s)}, ran N {sum(q.get('ran') == N for q in s)}, p99 {pct(lat, .99)} ms")
        # The site copy each isolate took (`copy=<site>@<ms>`, run 3 on), and its isolates' unpinned requests.
        took = {}
        for q in s:
            c = view(q).get("copy", "none")
            if c != "none":
                took.setdefault(iso(q), c)
        if took:
            ms = sorted(int(c.split("@")[1]) for c in took.values())
            sites = sorted({c.split("@")[0] for c in took.values()})
            print(f"    took a site's copy: {len(took)} isolates, sites {sites}, read ms p50 {pct(ms, .5)} max {max(ms)}")
        bad = [q for q in s if q["pin"] == "none"]
        if bad:
            print(f"    unpinned: {len(bad)} requests on {len({iso(q) for q in bad})} isolates, the last"
                  f" {max(q['t0'] - first[iso(q)] for q in bad)} ms after its isolate's first request")
        # By isolate start, in minutes from the blackhole.
        by = {}
        for q in s:
            m = (first[iso(q)] - b0) // 60000
            n = by.setdefault(m, [0, 0])
            n[0] += 1
            n[1] += q["pin"] != "none"
        if name == "started during":
            print("    by isolate start (min after blackhole: requests, pinned): "
                  + ", ".join(f"{m}: {n[0]}/{n[1]}" for m, n in sorted(by.items())))
    po = answers.get("bf8-pin-outside")
    if po:
        t, end = po["t1"], ev("restored-n", po["t1"])
        quiet = [q for q in r if t <= q["t0"] < end and q.get("alarm") != "1"]
        print(f"  not alarmed after the outside pin: {len(quiet)}, pins {sorted({q['pin'] for q in quiet})},"
              f" last at +{max((q['t0'] - t for q in quiet), default=0)} ms")


bf43()
bf8()
bf8_isolates()
bf48()
