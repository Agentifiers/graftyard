#!/usr/bin/env python3
"""humanjoin.py TSAVE LOAD_START_ISO [SECONDS] -> `human-join`'s replay on a leader trace: what Human and MergeQueue
commit latency would be if a command admitted while a batch is frozen joined that batch (`decisions/DECISIONS.md`,
"`human-join`").

From the leader's rows (site 0), as `slotsplit.py` reads them:
- each cut (`Cut`, kind 29): its time, what ended it, how many batches it was held behind, and its Human and MergeQueue
  counts (`taken`, `cut_row`);
- its entry: the (held + 1)-th voter `AppendQueued` (kind 12) after the cut; that entry's apply, the first `Applied`
  (kind 4) at or past its last index.
The trace has no per-command class, so each cut's Human and MergeQueue commands are spread evenly over the gap since
the cut before it [inference, as the 2026-11-21 replay of sync-ab did]. Baseline latency is the entry's apply minus that
arrival.

With the join, a command that arrived while the cut before its own was still unappended (a frozen batch waiting), at
rides that batch instead if a joiners' read asked at or after its arrival lands by that batch's append. Joiners share
one read out at a time, as the code does: the first asks at its arrival, and one arriving while a read is out waits for
the next, asked when that read lands READ_MS later. It rides the batch unless the batch was cut at a byte cap (`Soft`, `EntryMax`; `AgentCmds` and `AgentTokens` still take one) or with no
entry in flight (the cut row's in-flight bit), when the batch waits only on its own read. Its
latency is then that batch's apply minus its arrival. Agent and Bot commands do not move. The model holds the append
times fixed: a joiner never delays one, as the code ensures by sending a late joiner back to its lane.
"""
import bisect, datetime, json, lzma, sys

path, start = sys.argv[1], sys.argv[2]
secs = int(sys.argv[3]) if len(sys.argv) > 3 else 600
t0 = int(datetime.datetime.fromisoformat(start.replace("Z", "+00:00")).timestamp() * 1000)
t1 = t0 + 1000 * secs
ENDS = ["Empty", "Soft", "EntryMax", "AgentTokens", "AgentCmds"]
seen, cuts, appends, applied = set(), [], {}, {}
for l in (lzma.open if path.endswith(".xz") else open)(path, "rt"):
    d = json.loads(l)
    if "rows" not in d or "learner" in d or d.get("site") != 0:
        continue
    for r in d["rows"]:
        if tuple(r) in seen:
            continue
        seen.add(tuple(r))
        at, kind, peer = r[0], r[1], r[2]
        b, i = int(float(r[3])), int(float(r[4]))
        if kind == 29:
            cuts.append(dict(at=at, end=ENDS[peer % 16] if peer % 16 < 5 else peer % 16, held=(peer // 16) % 4,
                             early=bool(peer & 64),
                             first=((b >> 0) & 0xFFF) + ((b >> 12) & 0xFFF), all=sum((b >> (12 * c)) & 0xFFF for c in range(4))))
        elif kind == 12 and peer < 192:
            appends.setdefault(i, at)
        elif kind == 4:
            applied.setdefault(i, at)
cuts.sort(key=lambda c: c["at"])
app = sorted((at, i) for i, at in appends.items())
app_t = [a for a, _ in app]
ap_idx = sorted(applied)
for c in cuts:
    k = bisect.bisect_right(app_t, c["at"]) + c["held"]
    c["append"], last = app[k] if k < len(app) else (None, None)
    j = bisect.bisect_left(ap_idx, last) if last is not None else len(ap_idx)
    c["apply"] = applied[ap_idx[j]] if j < len(ap_idx) else None


def pc(xs, q):
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(len(xs) * q))] if xs else 0


def joins(p, arrivals, read_ms):
    """Which of a cut's Human and MergeQueue arrivals (ascending) join the frozen batch `p` under one-at-a-time reads."""
    if p["append"] is None or p["apply"] is None or p["end"] in ("Soft", "EntryMax") or not p["early"]:
        return [False] * len(arrivals)
    out, out_until, back = [], None, False  # when the read now out lands; whether a joiner went back to its lane
    for a in arrivals:
        if a >= p["append"]:
            out.append(False)
            continue
        # No read out: this joiner asks one now. Otherwise it waits for the one out; the next leaves when that lands.
        ask = a if out_until is None or out_until <= a else out_until
        out_until = ask + read_ms
        # Once one goes back to its lane, every later one does, keeping their order.
        back = back or out_until > p["append"]
        out.append(not back)
    return out


def run(read_ms):
    base, joined_lat, n_join, frozen = [], [], 0, 0
    for j in range(1, len(cuts)):
        c, p = cuts[j], cuts[j - 1]
        if c["apply"] is None or not t0 <= c["at"] < t1:
            continue
        n = c["first"]
        arr = [p["at"] + (c["at"] - p["at"]) * (k + 1) / (n + 1) for k in range(n)]
        for a, ok in zip(arr, joins(p, arr, read_ms)):
            base.append(c["apply"] - a)
            frozen += p["append"] is not None and a < p["append"]
            if ok:
                n_join += 1
                joined_lat.append(p["apply"] - a)
            else:
                joined_lat.append(c["apply"] - a)
    return base, joined_lat, n_join, frozen


def all_commands(read_ms):
    # Every command, by the same spread; agents keep their latency, Humans and MergeQueue take the join's.
    out_b, out_j = [], []
    for j in range(1, len(cuts)):
        c, p = cuts[j], cuts[j - 1]
        if c["apply"] is None or not t0 <= c["at"] < t1:
            continue
        arr = [p["at"] + (c["at"] - p["at"]) * (k + 1) / (c["all"] + 1) for k in range(c["all"])]
        # The cut's Human and MergeQueue commands are its first `first`, spread over the same gap.
        ok_h = joins(p, arr[: c["first"]], read_ms)
        for k, a in enumerate(arr):
            out_b.append(c["apply"] - a)
            ok = k < c["first"] and ok_h[k]
            out_j.append((p["apply"] if ok else c["apply"]) - a)
    return out_b, out_j


base, _, _, frozen = run(0)
print(f"{len(cuts)} cuts; ends {dict((e, sum(c['end'] == e for c in cuts)) for e in ENDS)}; "
      f"Human+MergeQueue commands {len(base)}, {frozen} arrived behind a frozen batch")
print(f"baseline: Human p50 {pc(base, .5):.0f} p90 {pc(base, .9):.0f} p99 {pc(base, .99):.0f}, "
      f"within 1 s {sum(x <= 1000 for x in base) / max(1, len(base)):.0%}")
for d in (0, 75, 150):
    _, jl, nj, _ = run(d)
    ab, aj = all_commands(d)
    print(f"join, {d} ms read: {nj / max(1, len(jl)):.0%} join; Human p50 {pc(jl, .5):.0f} p90 {pc(jl, .9):.0f} "
          f"p99 {pc(jl, .99):.0f}, within 1 s {sum(x <= 1000 for x in jl) / max(1, len(jl)):.0%}; all commands p99 "
          f"{pc(ab, .99):.0f} -> {pc(aj, .99):.0f}")
