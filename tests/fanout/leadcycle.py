"""B0-33's leader cycle: where each entry-to-entry cycle's time goes, per leadership, from a tsave trace.

    python3 tests/fanout/leadcycle.py TRACE.jsonl.xz SITE:FROM:TO [SITE:FROM:TO ...]

FROM and TO are ISO times (2026-11-21T00:12:01Z). For each entry k the leader appended, the cycle k -> k+1 is split
on the leader's own clock (`workers/site/src/host.rs`, `Trace`):
- round: k's first AppendQueued (12) to its first ok AppendEnded (14) from a voter;
- apply: that ack to the leader's Applied (4) for k;
- wait to cut: Applied to the cut (29) whose batch became k+1, when that cut came after it;
- tail: the later of that cut and Applied, to k+1's first AppendQueued: the deps read's overrun, `decide`, encode.
A cut's batch is entry (last index appended before it) + held + 1, from its `held` column. Rows of one millisecond keep
their trace order. "full" cycles are those whose entry k was cut `AgentCmds` or `AgentTokens`.

Then each voter call's leg (CallSent 23 to AnswerArrived 21, leader clock) less the follower's handler (CallLeft 24,
follower clock, entry to return), which leaves wire both ways, both input queues and the follower's output gate.
"""
import bisect
import collections
import json
import lzma
import sys
from datetime import datetime


def ms(s):
    return int(datetime.fromisoformat(s.replace('Z', '+00:00')).timestamp() * 1000)


def pc(xs, q):
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(q * len(xs)))] if xs else None


def mean(xs):
    return round(sum(xs) / len(xs)) if xs else None


def load(path):
    rows, seen = collections.defaultdict(list), set()
    for line in lzma.open(path, 'rt'):
        d = json.loads(line)
        if 'rows' not in d or 'learner' in d:
            continue
        for r in d['rows']:
            k = (d.get('site'),) + tuple(r)
            if k not in seen:
                seen.add(k)
                rows[d.get('site')].append(r)
    return rows


def cycles(rows, s, a, b):
    R = sorted((r for r in rows[s] if a <= r[0] < b), key=lambda r: r[0])
    app, first, acks = {}, {}, collections.defaultdict(list)
    for pos, r in enumerate(R):
        if r[1] == 4:
            app.setdefault(int(float(r[4])), r[0])
        if r[1] == 12 and r[2] < 192:
            first.setdefault(int(float(r[4])), (r[0], int(float(r[3])), pos))
        if r[1] == 14 and r[2] < 192 and int(float(r[4])) == 0:
            acks[int(float(r[3]))].append(r[0])
    idxs = sorted(first, key=lambda i: first[i][2])
    apos = [first[i][2] for i in idxs]
    feed = {}
    for pos, r in enumerate(R):
        if r[1] != 29:
            continue
        k = bisect.bisect_left(apos, pos) - 1
        if k >= 0:
            feed.setdefault(idxs[k] + (r[2] // 16) % 4 + 1, (r[0], r[2] % 16))
    out = []
    for i in idxs:
        n = i + 1
        if n not in first or n not in feed or i not in feed:
            continue
        t, rnd, _ = first[i]
        ack = min((x for x in acks.get(rnd, []) if x >= t), default=None)
        if ack is None:
            continue
        ap = max(app.get(i, ack), ack)
        ct, nxt = feed[n][0], first[n][0]
        if nxt - t > 5000:
            continue
        out.append({'full': feed[i][1] in (3, 4), 'round': ack - t, 'apply': ap - ack, 'frozen': ct <= ack,
                    'wait': max(0, ct - ap), 'tail': nxt - max(ct, ap), 'cycle': nxt - t})
    for full in (True, False):
        v = [o for o in out if o['full'] == full]
        if not v:
            continue
        tot = sum(o['cycle'] for o in v)

        def sh(k):
            return f"{mean([o[k] for o in v])} ({100 * sum(o[k] for o in v) // tot}%)"
        print(f"site {s} {'full ' if full else 'other'} n {len(v)} cycle {mean([o['cycle'] for o in v])} | round "
              f"{sh('round')} | apply {sh('apply')} | frozen before commit {sum(o['frozen'] for o in v)}/{len(v)} | "
              f"wait to cut {sh('wait')} | tail {sh('tail')}, p90 {pc([o['tail'] for o in v], .9)}")


def legs(rows, s, a, b):
    RL = [r for r in rows[s] if a <= r[0] < b]
    sent = {(r[2], int(float(r[3]))): r[0] for r in RL if r[1] == 23 and r[2] < 192}
    ans = {(r[2], int(float(r[3]))): r[0] for r in RL if r[1] == 21 and r[2] < 192}
    for f in sorted({p for p, _ in sent}):
        RF = [r for r in rows[f] if a - 3000 <= r[0] < b + 3000]
        left = {int(float(r[3])): (r[0], int(float(r[4]))) for r in RF if r[1] == 24 and r[2] == s}
        v = [(ans[(p, c)] - t, left[c][0] - left[c][1]) for (p, c), t in sent.items()
             if p == f and c in left and (p, c) in ans]
        if v:
            print(f"site {s} -> {f}: calls {len(v)} leg p50 {pc([x[0] for x in v], .5)} p90 {pc([x[0] for x in v], .9)}"
                  f" | handler p50 {pc([x[1] for x in v], .5)} | outside the handler p50 "
                  f"{pc([x[0] - x[1] for x in v], .5)}")


def main():
    rows = load(sys.argv[1])
    for spec in sys.argv[2:]:
        # SITE:FROM:TO, whose ISO times hold colons: split on the 'Z:' between them.
        s, span = spec.split(':', 1)
        a, b = span.split('Z:')
        a, b = ms(a + 'Z'), ms(b)
        cycles(rows, int(s), a, b)
        legs(rows, int(s), a, b)


if __name__ == '__main__':
    main()
