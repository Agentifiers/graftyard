"""LM-1 round 16b: the near follower's AppendEntries leg, split, from tests/fanout/legs.mjs's samples.

    python3 tests/fanout/legs.py <dir> [from ISO] [to ISO]

Leader trace (`workers/site/src/host.rs`, `Trace`): AppendQueued 12 (batch round, index last entry, ms bytes),
AppendIssued 13 (ms since queued), AppendEnded 14 (index 0 ok, ms since issued). Follower: `raftIn` rows
[t0, bytes, body_ms, work_ms] (t0 the `/raft` handler's start; `work` is decode, step, and the output gate), and its
trace Stepped 15 (ms the synchronous step and persist), Confirm 10 (ms the `sync()`, at its end). Durations are
taken on one clock each; only `send -> receive` crosses clocks (leader issue to follower t0), so it carries any skew.
Per matched call: leg = leader issue to end; in = follower t0 - leader issue (wire out plus the follower's input
queue); body; step; gate = work - step (the output gate: waiting for the confirm); back = leg - in - body - work.

Round 17 (main a858bfb's send stamps, test-faults): the leader's AnswerArrived 21 (`peer` the follower, `index` the
follower's clock when its handler returned the answer, `x-graft-left`; logged at the leader when the fetch's response
headers arrived) splits `back` into fsend = left - (t0 + body + work) on the follower's clock (its handler's return to
the response leaving: the runtime's output gate) and wire_back = leader arrival - left (crosses clocks), and lq =
AppendEnded - arrival on the leader's clock (the response body read and decode, before the reply is stepped). The
follower's CallArrived 20 (`index` the leader's `x-graft-sent`) gives wire_in = t0 - sent (crosses clocks). The clock
offset cancels in wire_in + wire_back, the round trip's network and both input queues; `skew` estimates it from each
pair's asymmetry, which is only valid if the two directions take about the same time. Both arrival stamps are taken
when the receiving isolate's code runs, so wire_in holds the follower's input queue and wire_back the leader's event
loop wait as well as the network; the leader's Lag 8 samples (its 100 ms ticker woken late) bound that wait.
"""
import glob, json, sys
from collections import defaultdict


KEYS = ["leg", "inn", "body", "step", "gate", "sync", "back", "wire", "bytes"]
STAMP_KEYS = ["wire_in", "fsend", "wire_back", "lq", "rtt_wire", "skew"]


def pct(xs, p):
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(p * len(xs)))] if xs else None


