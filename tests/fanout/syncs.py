#!/usr/bin/env python3
"""sync-trace (`decisions/OPEN-QUESTIONS.md`, F106): each storage confirm a member logged (`Trace::Sync`, kind 31,
`sync_row` in workers/site/src/host.rs), and what ran in its isolate while it was pending.

Usage: syncs.py TRACE.jsonl[.xz] [T0_MS SECONDS] [SLOW_MS]

Per site it prints the confirms' wall p50/p99/max, then every confirm of SLOW_MS (default 1,000) or more with its
statements and bytes, the ticker wakes and requests that ran inside it, and the loop lag at its start and end, and a
verdict for each:
  gate held     no wake and no request inside, and the end lag is at least half its wall: nothing in the isolate ran
                while it was pending, so the input gate (or the runtime) held the object for the storage write;
  isolate late  the start lag is already at least half its wall: the isolate was behind before the write began;
  storage       wakes or requests ran inside it: the isolate was free and the write itself was slow.
"""
import json, lzma, sys

path = sys.argv[1]
t0 = int(sys.argv[2]) if len(sys.argv) > 3 else 0
t1 = t0 + 1000 * int(sys.argv[3]) if len(sys.argv) > 3 else 1 << 62
slow = int(sys.argv[4]) if len(sys.argv) > 4 else 1000


def unpack(r):
    batch, index, ms = int(float(r[3])), int(float(r[4])), int(float(r[5]))
    return {'at': r[0], 'wall': ms, 'wakes': int(r[2]), 'inputs': batch & 0xfffff, 'lag_begin': (batch >> 20) & 0xffff,
            'lag_end': batch >> 36, 'stmts': index & 0xfffff, 'bytes': index >> 20}


def verdict(s):
    if s['lag_begin'] * 2 >= s['wall']:
        return 'isolate late'
    if s['wakes'] == 0 and s['inputs'] == 0 and s['lag_end'] * 2 >= s['wall']:
        return 'gate held'
    if s['wakes'] or s['inputs']:
        return 'storage'
    return 'unclear'


if __name__ == '__main__':
    seen, by = set(), {}
    for l in (lzma.open if path.endswith('.xz') else open)(path, 'rt'):
        d = json.loads(l)
        if 'rows' not in d or 'learner' in d:
            continue
        for r in d['rows']:
            t = (d.get('site'),) + tuple(r)
            if r[1] != 31 or t in seen or not t0 <= r[0] < t1:
                continue
            seen.add(t)
            by.setdefault(d.get('site'), []).append(unpack(r))
    if not by:
        sys.exit('no kind 31 rows in the window')
    pc = lambda xs, q: sorted(xs)[min(len(xs) - 1, int(q * len(xs)))]
    for site, ss in sorted(by.items(), key=lambda x: str(x[0])):
        walls = [s['wall'] for s in ss]
        print(f"site {site}: {len(ss)} confirms, wall p50 {pc(walls, .5)} p99 {pc(walls, .99)} max {max(walls)} ms")
        for s in sorted(ss, key=lambda s: s['at']):
            if s['wall'] >= slow:
                print(f"  at {s['at']} wall {s['wall']} ms, {s['stmts']} stmts {s['bytes']} B, wakes {s['wakes']} "
                      f"requests {s['inputs']}, lag {s['lag_begin']} -> {s['lag_end']} ms: {verdict(s)}")
