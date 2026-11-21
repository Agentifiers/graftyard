#!/usr/bin/env python3
"""A37's split of a slow leader confirm into the leader's input stop and its output hold, from data every test-faults
install already logs: the leader object's Workers Logs events (one per request, with its real start time, its wall
time, and for a `/raft` call the caller's `x-graft-call`, `x-graft-from` and `x-graft-sent` headers) joined to the
row's member trace (`tsave-<row>.jsonl[.xz]`).

Why Workers Logs: inside a long `sync()` the leader isolate's `Date.now()` can read seconds behind its callers' clocks
(m3fgb1: up to 3.3 s), so the member trace alone cannot say when the leader handled a call. Workers Logs stamps each
event's start on the platform's clock, and its `wallTimeMs` runs until the response leaves, output gate included. So
per `/raft` call:
  in    = event start - the caller's `x-graft-sent`     (wire there plus the leader's input queue; a negative value is
                                                          the caller's clock ahead of the platform's)
  hold  = event wall - the handler's time on the leader's own clock (`CallArrived` 20 to `CallLeft` 24)
                                                         (the response held after the handler returned: the output
                                                          gate behind earlier writes) [inference: nothing else awaits]
and over the window, the longest span with no leader event start (the input stop).

Usage:
  leaderwl.py fetch SERVICE DO_ID FROM_ISO TO_ISO OUT.json   pull the object's events (1 s chunks, needs
                                                             CLOUDFLARE_ACCOUNT_ID and CLOUDFLARE_API_TOKEN)
  leaderwl.py split OUT.json TSAVE T0_MS                     the per-half-second table from T0_MS (a time on the
                                                             platform's clock, such as the burst's first cut)
"""
import ast, collections, json, lzma, os, sys, urllib.request
from datetime import datetime


def pct(v, p):
    v = sorted(v)
    return v[min(len(v) - 1, int(p * len(v)))] if v else None


def fetch(service, do_id, frm, to, out):
    acct, tok = os.environ['CLOUDFLARE_ACCOUNT_ID'], os.environ['CLOUDFLARE_API_TOKEN']
    ms = lambda s: int(datetime.fromisoformat(s.replace('Z', '+00:00')).timestamp() * 1000)
    a, b, ev = ms(frm), ms(to), []
    for t in range(a, b, 1000):
        f = [{'key': '$metadata.service', 'operation': 'eq', 'type': 'string', 'value': service},
             {'key': '$workers.durableObjectId', 'operation': 'eq', 'type': 'string', 'value': do_id}]
        body = {'queryId': 'q', 'timeframe': {'from': t, 'to': min(b, t + 1000)}, 'view': 'events', 'limit': 2000,
                'parameters': {'filters': f, 'limit': 2000}}
        url = f'https://api.cloudflare.com/client/v4/accounts/{acct}/workers/observability/telemetry/query'
        req = urllib.request.Request(url, data=json.dumps(body).encode(), method='POST',
                                     headers={'authorization': f'Bearer {tok}', 'content-type': 'application/json'})
        for _ in range(3):
            try:
                ev += json.load(urllib.request.urlopen(req, timeout=90))['result']['events']['events']
                break
            except Exception as e:  # a long window or a busy API answers 400/403; retry the chunk
                print('retry', t, e, file=sys.stderr)
    json.dump(ev, open(out, 'w'))
    print(f'{len(ev)} events')


def split(evpath, tsave, t0):
    ev = json.load(open(evpath))
    wl, starts = {}, []
    for e in ev:
        w = e['$workers']
        starts.append(e['timestamp'])
        evn = w.get('event') or {}
        if evn.get('path') != '/raft':
            continue
        req = evn.get('request')
        h = (req if isinstance(req, dict) else ast.literal_eval(req)).get('headers', {})
        if h.get('x-graft-call'):
            key = (int(h.get('x-graft-from', 0)), int(h['x-graft-call']))
            wl[key] = (e['timestamp'], w.get('wallTimeMs') or 0, int(h.get('x-graft-sent', 0)))
    arr, left, syncs, seen = {}, {}, [], set()
    op = lzma.open if tsave.endswith('.xz') else open
    for line in op(tsave, 'rt'):
        d = json.loads(line)
        if d.get('site') != 0 or 'learner' in d or 'rows' not in d:
            continue
        for r in d['rows']:
            if tuple(r) in seen:
                continue
            seen.add(tuple(r))
            if r[1] == 20:
                arr[(int(r[2]), int(r[3]))] = r[0]
            elif r[1] == 24:
                left[(int(r[2]), int(r[3]))] = r[0]
            elif r[1] == 31 and float(r[5]) >= 300:
                syncs.append((r[0], int(float(r[5]))))
    by = collections.defaultdict(list)
    for k, (ts, wall, sent) in wl.items():
        if k in arr and k in left:
            by[(ts - t0) // 500 * 500].append((ts - sent, wall, wall - (left[k] - arr[k])))
    print("leader confirms of 300 ms or more (leader clock end, wall):", syncs)
    print('half-second from T0 | /raft calls | in p50 | wall p50 max | hold p50 max')
    for b, v in sorted(by.items()):
        if -2000 <= b < 15000:
            c = lambda i: [x[i] for x in v]
            print(f' {b:>6} {len(v):>4} | {pct(c(0), .5):>6} | {pct(c(1), .5):>4} {max(c(1)):>4} | '
                  f'{pct(c(2), .5):>4} {max(c(2)):>4}')
    starts.sort()
    gaps = sorted((y - x, x - t0) for x, y in zip(starts, starts[1:]) if -2000 <= x - t0 < 15000)[-5:]
    print('longest spans with no leader event start (ms, starting ms from T0):', gaps)


if __name__ == '__main__':
    if sys.argv[1] == 'fetch':
        fetch(*sys.argv[2:7])
    else:
        split(sys.argv[2], sys.argv[3], int(sys.argv[4]))