def main():
    d = sys.argv[1]
    lo = sys.argv[2] if len(sys.argv) > 2 else ""
    hi = sys.argv[3] if len(sys.argv) > 3 else "~"
    calls = {}  # (peer, at_issued) from the leader: (issued_at, ended_at, bytes, ok)
    raft_in = defaultdict(dict)  # site -> t0 -> row
    stepped = defaultdict(dict)  # site -> at -> ms
    confirm = defaultdict(dict)
    answered = defaultdict(list)  # leader: peer -> [(arrival at leader, follower's left stamp)]
    arrived = defaultdict(dict)  # follower site -> t0 -> leader's sent stamp
    reads = defaultdict(list)  # (site, name) -> [(at, inbound /read)]
    leader = None
    for f in sorted(glob.glob(f"{d}/legs-*.json")):
        s = json.load(open(f))
        for m in s["members"]:
            if m.get("summary"):
                continue
            key = (m["site"], m["name"])
            if m.get("inbound"):
                reads[key].append((s["at"], m["inbound"].get("/read", 0), m["voter"], m["role"]))
            if m.get("summary"):
                continue
            if m["role"] is True:
                leader = m["site"]
                q = {}
                for at, k, peer, batch, index, ms in m["trace"]:
                    if k == 12:
                        q[(peer, batch)] = (at, ms)
                    elif k == 13 and (peer, batch) in q:
                        q[(peer, batch)] = (q[(peer, batch)][0] + ms, q[(peer, batch)][1])
                    elif k == 21:
                        answered[peer].append((at, index))
                for at, k, peer, batch, index, ms in m["trace"]:
                    if k == 14 and (peer, batch) in q:
                        issued, nbytes = q[(peer, batch)]
                        calls[(peer, issued)] = (issued, at, nbytes, index == 0)
            elif m["voter"] and m["raftIn"]:
                for r in m["raftIn"]:
                    raft_in[m["site"]][r[0]] = r
                for at, k, peer, batch, index, ms in m["trace"]:
                    if k == 15:
                        stepped[m["site"]][at] = ms
                    elif k == 10:
                        confirm[m["site"]][at] = ms
                    elif k == 20:
                        arrived[m["site"]][at] = index
    iso = lambda ms: __import__("datetime").datetime.fromtimestamp(ms / 1000, __import__("datetime").UTC).strftime("%Y-%m-%dT%H:%M:%S")
    out = {}
    for site in sorted(raft_in):
        rows = sorted(raft_in[site].values())
        st = sorted(stepped[site].items())
        cf = sorted(confirm[site].items())
        legs = []
        for (peer, issued), (iss, end, nbytes, ok) in sorted(calls.items()):
            if peer != site or not ok or nbytes == 0 or not (lo <= iso(iss) <= hi):
                continue
            # The follower's handler that started after the issue and ended before the leader's end.
            cand = [r for r in rows if iss - 50 <= r[0] <= end and r[0] + r[2] + r[3] <= end + 50]
            if not cand:
                continue
            r = min(cand, key=lambda r: abs(r[1] - nbytes))
            t0, b, body, work = r
            step = next((ms for at, ms in st if t0 + body <= at <= t0 + body + work), 0)
            sync = max((ms for at, ms in cf if t0 + body <= at <= t0 + body + work), default=0)
            leg = end - iss
            inn = t0 - iss
            back = leg - inn - body - work
            x = dict(leg=leg, inn=inn, body=body, step=step, gate=max(0, work - step), sync=sync,
                     back=back, wire=inn + back, bytes=nbytes)
            # Send stamps: the answer whose left stamp falls after this handler's work, arriving before the end.
            done = t0 + body + work
            ans = [(a, left) for a, left in answered[site] if done - 5 <= left <= done + 2000 and iss <= a <= end]
            sent = arrived[site].get(t0)
            if ans and sent is not None:
                a, left = min(ans, key=lambda p: p[1])
                x.update(fsend=left - done, wire_back=a - left, lq=end - a, wire_in=t0 - sent,
                         rtt_wire=(t0 - sent) + (a - left), skew=((t0 - sent) - (a - left)) // 2)
            legs.append(x)
        if not legs:
            continue
        f = lambda k: (pct([x[k] for x in legs if k in x], 0.5), pct([x[k] for x in legs if k in x], 0.99))
        slow = sorted(legs, key=lambda x: -x["leg"])[: max(1, len(legs) // 100)]
        g = lambda k: round(sum(x[k] for x in slow if k in x) / max(1, sum(k in x for x in slow)))
        stamped = sum("fsend" in x for x in legs)
        out[site] = dict(n=len(legs), stamped=stamped, **{k: f(k) for k in KEYS + STAMP_KEYS},
                         slowest_1pct_mean={k: g(k) for k in KEYS[:-1] + (STAMP_KEYS if stamped else [])})
    print("leader", leader)
    for site, v in out.items():
        print(f"site {site} n {v['n']} stamped {v['stamped']}:",
              " ".join(f"{k} {v[k][0]}/{v[k][1]}" for k in KEYS + (STAMP_KEYS if v["stamped"] else [])))
        print("   slowest 1% mean:", v["slowest_1pct_mean"])
    # Entries: the bytes of each round's AppendEntries to the near follower.
    share = {}
    for key, xs in reads.items():
        xs = [x for x in xs if lo <= x[0] <= hi] or xs
        if len(xs) >= 2:
            share[f"{key[0]}:{key[1]}"] = dict(reads=xs[-1][1] - xs[0][1], voter=xs[0][2], leader=xs[-1][3])
    tot = sum(v["reads"] for v in share.values()) or 1
    for k, v in sorted(share.items(), key=lambda kv: -kv[1]["reads"]):
        print(k, v["reads"], f"{100 * v['reads'] / tot:.1f}%", "voter" if v["voter"] else "learner", "leader" if v["leader"] else "")


main()
