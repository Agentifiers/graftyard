#!/usr/bin/env python3
"""The far-leg check (`tests/m0/results/farleg.md`): the leader's AppendEntries to its far voter, split by call size,
by calls to that voter out at issue, and by why a call was a re-send, from the leader's trace in each run's
`.stats.json(.gz)`.

Trace rows are [t, kind, peer, round, index, ms]:
- 12 queued: index the call's last entry, ms its body bytes;
- 13 issued;
- 14 ended: index 0 transport ok, ms since issued;
- 20 answered (AppendAnswered, test-faults builds from `m0-adaptive-append`): index 0 ok, 1 refused for a gap, 2
  refused by `verify`, 3 other refusal; ms the call's first entry.

A re-send is a far call whose last entry is at or below the highest last already queued to that voter. Its cause is
read from the calls before it:
- gap: within 5 ms of a call's answer that was refused for its gap (AppendAnswered 1);
- refused: within 5 ms of another refusal (2 or 3);
- tick: a call to the far voter had been out CALL_DEADLINE (1 s) or more;
- failed: within 5 ms of a transport failure;
- ahead answered: within 5 ms of an ok answer (the call ahead answered and the dropped calls go again);
- other.
Arms without AppendAnswered (main) fall back to the timing rules alone, and their gap and refused counts read 0.

    python3 tests/m0carried/farsplit.py <raw dir> <run prefix> [...]
"""
import collections, glob, gzip, json, sys


def load(f):
    return json.load(gzip.open(f) if f.endswith('.gz') else open(f))


def pct(v, p):
    v = sorted(v)
    return v[min(len(v) - 1, int(len(v) * p / 100))] if v else None


def calls_of(r):
    C = {}
    for (t, kind, peer, rnd, index, ms) in r['trace']:
        k = (peer, rnd)
        if kind == 12:
            C[k] = dict(peer=peer, q=t, last=index, b=ms, iss=None, end=None, ok=None, ans=None, ans_t=None)
        elif k not in C:
            continue
        elif kind == 13:
            C[k]['iss'] = t
        elif kind == 14 and C[k]['iss'] is not None:
            C[k]['end'] = C[k]['iss'] + ms
            C[k]['ok'] = index == 0
        elif kind == 20:
            C[k]['ans'] = index
            C[k]['ans_t'] = t
    return C


def report(raw, pre):
    files = sorted(glob.glob(f'{raw}/{pre}*.stats.json*'))
    size = collections.defaultdict(list)
    inflight = collections.defaultdict(list)
    why = collections.Counter()
    by_kind = {'first': [], 'resend': []}
    answers = collections.Counter()
    near_ms, n_far = [], 0
    for f in files:
        d = load(f)
        L = [m for m in d if m.get('r') and m['r'].get('role') is True]
        if not L:
            continue
        r = L[0]['r']
        a = r['appendMs']
        far = int(max(a, key=lambda k: pct(a[k], 50)))
        near = int(min(a, key=lambda k: pct(a[k], 50)))
        C = calls_of(r)
        farc = sorted((c for c in C.values() if c['peer'] == far and c['iss'] is not None), key=lambda c: c['q'])
        near_ms += [c['end'] - c['iss'] for c in C.values() if c['peer'] == near and c['end'] is not None and c['ok']]
        top = 0
        for i, c in enumerate(farc):
            n_far += 1
            if c['ans'] is not None:
                answers[c['ans']] += 1
            done = c['end'] is not None and c['ok']
            ms = c['end'] - c['iss'] if done else None
            out = sum(1 for x in farc[max(0, i - 16):i] if (x['end'] or 1e18) > c['iss'])
            if done:
                kb = c['b'] / 1000
                bin_ = '<20 KB' if kb < 20 else '20-40 KB' if kb < 40 else '40-60 KB' if kb < 60 else '60-80 KB' if kb < 80 else '80+ KB'
                size[bin_].append(ms)
                inflight[min(out, 4)].append(ms)
            if c['last'] > top:
                top = c['last']
                if done:
                    by_kind['first'].append(ms)
                continue
            if done:
                by_kind['resend'].append(ms)
            t = c['q']
            prev = farc[max(0, i - 16):i]
            near_end = [p for p in prev if p['end'] is not None and 0 <= t - p['end'] <= 5]
            outs = [p for p in prev if (p['end'] or 1e18) > t]
            if any(p['ans'] == 1 and p['ans_t'] is not None and 0 <= t - p['ans_t'] <= 5 for p in prev):
                k = 'gap refusal'
            elif any(p['ans'] in (2, 3) and p['ans_t'] is not None and 0 <= t - p['ans_t'] <= 5 for p in prev):
                k = 'other refusal'
            elif any(t - p['iss'] >= 990 for p in outs):
                k = 'tick (a call out 1 s+)'
            elif any(not p['ok'] for p in near_end):
                k = 'after a transport failure'
            elif near_end:
                k = 'ahead answered'
            else:
                k = 'other'
            why[k] += 1
    n = sum(why.values())
    print(f'== {pre}: {len(files)} runs, far calls {n_far}, re-sends {n} ({100 * n / max(n_far, 1):.1f}%)')
    print('   answers (0 ok, 1 gap, 2 verify, 3 other):', dict(sorted(answers.items())))
    for k, v in why.most_common():
        print(f'   re-send cause {k}: {v}')
    for k, v in by_kind.items():
        print(f'   far call ms, {k}: n {len(v)} p50 {pct(v, 50)} p99 {pct(v, 99)}')
    allc = by_kind['first'] + by_kind['resend']
    print(f'   far call ms, all: n {len(allc)} p50 {pct(allc, 50)} p99 {pct(allc, 99)}')
    for k in ['<20 KB', '20-40 KB', '40-60 KB', '60-80 KB', '80+ KB']:
        v = size.get(k, [])
        print(f'   far call ms by size {k}: n {len(v)} p50 {pct(v, 50)} p99 {pct(v, 99)}')
    for k in sorted(inflight):
        v = inflight[k]
        print(f'   far call ms with {k} far calls already out: n {len(v)} p50 {pct(v, 50)} p99 {pct(v, 99)}')
    print(f'   near call ms: n {len(near_ms)} p50 {pct(near_ms, 50)} p99 {pct(near_ms, 99)}')


if __name__ == '__main__':
    raw = sys.argv[1]
    for pre in sys.argv[2:]:
        report(raw, pre)
