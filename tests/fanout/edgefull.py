#!/usr/bin/env python3
"""edgefull.py: B0-33 row B's edge refusals, from the edge's `edgefull` log lines (`workers/edge/src/lib.rs`, `mod
edgefull`), read against `decisions/DECISIONS.md`, "`edge-refusal`", flips 2 to 4, and "`edge-refusal-bound`", flips
R1b and R1c.

  edgefull.py fetch FROM_ISO TO_ISO OUT.jsonl [SCRIPT]   every `edgefull` line SCRIPT (default graft-m2b-edge) logged
                                                         in the window, one JSON object a line with its Workers Logs
                                                         timestamp (`t`, Cloudflare's clock). Workers Logs `events`
                                                         view; account and token from the environment, the token only
                                                         as a header. Writes no account field.
  edgefull.py read LINES.jsonl [TAG]                     the summary: refusals and fail-open sends, push and REST, per
                                                         isolate and per 10 s window; with TAG, each window joined to
                                                         the leader's own cuts (`tsave-TAG.jsonl.xz`, `agentdrain.py`)
                                                         for flips 2 to 4. Prints one `flip N:` line each. Run from the
                                                         repository root.

The join's clock: the cuts are stamped on the leader's clock, which ran 1 to 3.5 s behind Cloudflare's under load
(`decisions/OPEN-QUESTIONS.md`, A41's correction). A refusal at Cloudflare time t is therefore read against its own
10 s window and the one before it: flip 2 takes the larger leader wait of the two, so a refusal counts as over-refusal
only when both windows read under half the bound; flip 3 counts an empty lane only when both windows had one.
Flip 1 (a Human, Bot or MergeQueue write refused) is not read here: every `edgefull` line is an Agent's by
construction, and a1 stops the row on any non-agent write with the `advert` mark (`a1.mjs`, `flip1`).
"""
import collections, json, os, sys, time, urllib.request
from datetime import datetime, timezone

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

W = 10_000
BOUND_MS = 5_000
AGENT_CMDS_ENTRY = 64
# A chain fails open at most once an ADVERT_TTL per isolate (2 s, `graftyard_raft::admit::ADVERT_TTL`).
ADVERT_TTL_MS = 2_000
# A lapsed full advert holds Agent writes this long (`graftyard_raft::admit::ADVERT_STALE`).
ADVERT_STALE_MS = 10_000
# edge-refusal-bound's flip R2b (`decisions/DECISIONS.md`): the baseline's agent commands cut a second while refusing,
# p50. A run below it reverts the send check. The baseline is erBA's 78.7, the row the send check was built against
# (the lead's ruling of 2026-11-24 on B0-33 h1A, `tests/am1/results/b033-h1-set.md`); erDA's 114.2, used from
# 2026-11-23, was a fast host's drain with no edge refusal, so a refusing run read FLIPPED against it while it drained
# above the slow-host baseline.
ERBA_CUT_P50 = 78.7
ms = lambda s: int(datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)


def fetch(a, b, out, svc="graft-m2b-edge"):
    acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
    url = f"https://api.cloudflare.com/client/v4/accounts/{acct}/workers/observability/telemetry/query"
    flt = [{"key": "$metadata.service", "operation": "eq", "type": "string", "value": svc},
           {"key": "edgefull", "operation": "exists", "type": "string"}]

    def events(t, u):
        body = {"queryId": "edgefull", "timeframe": {"from": t, "to": u}, "view": "events", "limit": 2000,
                "parameters": {"filters": flt, "limit": 2000}}
        for i in range(5):
            try:
                req = urllib.request.Request(url, data=json.dumps(body).encode(), method="POST",
                                             headers={"authorization": f"Bearer {tok}", "content-type": "application/json"})
                return json.load(urllib.request.urlopen(req, timeout=120))["result"].get("events", {}).get("events", [])
            except Exception as e:  # noqa: BLE001
                print("retry", i, e, file=sys.stderr)
                time.sleep(3 + 3 * i)
        raise SystemExit("query failed")

    # One query a second: the view answers at most 2,000 events, so a full second is reported, not silently cut.
    n, full, t, end = 0, 0, ms(a), ms(b)
    with open(out, "w") as f:
        for t in range(ms(a), end, 1_000):
            ev = events(t, min(end, t + 1_000))
            full += len(ev) >= 2000
            for e in ev:
                src = e.get("source") or {}
                if "edgefull" in src:
                    f.write(json.dumps({"t": e["timestamp"], **src}) + "\n")
                    n += 1
    print(f"{n} edgefull lines from {a} to {b}" + (f"; {full} seconds hit 2,000 lines and are cut" if full else ""))


