#!/usr/bin/env python3
"""replay.py TSAVE LOAD_START_MS -> the leader's lease guard on a B0 trace, old rule against clock-guard's (`tests/m0/results/clock-guard.md`).
Rebuilds every traced member's member clock over the /raft call graph, so each stamp is its sender's member clock.
    python3 tests/m0/clockguard/replay.py tests/fanout/raw/tsave-m2bb29c.jsonl.xz 1794958512715
"""
import json, lzma, sys, collections
# Reconstructs every traced member's member clock (running max of its raw clock and every stamp it received) over the
# /raft call graph, so each message's stamp is its sender's member clock, as `x-graft-clock` is. Then replays the
# leader's guard: main's lag (stamp - own raw) and clock-guard's (stamp - member clock before it). Edge stamps are not
# traced and are left out. Events: CallSent 23 (sender, peer=to, batch=id), CallArrived 20 (receiver, peer=from,
# batch=id), CallLeft 24 (receiver, reply leaves), AnswerArrived 21 (sender, peer=to, batch=id).
path, t0 = sys.argv[1], int(sys.argv[2]); t1 = t0 + 600000
op = lzma.open if path.endswith('.xz') else open
seen = set(); ev = collections.defaultdict(list)
with op(path, 'rt') as f:
    for line in f:
        d = json.loads(line)
        if 'rows' not in d: continue
        me = d['learner'] if 'learner' in d else d.get('site')
        for r in d['rows']:
            k = (me, tuple(r))
            if k in seen: continue
            seen.add(k)
            if r[1] in (20, 21, 23, 24) and t0 - 60000 <= r[0] < t1:
                ev[me].append(tuple(r))
for m in ev: ev[m].sort()
# keys: call (sender, id) stamp out; reply (sender, id) stamp out from the callee.
call_st, reply_st = {}, {}
for p in range(6):
    new_call, new_reply = {}, {}
    out = {}
    for m, rows in ev.items():
        mc = 0; lags_main, lags_new, wm, wn, sm, sn, n = [], [], collections.deque(maxlen=8), collections.deque(maxlen=8), 0, 0, 0
        for r in rows:
            at, kind, peer, cid = r[0], r[1], r[2], r[3]
            if kind == 23:
                mc = max(mc, at); new_call[(m, cid)] = mc
            elif kind == 24:
                mc = max(mc, at); new_reply[(peer, cid)] = mc
            else:
                st = call_st.get((peer, cid), r[4]) if kind == 20 else reply_st.get((m, cid))
                if st is None: continue
                before = max(mc, at)
                lm, ln = max(0, st - at), max(0, st - before)
                mc = max(before, st)
                wm.append(lm); wn.append(ln)
                if m == 0 and t0 <= at < t1:
                    n += 1; lags_main.append(lm); lags_new.append(ln); sm += max(wm) >= 250; sn += max(wn) >= 250
        out[m] = (lags_main, lags_new, sm, sn, n)
    call_st, reply_st = new_call, new_reply
def pc(xs):
    xs = sorted(xs); k = len(xs)
    return f"p50 {xs[k//2]} p90 {xs[int(k*.9)]} p99 {xs[int(k*.99)]} p999 {xs[int(k*.999)]} max {xs[-1]}"
lm, ln, sm, sn, n = out[0]
print(f"leader stamps in load {n}")
print(f"main lag (stamp - own):            {pc(lm)}; >=256 {100*sum(x>=256 for x in lm)/n:.1f}%; suspect {100*sm/n:.1f}%")
print(f"clock-guard lag (stamp - member):  {pc(ln)}; >=256 {100*sum(x>=256 for x in ln)/n:.2f}%; suspect {100*sn/n:.2f}%")
