#!/usr/bin/env python3
"""A37's replay (`decisions/OPEN-QUESTIONS.md`): how late the leader's read-index grants reached the members on a B0-32
row, how much of that lateness could be the leader's output gate, and the read gate's repository read if grants left
past the gate.

Usage: grantgate.py TSAVE.jsonl[.xz] T_MS

T_MS is the burst's first leader cut on the leader's clock (m3fgb1: 1795223930000; m3fpc1: 1795203056000).

It joins each member's `/raft` call to the leader by its call id: `CallSent` (kind 23) and `AnswerArrived` (21) on the
member's clock, `CallArrived` (20) and `CallLeft` (24) on the leader's. It prints:
  - the members' round trip to the leader by send second, on their own clocks;
  - the leader's clock against its callers': each `CallArrived` stamp against the sender's `x-graft-sent` stamp. A call
    is never handled before it is sent, so a positive difference is how far the leader's clock read behind;
  - a split of each round trip: `before` is at least the time from the member's send to the latest send among the calls
    the leader stamped within 30 ms of this one, which the leader cannot have handled earlier; `after` is the rest, at
    most the wire back plus anything that held the answer (the output gate);
  - the read gate's repository read (up to three member tries, each `ov` at about 1.15 s), replayed from reads starting
    each 5 ms with the round trips of calls sent within 150 ms: as traced, with `after` cut to 80 ms for calls whose
    `before` is under 300 ms (conservative), and with each call's leader-side time capped at the leader's own input stops
    (least gain).
"""
import bisect, collections, json, lzma, random, sys


def load(path):
    seen, rows = set(), []
    op = lzma.open if path.endswith('.xz') else open
    for line in op(path, 'rt'):
        d = json.loads(line)
        if 'rows' not in d:
            continue
        who = f"L{d['learner']}" if 'learner' in d else f"S{d['site']}"
        for r in d['rows']:
            t = (who,) + tuple(r)
            if t not in seen:
                seen.add(t)
                rows.append(t)
    return rows


def pct(v, p):
    v = sorted(v)
    return v[min(len(v) - 1, int(p * len(v)))] if v else None


def main():
    rows, T = load(sys.argv[1]), int(sys.argv[2])
    who = lambda a: f'L{a}' if a >= 192 else f'S{a}'
    sent = {(r[0], int(r[4])): r[1] for r in rows if r[0] != 'S0' and r[2] == 23 and int(r[3]) == 0}
    ans = {(r[0], int(r[4])): r[1] for r in rows if r[0] != 'S0' and r[2] == 21 and int(r[3]) == 0}
    arr = {(who(int(r[3])), int(r[4])): (r[1], int(float(r[5]))) for r in rows if r[0] == 'S0' and r[2] == 20}

    by = collections.defaultdict(list)
    for k, s in sent.items():
        if k in ans and T - 2000 <= s < T + 12000:
            by[(s - T) // 1000].append(ans[k] - s)
    print('member round trip to the leader by send second (s, n, p50, p90, max):')
    print(' ', [(b, len(v), pct(v, .5), pct(v, .9), max(v)) for b, v in sorted(by.items())])

    lag = collections.defaultdict(list)
    for at, s in arr.values():
        if T - 2000 <= at < T + 12000 and s > 1e12:
            lag[(at - T) // 1000].append(s - at)
    print("sender's stamp minus the leader's at arrival, by leader second (s, p50, max):")
    print(' ', [(b, pct(v, .5), max(v)) for b, v in sorted(lag.items())])

    stamps = collections.defaultdict(list)
    for k, (at, _) in arr.items():
        if k in sent:
            stamps[at].append(sent[k])
    keys = sorted(stamps)

    def handled_after(at):
        i, j = bisect.bisect_left(keys, at - 30), bisect.bisect_right(keys, at + 30)
        return max(max(stamps[x]) for x in keys[i:j])

    entered = sorted(r[1] for r in rows if r[0] == 'S0' and r[2] == 25 and T - 2000 <= r[1] < T + 12000)
    stops = [(a, b) for a, b in zip(entered, entered[1:]) if b - a >= 150]

    def taken(t):
        for a, b in stops:
            if a < t < b:
                return b
        return t

    calls = []
    for k, s in sent.items():
        if k in ans and k in arr and T + 2000 <= s < T + 9000:
            rt, h = ans[k] - s, handled_after(arr[k][0])
            before, after = max(0, h - s), ans[k] - max(h, s)
            conservative = rt if before >= 300 else rt - after + 80
            lower = min(rt, taken(s + 40) - s + 40)
            calls.append((s, rt, conservative, lower, before, after))
    calls.sort()
    col = lambda i: [c[i] for c in calls]
    print(f'member calls sent T+2..9 s: {len(calls)}; round trip p50 {pct(col(1), .5)} p90 {pct(col(1), .9)}, '
          f'1 s or more {sum(x >= 1000 for x in col(1))}')
    print(f"  before (at least) p50 {pct(col(4), .5)} p90 {pct(col(4), .9)}; after (at most) p50 {pct(col(5), .5)} "
          f"p90 {pct(col(5), .9)}")
    print(f'  1 s or more: conservative {sum(x >= 1000 for x in col(2))}, lower bound {sum(x >= 1000 for x in col(3))}')

    starts = col(0)
    rng = random.Random(3)

    def gate_read(t, i):
        s = t
        for _ in range(3):
            a, b = bisect.bisect_left(starts, t - 150), bisect.bisect_right(starts, t + 150)
            x = calls[rng.randrange(a, b)][i] if a < b else 300
            if x < 1000:
                return t + x - s
            t += 1150
        return t - s

    for lo, hi in ((2000, 9000), (2700, 6000)):
        for label, i in (('as traced', 1), ('conservative', 2), ('least gain', 3)):
            v = [gate_read(T + x, i) for x in range(lo, hi, 5)]
            print(f'gate repo read, reads starting T+{lo / 1000}..{hi / 1000} s, {label}: p50 {pct(v, .5)} '
                  f'p90 {pct(v, .9)}, 3 s or more {100 * sum(y >= 3000 for y in v) / len(v):.0f}%')


if __name__ == '__main__':
    main()
