#!/usr/bin/env python3
"""heavyturns.py TAG [TAG ...]: what fills the repository leader's heavy turns, from the row's leader trace
(`tsave-TAG.jsonl.xz`) and its Workers Logs extract (`wlobj.py fetch`), for tests/am1/results/b033-pfa.md.

workerd bills an actor's CPU to the request delivered most recently, until the next one is delivered
(`src/workerd/io/io-context.c++`, `IoContext::IncomingRequest::delivered`: "attribute accumulated CPU time" to the old
front). So an invocation's `cpuTimeMs` is the CPU of every turn that ran between its delivery and the next one,
whichever request the work belongs to [doc, workerd source]. A test install's handler writes its `Entered` row (kind 25)
first, so in ring order the rows between one `/raft` call's `Entered` and the next `Entered` are the work its CPU paid
for. No clock is read: the leader's clock runs seconds late under load (`b033-percmd.md`).

For every leader `/raft` call joined exactly to Workers Logs on (caller, `x-graft-call`), in 90 s to the run's end, it
sums that slice's Work (30), Sync (31), Confirm (10), Flushed (16), Cut (29) and CallSent (23) rows and classes the
slice by its heaviest content: applies entries, writes without an apply (the deferred transaction), storage confirm,
cuts or sends, resolutions only, or no traced work. Run from the repository root.
"""
import bisect, collections, json, lzma, os, sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), 'b033probes', 'host'))
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import RUNS, ms  # noqa: E402
from tracerows import merge  # noqa: E402

HEAVY = 20
WL = {'ms': 'wl-ms', 'mr': 'wl-mr', 'pg': 'wl-pg', 'pf': 'wl-pf', 'bl': 'wl-bl', 'ql': 'wl-ql', 'm3': 'wl-m3'}
RAW = os.environ.get('B033_RAW', 'tests/fanout/raw')


def trace(tag):
    """Site 0's rows in ring order, merged by `tracerows.merge`: equal rows in one save are distinct events."""
    saves = (json.loads(line) for line in lzma.open(f'{RAW}/tsave-{tag}.jsonl.xz', 'rt'))
    rows = merge((0, d['rows']) for d in saves if d.get('site') == 0 and 'learner' not in d and 'rows' in d)
    return [list(r) for r in rows[0]]


def slices(tag):
    rows = trace(tag)
    a = ms(RUNS[tag][0])
    lo, hi = a + 90_000, a + RUNS[tag][1] * 1000
    pos = {(r[2], r[3]): i for i, r in enumerate(rows) if r[1] == 25 and r[3]}
    n = len(rows)
    nxt = [n] * (n + 1)
    for i in range(n - 1, -1, -1):
        nxt[i] = i if rows[i][1] == 25 else nxt[i + 1]
    out = []
    for x in json.load(lzma.open(f'{RAW}/{WL[tag[:2]]}/{tag}-f0.json.xz', 'rt')):
        if x.get('path') != '/raft' or not x.get('call'):
            continue
        i = pos.get((int(x.get('from') or 0), int(x['call'])))
        if i is None or not lo <= rows[i][0] < hi:
            continue
        c = collections.Counter(cpu=x.get('cpu') or 0)
        for r in rows[i + 1:nxt[i + 1]]:
            k = r[1]
            if k == 30:
                c['work'] += 1
                c['appends'] += r[2]
                c['stmts'] += r[4] & 0xfffff
                c['stmtKB'] += (r[4] >> 20) / 1024
                c['applied'] += r[5] & 0xffff
                c['events'] += (r[5] >> 16) & 0xffff
                c['resolutions'] += r[5] >> 32
            elif k in (10, 31):
                c['confirm'] += 1
            elif k == 16:
                c['flushed'] += 1
                c['flushStmts'] += r[4]
            elif k in (23, 29):
                c['sends'] += 1
        out.append(c)
    return out


def kind(c):
    if c['applied']:
        return 'applies entries'
    if c['flushed'] or c['stmts']:
        return 'deferred transaction'
    if c['confirm']:
        return 'storage confirm'
    if c['sends'] or c['appends']:
        return 'cuts or sends'
    if c['resolutions']:
        return 'resolutions only'
    return 'no traced work'


for tag in sys.argv[1:]:
    cs = slices(tag)
    tot = sum(c['cpu'] for c in cs)
    heavy = [c for c in cs if c['cpu'] >= HEAVY]
    print(f'{tag}: {len(cs)} /raft slices, {sum(c["cpu"] for c in cs) / max(len(cs), 1):.2f} ms CPU a slice; '
          f'{len(heavy)} of {HEAVY} ms or more carry {100 * sum(c["cpu"] for c in heavy) / max(tot, 1):.0f}%')
    by = collections.defaultdict(list)
    for c in cs:
        by[kind(c)].append(c)
    for k, v in sorted(by.items(), key=lambda kv: -sum(c['cpu'] for c in kv[1])):
        cpu = sum(c['cpu'] for c in v)
        s = sorted(c['cpu'] for c in v)
        line = (f'  {k:21} n {len(v):6}  CPU {100 * cpu / max(tot, 1):3.0f}%  mean {cpu / len(v):6.1f}  '
                f'p50 {s[len(s) // 2]:4}  p90 {s[int(.9 * len(s))]:4}')
        if k == 'applies entries':
            ent, ev = sum(c['applied'] for c in v), sum(c['events'] for c in v)
            st, kb, res = (sum(c[k] for c in v) / ent for k in ('stmts', 'stmtKB', 'resolutions'))
            line += (f'  | {ev / ent:.0f} events an entry, {cpu / ent:.1f} ms an entry, {cpu / ev:.2f} ms an event; '
                     f'{st:.0f} statements and {kb:.0f} KB queued and {res:.0f} resolutions an entry')
        if k == 'deferred transaction':
            fl = [c for c in v if c['flushed']]
            if fl:
                st, fc = sum(c['flushStmts'] for c in fl), sum(c['cpu'] for c in fl)
                line += (f'  | {len(fl)} ran a deferred transaction: {st / len(fl):.0f} statements, '
                         f'{fc / len(fl):.1f} ms, {fc / max(st, 1):.2f} ms a statement')
        print(line)
    ap = [c for c in cs if c['applied']]
    if len(ap) > 5:
        def r(k):
            xs, ys = [c[k] for c in ap], [c['cpu'] for c in ap]
            mx, my = sum(xs) / len(xs), sum(ys) / len(ys)
            sxy = sum((x - mx) * (y - my) for x, y in zip(xs, ys))
            sxx, syy = sum((x - mx) ** 2 for x in xs), sum((y - my) ** 2 for y in ys)
            return sxy / (sxx * syy) ** .5 if sxx and syy else 0
        print(f'  apply slices: CPU against events r {r("events"):.2f}, statements queued r {r("stmts"):.2f}, '
              f'KB queued r {r("stmtKB"):.2f}, resolutions r {r("resolutions"):.2f}')
