#!/usr/bin/env python3
"""hostwork.py TSAVE SITE@FROM@TO [SITE@FROM@TO ...] -> is a slow leader a slower host or more work? (nochange-row.md)

For each leadership given, from one row's trace (`workers/site/src/host.rs`, `Trace`):
- the leader's inputs a second by route and sender (`Entered` 25: route `index % 64` in `ROUTES`, sender `peer`: 255 an
  edge or harness request, 0 to 2 a voter, 192 and up a learner);
- the edge's requests a second to every member, voters and learners, by route (`Entered` with peer 255 on each);
- ticker stalls of 500 ms or more (`Lag` 8), confirm p90 (`Sync` 31), and the leader's turn work (`Work` 30: appends,
  entry bytes encoded, statements, applies) per input, so the same inputs can be told from heavier ones;
- the voters' `/suspect` reports (`Reported` 28) a second, by the member they are about, before and after the first
  minute with a stall burst.
FROM and TO are ISO times on Cloudflare's clock, as the trace's rows are."""
import collections
import datetime
import json
import lzma
import sys

ROUTES = ['other', '/raft', '/read', '/commit', '/commit_many', '/outcome', '/outcome_by_request', '/suspect',
          '/campaign', '/status', '/stats', '/trace', '/ping', '/rtt', '/place', '/learners', '/record']


def ms(iso):
    return int(datetime.datetime.fromisoformat(iso.replace('Z', '+00:00')).timestamp() * 1000)


def pct(a, p):
    a = sorted(a)
    return a[min(len(a) - 1, int(p * len(a)))] if a else None


def route(r):
    c = int(r[4]) % 64
    return ROUTES[c] if c < len(ROUTES) else 'other'


def main(trace, spans):
    rows = collections.defaultdict(set)
    for line in (lzma.open if trace.endswith('.xz') else open)(trace, 'rt'):
        d = json.loads(line)
        if 'rows' not in d:
            continue
        who = ('l', d['learner']) if d.get('learner') is not None else ('s', d.get('site'))
        for r in d['rows']:
            if r[1] in (8, 25, 28, 30, 31):
                rows[who].add(tuple(r))
    for span in spans:
        s, a, b = span.split('@')
        site, a, b = int(s), ms(a), ms(b)
        secs = (b - a) / 1000
        L = [r for r in rows[('s', site)] if a <= r[0] < b]
        ent = [r for r in L if r[1] == 25]
        by = collections.Counter((('edge' if int(r[2]) == 255 else 'learner' if int(r[2]) >= 192 else 'voter'), route(r))
                                 for r in ent)
        stalls = [int(float(r[5])) for r in L if r[1] == 8 and int(float(r[5])) >= 500]
        conf = [int(float(r[5])) for r in L if r[1] == 31]
        work = [r for r in L if r[1] == 30]
        appends = sum(int(r[2]) for r in work)
        abytes = sum(int(float(r[3])) for r in work)
        stmts = sum(int(float(r[4])) & ((1 << 20) - 1) for r in work)
        applied = sum(int(float(r[5])) & 0xffff for r in work)
        print(f'site {site} {datetime.datetime.fromtimestamp(a / 1000, datetime.UTC):%H:%M:%S}+{secs:.0f}s: '
              f'inputs {len(ent) / secs:.1f}/s, stalls>=500 {len(stalls)} ({sum(stalls) / 1000:.1f} s, '
              f'{len(stalls) / secs * 60:.1f}/min), confirm p50/p90 {pct(conf, .5)}/{pct(conf, .9)}')
        print('  by sender and route /s: ' + ', '.join(f'{k[0]} {k[1]} {v / secs:.1f}' for k, v in by.most_common(9)))
        n = max(1, len(ent))
        print(f'  turn work per input: appends {appends / n:.3f}, entry bytes {abytes / n:.0f}, statements '
              f'{stmts / n:.3f}, applied {applied / n:.3f}; per s: appends {appends / secs:.1f}, '
              f'{abytes / secs / 1e6:.2f} MB encoded, statements {stmts / secs:.0f}')
        edge = collections.Counter()
        for who, rs in rows.items():
            for r in rs:
                if r[1] == 25 and a <= r[0] < b and int(r[2]) == 255:
                    edge[(who[0] + str(who[1]), route(r))] += 1
        tot = collections.Counter()
        for (w, rt), v in edge.items():
            tot[rt] += v
        print('  edge -> all members /s by route: ' + ', '.join(f'{k} {v / secs:.1f}' for k, v in tot.most_common(7)))
        print('  edge -> each member /s: ' + ', '.join(f'{w} {sum(v for (x, _), v in edge.items() if x == w) / secs:.1f}'
                                                     for w in sorted({x for x, _ in edge})))
        # /suspect reports stepped on voters, by the member they are about, per 30 s.
        rep = collections.defaultdict(collections.Counter)
        for who, rs in rows.items():
            if who[0] != 's':
                continue
            for r in rs:
                if r[1] == 28 and a <= r[0] < b:
                    rep[(r[0] - a) // 30000][int(r[2])] += 1
        stall30 = collections.Counter((r[0] - a) // 30000 for r in L if r[1] == 8 and int(float(r[5])) >= 500)
        print('  per 30 s: stalls | /suspect reports on voters by member about: ' + '; '.join(
            f'{w}: {stall30[w]} | {dict(rep[w])}' for w in range(int(secs // 30) + 1)))


if __name__ == '__main__':
    if len(sys.argv) < 3:
        sys.exit(__doc__)
    main(sys.argv[1], sys.argv[2:])
