#!/usr/bin/env python3
"""legsplit.py TSAVE CALLEE CALLER APPLEGS.json DOCPUSEC.txt -> a leader's append time outside the follower's handler,
split by the AppendEntries body size and by the leader's CPU-seconds in the second it sent (round-remainder.md).

Joins as `applegs.py join` does. Per call, `outside` is the leader's round (CallSent to AnswerArrived, its isolate clock)
less the follower's invocation (Cloudflare's start to CallLeft): wire both ways, the leader's late send and its late
read of the answer. One clock per difference, so no skew estimate enters it. `body` is the bytes of the leader's last
`AppendQueued` to that follower within 300 ms before the send (0: an empty heartbeat or a read-index call). `cpu` is
`tests/fanout/docpusec.py`'s upper bound for the leader object in the send's second, keyed by the member clock.
`stale` is the leader's member clock less its isolate clock at send, as in applegs.py."""
import bisect, datetime, json, lzma, re, sys


def pc(v, p):
    return sorted(v)[min(len(v) - 1, int(p * len(v)))] if v else None


def main(trace, callee, caller, logs, dps):
    callee, caller = int(callee), int(caller)
    rows = set()
    for line in (lzma.open if trace.endswith('.xz') else open)(trace, 'rt'):
        d = json.loads(line)
        if 'rows' in d and d.get('site') in (callee, caller) and d.get('learner') is None:
            rows.update((d['site'],) + tuple(x) for x in d['rows'] if x[1] in (12, 21, 23, 24))
    left = {x[4]: (x[1], x[5]) for x in rows if x[0] == callee and x[2] == 24 and x[3] == caller}
    sent = {x[4]: x[1] for x in rows if x[0] == caller and x[2] == 23 and x[3] == callee}
    ans = {x[4]: x[1] for x in rows if x[0] == caller and x[2] == 21 and x[3] == callee}
    aq = sorted((x[1], x[6]) for x in rows if x[0] == caller and x[2] == 12 and x[3] == callee)
    at = [a[0] for a in aq]
    cpu = {}
    for line in open(dps).read().splitlines()[1:]:
        m = re.match(r'(\S+) (\d+) ([\d.]+)\.\.([\d.]+)', line)
        if m:
            cpu[m[1]] = float(m[4])
    J = []
    for r in json.load(open(logs)):
        c = int(r['call'] or 0)
        if c in left and c in sent and c in ans and r['clock']:
            (lt, _), s, start = left[c], sent[c], r['ts'] - r['wall']
            k = bisect.bisect_right(at, s) - 1
            body = aq[k][1] if k >= 0 and s - aq[k][0] <= 300 else 0
            sec = datetime.datetime.fromtimestamp(r['clock'] / 1000, datetime.UTC).strftime('%H:%M:%S')
            J.append({'outside': ans[c] - s - (lt - start), 'rtt': ans[c] - s, 'stale': r['clock'] - s,
                      'body': body, 'cpu': cpu.get(sec)})
    print(f'{caller}->{callee}: {len(J)} calls; outside p50 {pc([j["outside"] for j in J], .5)} '
          f'p90 {pc([j["outside"] for j in J], .9)}, rtt p50 {pc([j["rtt"] for j in J], .5)}')
    bands = (('body', ((0, 1), (1, 40000), (40000, 65536), (65536, 150000), (150000, 10**9)), lambda j: j['body']),
             ('cpu', ((0, .3), (.3, .6), (.6, 99)), lambda j: j['cpu']),
             ('stale', ((-10**6, 200), (200, 1000), (1000, 10**6)), lambda j: j['stale']))
    for name, cuts, key in bands:
        for lo, hi in cuts:
            v = [j for j in J if key(j) is not None and lo <= key(j) < hi]
            o = [j['outside'] for j in v]
            print(f'  {name} {lo}..{hi}: n {len(v)} outside p50 {pc(o, .5)} p90 {pc(o, .9)}')


if __name__ == '__main__':
    main(*sys.argv[1:6])
