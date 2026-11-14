"""LM-1 round 16b: the near follower's AppendEntries leg, split, from tests/fanout/legs.mjs's samples.

    python3 tests/fanout/legs.py <dir> [from ISO] [to ISO]

Leader trace (`workers/site/src/host.rs`, `Trace`): AppendQueued 12 (batch round, index last entry, ms bytes),
AppendIssued 13 (ms since queued), AppendEnded 14 (index 0 ok, ms since issued). Follower: `raftIn` rows
[t0, bytes, body_ms, work_ms] (t0 the `/raft` handler's start; `work` is decode, step, and the output gate), and its
trace Stepped 15 (ms the synchronous step and persist), Confirm 10 (ms the `sync()`, at its end). Durations are
taken on one clock each; only `send -> receive` crosses clocks (leader issue to follower t0), so it carries any skew.
Per matched call: leg = leader issue to end; in = follower t0 - leader issue (wire out plus the follower's input
queue); body; step; gate = work - step (the output gate: waiting for the confirm); back = leg - in - body - work.
"""
import glob, json, sys
from collections import defaultdict


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
    reads = defaultdict(list)  # (site, name) -> [(at, inbound /read)]
    leader = None
    for f in sorted(glob.glob(f"{d}/legs-*.json")):
        s = json.load(open(f))
        for m in s["members"]:
            key = (m["site"], m["name"])
            if m.get("inbound"):
                reads[key].append((s["at"], m["inbound"].get("/read", 0), m["voter"], m["role"]))
            if m["role"] is True:
                leader = m["site"]
                q = {}
                for at, k, peer, batch, index, ms in m["trace"]:
                    if k == 12:
                        q[(peer, batch)] = (at, ms)
                    elif k == 13 and (peer, batch) in q:
                        q[(peer, batch)] = (q[(peer, batch)][0] + ms, q[(peer, batch)][1])
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
            legs.append(dict(leg=leg, inn=inn, body=body, step=step, gate=max(0, work - step), sync=sync,
                             back=back, wire=inn + back, bytes=nbytes))
        if not legs:
            continue
        f = lambda k: (pct([x[k] for x in legs], 0.5), pct([x[k] for x in legs], 0.99))
        slow = sorted(legs, key=lambda x: -x["leg"])[: max(1, len(legs) // 100)]
        g = lambda k: round(sum(x[k] for x in slow) / len(slow))
        out[site] = dict(n=len(legs), **{k: f(k) for k in ["leg", "inn", "body", "step", "gate", "sync", "back", "wire", "bytes"]},
                         slowest_1pct_mean={k: g(k) for k in ["leg", "inn", "body", "step", "gate", "sync", "back", "wire"]})
    print("leader", leader)
    for site, v in out.items():
        print(f"site {site} n {v['n']}:", " ".join(f"{k} {v[k][0]}/{v[k][1]}" for k in ["leg", "inn", "body", "step", "gate", "sync", "back", "wire", "bytes"]))
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
