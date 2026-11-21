#!/usr/bin/env python3
"""m3fhs1's member round-trip split, read on the members' clocks only.

A Workers clock (`Date.now()`) reads the time of the isolate's last I/O, so it can only read behind real time, never
ahead [doc, https://developers.cloudflare.com/workers/reference/security-model/]. For one `/raft` call from member M to
the leader, M's `CallSent` stamp is at or before the real send, and the leader's `Entered` stamp (and its Workers Logs
event start) must be after the real send. So when the leader's stamp reads earlier than M's send, the leader's clock is
behind real time by at least that much. This script prints, per second of send after T0:
  rt      M's AnswerArrived - CallSent, on M's clock (the whole round trip)
  hold    M's own pending storage confirm at the send (M's output gate), from its Sync rows
  lag     M's send - the leader's Entered stamp for the call, less M's quiet-time offset: how far the leader's clock
          read behind M's at least (positive means behind)
  wl_lag  the same against the leader's Workers Logs event start (needs the leaderwl.py events file)

Then the causal bounds, on the members' clocks only (each member shifted by its quiet-time offset to the others): the
leader took a call no earlier than the latest send among the calls it took before it, and no later than the earliest
answer arrival among the calls it took after it. So per call, `in` (send to the leader taking it) has a lower and an
upper bound, and `after` (taking it to the answer arriving) is at least the answer minus that upper bound.

Usage: callclock.py TSAVE T0_MS [LEADER_EVENTS.json]
"""
import ast, collections, json, lzma, sys


def pct(v, p):
    v = sorted(v)
    return v[min(len(v) - 1, int(p * len(v)))] if v else None


def rows(path):
    seen, lead, mem = set(), [], collections.defaultdict(list)
    for line in (lzma.open if path.endswith('.xz') else open)(path, 'rt'):
        d = json.loads(line)
        if 'rows' not in d:
            continue
        if d.get('site') == 0 and 'learner' not in d:
            for r in d['rows']:
                if tuple(r) not in seen:
                    seen.add(tuple(r)); lead.append(r)
        else:
            mem[int(d['learner']) if 'learner' in d else int(d['site'])].extend(d['rows'])
    return lead, mem


def main(path, t0, events=None):
    lead, mem = rows(path)
    ent = {(int(r[2]), int(r[3])): r[0] for r in lead if r[1] == 25 and int(r[2]) != 255 and int(r[3])}
    wl = {}
    if events:
        for e in json.load(open(events)):
            ev = e['$workers'].get('event') or {}
            if ev.get('path') != '/raft':
                continue
            req = ev.get('request') or {}
            h = (req if isinstance(req, dict) else ast.literal_eval(req)).get('headers', {})
            if h.get('x-graft-call'):
                wl[(int(h['x-graft-from']), int(h['x-graft-call']))] = e['timestamp']
    by = collections.defaultdict(list)
    for a, rr in mem.items():
        sent = {int(r[3]): r[0] for r in rr if r[1] == 23 and int(r[2]) == 0}
        ans = {int(r[3]): r[0] for r in rr if r[1] == 21 and int(r[2]) == 0}
        syn = [(r[0] - int(float(r[5])), r[0]) for r in rr if r[1] == 31]
        quiet = [ent[(a, i)] - s for i, s in sent.items() if (a, i) in ent and i in ans and ans[i] - s < 30
                 and not t0 - 500 <= s < t0 + 9000]
        off = pct(quiet, .5) or 0
        for i, s in sent.items():
            if i not in ans or (a, i) not in ent or not t0 - 1000 <= s < t0 + 12000:
                continue
            hold = max([e - s for b, e in syn if b <= s < e], default=0)
            wlag = s - wl[(a, i)] + off if (a, i) in wl else None
            by[(s - t0) // 1000].append((ans[i] - s, hold, s - ent[(a, i)] + off, wlag))
    print('send s | n | rt p50 p90 | own hold p50 p90 | leader clock behind p50 p90 | leader WL start behind p50 p90')
    for k, v in sorted(by.items()):
        c = lambda j: [x[j] for x in v if x[j] is not None]
        print(f'{k:>4} {len(v):>4} | {pct(c(0), .5):>5} {pct(c(0), .9):>5} | {pct(c(1), .5):>3} {pct(c(1), .9):>3} | '
              f'{pct(c(2), .5):>5} {pct(c(2), .9):>5} | {pct(c(3), .5)} {pct(c(3), .9)}')

    off, S, A = {}, {}, {}
    for a, rr in mem.items():
        sent = {int(r[3]): r[0] for r in rr if r[1] == 23 and int(r[2]) == 0}
        ans = {int(r[3]): r[0] for r in rr if r[1] == 21 and int(r[2]) == 0}
        q = [ent[(a, i)] - s for i, s in sent.items() if (a, i) in ent and i in ans and ans[i] - s < 30
             and not t0 - 500 <= s < t0 + 9000]
        if len(q) < 5:
            continue
        off[a] = pct(q, .5)
        S.update({(a, i): s + off[a] for i, s in sent.items()})
        A.update({(a, i): s + off[a] for i, s in ans.items()})
    order = [(int(r[2]), int(r[3])) for r in lead if r[1] == 25 and int(r[2]) != 255]
    lb, m = {}, -1e18
    for k in order:
        m = max(m, S.get(k, m)); lb[k] = m
    ub, m = {}, 1e18
    for k in reversed(order):
        m = min(m, A.get(k, m)); ub[k] = m
    by = collections.defaultdict(list)
    for k in order:
        if k in S and k in A and t0 - 1000 <= S[k] < t0 + 9000:
            by[(S[k] - t0) // 1000].append((A[k] - S[k], lb[k] - S[k], ub[k] - S[k], A[k] - ub[k]))
    print('send s | n | rt p50 | in: lower p50, upper p50 p90 | after: at least p50 p90')
    for k, v in sorted(by.items()):
        c = lambda j: [x[j] for x in v]
        print(f'{k:>4} {len(v):>4} | {pct(c(0), .5):>5} | {pct(c(1), .5):>4}, {pct(c(2), .5):>5} {pct(c(2), .9):>5} | '
              f'{pct(c(3), .5):>4} {pct(c(3), .9):>4}')


if __name__ == '__main__':
    main(sys.argv[1], int(sys.argv[2]), sys.argv[3] if len(sys.argv) > 3 else None)
