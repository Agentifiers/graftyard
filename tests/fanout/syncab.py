#!/usr/bin/env python3
"""syncab.py TSAVE SITE@FROM@TO [SITE@FROM@TO ...] -> the leader-sync A/B's per-arm verdict lines from one row's trace
(`tests/am1/results/sync-ab.md`), for each leadership given: the leader's input requests a second by route (kind 25
`Entered`, route code `index % 64` in `ROUTES`; `/raft` from learners, peer 192 and up, apart), its storage confirm walls (kind 31 `Sync`) at p50, p90 and p99, its
ticker stalls of 500 ms or more (kind 8 `Lag`), and its held-input batches (one millisecond in which 50 or more
requests entered, `stallline.py`'s test). FROM and TO are ISO times on Cloudflare's clock, as the trace's rows are.
Commit p99 and agent commands a second come from the a1 row and `cuts.py`, not from here."""
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


def main(trace, spans):
    want = [(int(s), ms(a), ms(b)) for s, a, b in (x.split('@') for x in spans)]
    rows = collections.defaultdict(set)
    for line in (lzma.open if trace.endswith('.xz') else open)(trace, 'rt'):
        d = json.loads(line)
        if 'rows' not in d or d.get('learner') is not None or d.get('site') is None:
            continue
        for r in d['rows']:
            if r[1] in (8, 25, 31):
                rows[d['site']].add(tuple(r))
    for site, a, b in want:
        secs = (b - a) / 1000
        R = [r for r in rows[site] if a <= r[0] < b]
        ent = collections.Counter(ROUTES[int(r[4]) % 64] if int(r[4]) % 64 < len(ROUTES) else 'other'
                                  for r in R if r[1] == 25)
        total = sum(ent.values())
        confirms = [int(float(r[5])) for r in R if r[1] == 31]
        stalls = sorted(int(float(r[5])) for r in R if r[1] == 8 and int(float(r[5])) >= 500)
        per_ms = collections.Counter(r[0] for r in R if r[1] == 25)
        batches = [n for n in per_ms.values() if n >= 50]
        print(f'site {site} {datetime.datetime.fromtimestamp(a / 1000, datetime.UTC):%H:%M:%S} to '
              f'{datetime.datetime.fromtimestamp(b / 1000, datetime.UTC):%H:%M:%S} ({secs:.0f} s)')
        learner_raft = sum(1 for r in R if r[1] == 25 and int(r[4]) % 64 == 1 and int(r[2]) >= 192)
        print(f'  inputs {total / secs:.1f}/s: ' + ', '.join(f'{k} {v / secs:.1f}' for k, v in ent.most_common())
              + f'; of /raft, from learners {learner_raft / secs:.1f}')
        print(f'  confirm wall n {len(confirms)} p50 {pct(confirms, .5)} p90 {pct(confirms, .9)} p99 {pct(confirms, .99)} '
              f'max {max(confirms) if confirms else None} ms')
        print(f'  ticker stalls >= 500 ms: {len(stalls)}, {sum(stalls)} ms in total, max {max(stalls) if stalls else 0}')
        print(f'  held-input batches (>= 50 requests in one ms): {len(batches)}, holding {sum(batches)} of {total} '
              f'requests, largest {max(batches) if batches else 0}')


if __name__ == '__main__':
    if len(sys.argv) < 3:
        sys.exit(__doc__)
    main(sys.argv[1], sys.argv[2:])
