#!/usr/bin/env python3
"""agentdrain.py TAG [FIVEXX.jsonl]: what a B0-33 leader drained of its agent lane, and what it would have advertised,
from the row's leader trace (`tsave-TAG.jsonl.xz`, `Trace::Cut` rows, kind 29), for tests/am1/results/edge-refusal.md.

Per 10 s window of cuts on the leader's ring order: agent commands cut a second, the share of cuts that left agent
commands queued, the agent queue, and the advertised wait (queue over drain) in windows where most cuts left agent
commands queued. Then the agent bucket the leader sized itself from (AGENT_SHARE x ENTRY_SOFT / cmd_bytes x entries a
second, with cmd_bytes replayed as `Admission::cmd_bytes`'s moving mean of soft bytes a command) against that drain.

With FIVEXX, a file of `{"t": "HH:MM:SS", "c": {"kind:shed|full|other": n}}` lines per 20 s bucket (the edge's
Workers Logs `fivexx` lines grouped by kind and reason), it joins each bucket to the advert sampled at 5, 10, 15 and
20 s into it and prints the 503s an edge refusal at a 5 s bound would have answered as clean `full`s: every leader
`full` the edge rendered 503 (the render fix), and each bucket's sheds times the share of its samples whose advert
passed 5 s. That share is an upper bound: GraphQL's sheds include human writes, which are never refused. Run from the
repository root.
"""
import collections, json, lzma, os, sys
from datetime import datetime, timezone

RAW = os.environ.get('B033_RAW', 'tests/fanout/raw')
W = 10_000
BOUND_S = 5.0
SOFT, SHARE = 100_000, 0.5


def cuts(tag):
    rows = set()
    # A row's own tsave is plain jsonl until it is committed as .xz (b033.sh reads it in place).
    xz = f'{RAW}/tsave-{tag}.jsonl.xz'
    for line in lzma.open(xz, 'rt') if os.path.exists(xz) else open(f'{RAW}/tsave-{tag}.jsonl'):
        d = json.loads(line)
        if 'rows' not in d or str(d.get('site')) != str(d.get('leader')):
            continue
        rows.update(tuple(r) for r in d['rows'] if r[1] == 29)
    out = []
    for at, _, peer, batch, index, ms_ in sorted(rows):
        taken = [(batch >> (12 * i)) & 4095 for i in range(4)]
        left = [(index >> (13 * i)) & 8191 for i in range(4)]
        out.append((at, taken, left, peer % 16, ms_ & ((1 << 21) - 1)))
    return out


def advert(cs, t):
    xs = [c for c in cs if t - W <= c[0] < t]
    if not xs:
        return None
    rate = sum(c[1][3] for c in xs) / (W / 1000)
    q = xs[-1][2][3]
    return q / rate if rate > 0 else (99.0 if q else 0.0)


def main():
    tag = sys.argv[1]
    cs = cuts(tag)
    t0 = cs[0][0]
    by = collections.defaultdict(list)
    cmd = 1000.0
    for at, taken, left, end, soft in cs:
        n = sum(taken)
        for _ in range(n):
            cmd = 0.98 * cmd + 0.02 * (soft / n)
        by[(at - t0) // W].append((taken, left, cmd))
    drains, buckets = [], []
    for k in sorted(by):
        xs = by[k]
        lim = sum(1 for x in xs if x[1][3] > 0) / len(xs)
        drain = sum(x[0][3] for x in xs) / (W / 1000)
        q = sum(x[1][3] for x in xs) / len(xs)
        wait = q / drain if drain and lim >= 0.5 else None
        print(f'{k * 10:4d}s cuts {len(xs):3d} agent/s {drain:6.1f} limited {lim:4.2f} agentLeft mean {q:5.0f} '
              f'wait {("%.1f s" % wait) if wait else "-"}')
        if lim >= 0.5:
            drains.append(drain)
            bucket = SHARE * max(1.0, SOFT / max(xs[-1][2], 200.0)) * len(xs) / (W / 1000)
            buckets.append(bucket)
    if drains:
        d, b = sorted(drains), sorted(buckets)
        p = lambda s, f: s[min(len(s) - 1, int(len(s) * f))]
        print(f'saturated windows {len(d)}: drain p10/p50/p90 {p(d, .1):.1f} / {p(d, .5):.1f} / {p(d, .9):.1f} a second;'
              f' bucket p50 {p(b, .5):.0f}, {p(b, .5) / p(d, .5):.2f}x the drain')
    if len(sys.argv) > 2:
        day = datetime.fromtimestamp(t0 / 1000, timezone.utc).replace(hour=0, minute=0, second=0, microsecond=0)
        tot = collections.Counter()
        for line in open(sys.argv[2]):
            row = json.loads(line)
            h, m, s = map(int, row['t'].split(':'))
            t = int(day.timestamp() * 1000) + ((h * 60 + m) * 60 + s) * 1000
            ws = [w for w in (advert(cs, t + o) for o in (5_000, 10_000, 15_000, 20_000)) if w is not None]
            frac = sum(1 for w in ws if w > BOUND_S) / len(ws) if ws else 0.0
            shed = sum(v for k, v in row['c'].items() if k.endswith(':shed'))
            full = sum(v for k, v in row['c'].items() if k.endswith(':full'))
            tot['shed'] += shed
            tot['full503'] += full
            tot['shedOverBound'] += round(shed * frac)
        print(json.dumps(dict(tot)), f"clean fulls at most {tot['full503'] + tot['shedOverBound']}")


if __name__ == '__main__':
    main()
