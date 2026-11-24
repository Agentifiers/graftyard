#!/usr/bin/env python3
"""stallline.py TSAVE SITE FROM TO [MIN_MS] -> one member's ticker stalls lined up with its storage confirms, and
whether its inputs were held for those confirms (tests/am1/results/stall-line.md).

FROM and TO are UTC times of 2026-11-21 such as 00:12:00 (the m64r33 row; edit DAY for another). Reads kinds 8 (`Lag`),
16 (`Flushed`), 25 (`Entered`), 30 (`Work`) and 31 (`Sync`) of the member's trace.
- Per stall of MIN_MS (default 500) or more: the confirms pending in it, how much of it they cover, the largest one's
  statements, bytes, ticker wakes and requests inside, and the `Work` and `Entered` rows inside it.
- The input-gate test: requests that entered while a confirm was pending, and how many of them carry the clock value at
  which it resolved. An isolate's `Date.now()` moves only at I/O, so requests run back to back after one event share its
  time; many at the resolve's time means they waited for it. The baseline is the share of the window's milliseconds that
  are a resolve time."""
import bisect, collections, datetime, os, sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from tracerows import load

DAY = '2026-11-21'


def main(trace, site, frm, to, mn='500'):
    site, mn = int(site), int(mn)
    ts = lambda s: int(datetime.datetime.fromisoformat(f'{DAY}T{s}+00:00').timestamp() * 1000)
    A, B = ts(frm), ts(to)
    R = collections.defaultdict(list)
    keep = lambda r: A - 5000 <= r[0] < B + 5000 and r[1] in (8, 16, 25, 30, 31)
    for r in load(trace, keep=keep).get(site, []):
        R[r[1]].append(r)
    syncs = []
    for r in R[31]:
        b, i, ms = int(float(r[3])), int(float(r[4])), int(float(r[5]))
        syncs.append({'start': r[0] - ms, 'end': r[0], 'wall': ms, 'wakes': int(r[2]), 'inputs': b & 0xfffff,
                      'stmts': i & 0xfffff, 'bytes': i >> 20})
    stalls = sorted((r[0], int(float(r[5]))) for r in R[8] if A <= r[0] < B and int(float(r[5])) >= mn)
    entered = sorted(r[0] for r in R[25] if A <= r[0] < B)
    total = covered = 0
    for at, lag in stalls:
        s0 = at - lag
        ov = [s for s in syncs if s['start'] < at and s['end'] > s0]
        cover = max(0, min(at, max(s['end'] for s in ov)) - max(s0, min(s['start'] for s in ov))) if ov else 0
        total, covered = total + lag, covered + cover
        big = max(ov, key=lambda s: s['wall']) if ov else None
        work = sum(1 for r in R[30] if s0 <= r[0] <= at)
        ent = bisect.bisect_left(entered, at) - bisect.bisect_right(entered, s0)
        desc = (f"largest {big['wall']} ms, {big['stmts']} stmts {big['bytes']} B, wakes {big['wakes']} requests "
                f"{big['inputs']}") if big else 'no confirm'
        print(f'stall to {at} {lag:5d} ms: {len(ov)} confirms cover {cover:5d} ms; {desc}; work rows {work}, entered {ent}')
    print(f'{len(stalls)} stalls, {total} ms; covered by a pending confirm {covered} ms ({covered / max(total, 1):.0%})')
    ends = {s['end'] for s in syncs if A <= s['end'] < B}
    for lo, hi in ((0, 100), (100, 300), (300, 10**9)):
        inside = at_end = n = 0
        for s in syncs:
            if A <= s['end'] < B and lo <= s['wall'] < hi:
                n += 1
                inn = entered[bisect.bisect_right(entered, s['start']):bisect.bisect_right(entered, s['end'])]
                inside += len(inn)
                at_end += sum(1 for t in inn if t == s['end'])
        print(f'confirms of {lo} to {hi} ms: {n}; requests entered while pending {inside}, at the resolve time '
              f'{at_end} ({at_end / max(inside, 1):.0%})')
    print(f'baseline: resolve times are {len(ends) / (B - A):.2%} of the window\'s milliseconds')


if __name__ == '__main__':
    main(*sys.argv[1:6])