def pct(xs, p):
    s = sorted(xs)
    return s[min(len(s) - 1, int(len(s) * p))] if s else None


def leader_windows(tag):
    """{window: (leader wait ms or None, agent commands cut a second, lane empty)} on the leader's clock."""
    import agentdrain
    by = collections.defaultdict(list)
    for at, taken, left, end, _ in agentdrain.cuts(tag):
        by[at // W].append((taken[3], left[3], end))
    out = {}
    for k, xs in by.items():
        drain = sum(x[0] for x in xs) / (W / 1000)
        q = xs[-1][1]
        wait = q * 1000 / drain if drain else (None if q else 0.0)
        # Flip 3's empty lane: every cut of the window took under AGENT_CMDS_ENTRY agent commands and left none.
        empty = all(x[0] < AGENT_CMDS_ENTRY and x[1] == 0 for x in xs)
        out[k] = (wait, drain, empty)
    return out


def read(path, tag=None):
    import lzma
    lines = [json.loads(x) for x in (lzma.open(path, "rt") if path.endswith(".xz") else open(path)) if x.strip()]
    if not lines:
        print("no edgefull lines: the edge refused nothing and sent nothing on a lapsed full advert")
        return
    kinds = collections.Counter((x["edgefull"], "push" if x.get("push") else "rest") for x in lines)
    iso = collections.defaultdict(lambda: {"refused": 0, "failOpen": 0, "held": 0, "probe": 0, "stale": 0, "lines": 0})
    for x in lines:
        i = iso[x["isolate"]]
        for k in ("refused", "failOpen", "held", "probe", "stale"):
            i[k] = max(i[k], x.get(k, 0))
        i["lines"] += 1
    t0, t1 = min(x["t"] for x in lines), max(x["t"] for x in lines)
    span_s = max(1.0, (t1 - t0) / 1000)
    # edge-refusal-bound: `held` refuses on a lapsed advert as `refused` does on a fresh one; `probe` and `stale` are
    # the writes a lapsed full advert let through, which an edge before it logged as `failOpen`.
    refused = [x for x in lines if x["edgefull"] in ("refused", "held")]
    opened = [x for x in lines if x["edgefull"] in ("failOpen", "probe", "stale")]
    waits = [x["waitMs"] for x in refused if x.get("waitMs") is not None]
    print(json.dumps({"lines": len(lines), "byKind": {f"{a}:{b}": n for (a, b), n in sorted(kinds.items())},
                      "isolates": len(iso), "spanS": round(span_s),
                      "refusedWaitMs": {"p10": pct(waits, .1), "p50": pct(waits, .5), "p90": pct(waits, .9)},
                      "budgetMs": {"p50": pct([x["budgetMs"] for x in refused], .5)}}))
    ceiling = span_s * 1000 / ADVERT_TTL_MS
    top = sorted(iso.items(), key=lambda kv: -kv[1]["refused"])
    print(f"per isolate (running counts at their last line), {len(iso)} isolates; fail-open ceiling about "
          f"{ceiling:.0f} an isolate a chain over {span_s:.0f} s:")
    for k, v in top[:10]:
        print(f"  {k} refused {v['refused']:6d} held {v['held']:5d} probe {v['probe']:4d} stale {v['stale']:4d} "
              f"failOpen {v['failOpen']:5d} lines {v['lines']}")
    print(f"  sum: refused {sum(v['refused'] for v in iso.values())}, failOpen {sum(v['failOpen'] for v in iso.values())}"
          f"; max failOpen {max(v['failOpen'] for v in iso.values())}")
    win = collections.defaultdict(lambda: [0, 0])
    for x in refused:
        win[x["t"] // W][0] += 1
    for x in opened:
        win[x["t"] // W][1] += 1
    lw = leader_windows(tag) if tag else None
    print("10 s window (Cloudflare clock) | refused | failOpen" + (" | leader wait s (this, before) | agent cut/s | lane empty" if lw else ""))
    over, starved, refusing, rates = 0, 0, 0, []
    for k in sorted(win):
        r, o = win[k]
        at = datetime.fromtimestamp(k * W / 1000, timezone.utc).strftime("%H:%M:%S")
        if not lw:
            print(f"  {at} | {r:5d} | {o:4d}")
            continue
        here, before = lw.get(k), lw.get(k - 1)
        ws = [w[0] for w in (here, before) if w and w[0] is not None]
        empty = bool(here and here[2] and before and before[2])
        rate = here[1] if here else None
        if r:
            refusing += 1
            over += 1 if ws and max(ws) < BOUND_MS / 2 else 0
            starved += 1 if empty else 0
            if rate is not None:
                rates.append(rate)
        fmt = lambda w: "-" if not w or w[0] is None else f"{w[0] / 1000:.1f}"
        print(f"  {at} | {r:5d} | {o:4d} | {fmt(here)}, {fmt(before)} | {rate if rate is not None else '-'} | {empty}")
    bound(lines)
    if not lw:
        return
    first = min((k for k in win if win[k][0]), default=None)
    pre = [v[1] for k, v in lw.items() if first is not None and k < first - 1 and v[0] is not None and v[0] > 0]
    share = over / refusing if refusing else 0.0
    print(f"flip 2 (over-refusal): {over} of {refusing} refusing windows read a leader wait under "
          f"{BOUND_MS / 2000:.1f} s ({share:.0%}); flips over 10%: {'FLIPPED' if share > 0.10 else 'not met'}")
    print(f"flip 3 (starvation): {starved} refusing windows with the leader's agent lane empty; agent commands cut a "
          f"second while refusing p50 {pct(rates, .5)}, saturated drain before refusing began p50 {pct(pre, .5)} "
          "(compare with the matched baseline's agent commands cut a second at 90%)")
    # edge-refusal-bound's flip R2b, its drain half (the other half is pushfull.py's agent `ok` against erDA's 8,369).
    r2b = pct(rates, .5)
    print(f"flip R2b (drain half): agent commands cut a second while refusing p50 {r2b} against erBA's {ERBA_CUT_P50}: "
          + ("FLIPPED, revert the send check" if r2b is not None and r2b < ERBA_CUT_P50 else "not met"))
    so = sum(v["failOpen"] for v in iso.values())
    sr = sum(v["refused"] for v in iso.values())
    print(f"flip 4 (leader still refusing at pfA's counts, read with pushfull.py's admission and expire_agents): "
          f"failOpen {so} against refused {sr}; "
          + ("fail-opens pass half the refusals: the bursts are fail-opens" if sr and so > sr / 2 else
             "refusals near 0: check whether adverts reach the edge" if sr < 10 else "fail-opens under half the refusals"))


def bound(lines):
    """edge-refusal-bound's flips R1b and R1c, and the refusals by where they met the advert (`at`)."""
    held = [x for x in lines if x["edgefull"] == "held"]
    stale = [x for x in lines if x["edgefull"] == "stale"]
    if not held and not stale and not any(x["edgefull"] == "probe" for x in lines):
        return
    at = collections.Counter((x["edgefull"], x.get("at", "check")) for x in lines)
    print("by where the write met the advert: " + ", ".join(f"{a}@{b} {n}" for (a, b), n in sorted(at.items())))
    over = sum(1 for x in held if x.get("ageMs", 0) > ADVERT_STALE_MS)
    probes = collections.defaultdict(list)
    for x in lines:
        if x["edgefull"] == "probe":
            probes[(x["isolate"], x["repo"])].append(x["t"])
    # The edge stamps a probe on its own clock; Workers Logs' timestamp can differ by a few ms, so 50 ms of slack.
    twice = sum(1 for ts in probes.values() for a, b in zip(sorted(ts), sorted(ts)[1:]) if b - a < ADVERT_TTL_MS - 50)
    print(f"flip R1b (invariant): {over} held past ADVERT_STALE, {twice} second probes within ADVERT_TTL for one chain "
          f"and isolate: {'FLIPPED' if over or twice else 'not met'}")
    print(f"flip R1c (probes not bringing adverts back): stale sends {len(stale)} against held refusals {len(held)}: "
          f"{'FLIPPED' if len(stale) > len(held) else 'not met'}")


if __name__ == "__main__":
    if len(sys.argv) >= 5 and sys.argv[1] == "fetch":
        fetch(*sys.argv[2:6])
    elif len(sys.argv) >= 3 and sys.argv[1] == "read":
        read(sys.argv[2], sys.argv[3] if len(sys.argv) > 3 else None)
    else:
        raise SystemExit(__doc__)
