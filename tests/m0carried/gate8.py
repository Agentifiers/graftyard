#!/usr/bin/env python3
"""B0-02's gate report for `m0-carried-8` (`tests/m0/results/carried-8.md`): per run, commit p99, the leader's
per-request CPU p99 and p999 (`durableObjectsInvocationsAdaptiveGroups`), its `/commit` turns of 80 ms CPU or more
(Workers observability), and its storage confirm p99 by entry size, plus confirm p99 by entry size over all runs.

A confirm (`Trace::Confirm`) names the applied index its transaction covered; the entry's size is the bytes of the
AppendEntries to a follower whose last entry is that index and which carried it alone (`Trace::AppendQueued`).

Usage, from the repository root with CLOUDFLARE_ACCOUNT_ID and CLOUDFLARE_API_TOKEN set:

    python3 tests/m0carried/gate8.py PREFIX [--no-cpu]
"""
import collections
import datetime
import glob
import json
import os
import sys
import time
import urllib.request

RAW = 'tests/m1/wp2/raw'
SITES = {0: 'graft-m3d-site-a', 1: 'graft-m3d-site-b', 2: 'graft-m3d-site-c'}
BINS = [(0, 40), (40, 60), (60, 80), (80, 100), (100, 130), (130, 10_000)]


def pct(xs, p):
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(p * len(xs)))] if xs else None


def leader(stats):
    return max((s for s in stats if s['r']), key=lambda s: len(s['r']['commitMs']))['r']


def confirms_by_size(lead):
    """(entry KB, confirm ms) for each confirm whose entry went to a follower alone."""
    size = {}
    for t in lead['trace']:
        if t[1] == 12:
            size.setdefault(t[4], t[5])
    prev_last = {}
    lasts = sorted(size)
    for a, b in zip(lasts, lasts[1:]):
        prev_last[b] = a
    out = []
    for t in lead['trace']:
        if t[1] == 10 and t[4] in size and prev_last.get(t[4]) == t[4] - 1:
            out.append((size[t[4]] / 1000, t[5]))
    return out


def binned(pairs):
    row = {}
    for lo, hi in BINS:
        xs = [ms for kb, ms in pairs if lo <= kb < hi]
        row[f'{lo}-{hi if hi < 10_000 else ""}'] = (len(xs), pct(xs, 0.99))
    return row


class Cpu:
    def __init__(self):
        self.acct, tok = os.environ['CLOUDFLARE_ACCOUNT_ID'], os.environ['CLOUDFLARE_API_TOKEN']
        self.h = {'authorization': f'Bearer {tok}', 'content-type': 'application/json'}
        url = f'https://api.cloudflare.com/client/v4/accounts/{self.acct}/workers/durable_objects/namespaces?per_page=1000'
        res = self.get(url)['result']
        self.ns = {n['script']: n['id'] for n in res if n.get('class') == 'Member' and (n.get('script') or '') in SITES.values()}

    def get(self, url, body=None):
        data = json.dumps(body).encode() if body is not None else None
        req = urllib.request.Request(url, data=data, headers=self.h, method='POST' if body is not None else 'GET')
        for attempt in range(4):
            try:
                return json.load(urllib.request.urlopen(req, timeout=180))
            except OSError:
                if attempt == 3:
                    raise
                time.sleep(5 * (attempt + 1))

    def gql(self, q, v):
        r = self.get('https://api.cloudflare.com/client/v4/graphql', {'query': q, 'variables': v})
        if r.get('errors'):
            raise SystemExit(r['errors'])
        return r['data']['viewer']['accounts'][0]

    def run(self, svc, start, end):
        v = {'a': self.acct, 's': start.strftime('%Y-%m-%dT%H:%M:%SZ'), 'e': end.strftime('%Y-%m-%dT%H:%M:%SZ')}
        qp = '''query($a:String!,$s:Time!,$e:Time!,$n:String!){viewer{accounts(filter:{accountTag:$a}){
          p: durableObjectsPeriodicGroups(limit:10000, filter:{datetime_geq:$s, datetime_lt:$e, namespaceId:$n}){
          sum{cpuTime} dimensions{objectId} } }}}'''
        cpu = collections.Counter()
        for x in self.gql(qp, {**v, 'n': self.ns[svc]})['p']:
            cpu[x['dimensions']['objectId']] += x['sum']['cpuTime']
        if not cpu:
            return None
        obj = cpu.most_common(1)[0][0]
        qi = '''query($a:String!,$s:Time!,$e:Time!,$o:String!,$w:String!){viewer{accounts(filter:{accountTag:$a}){
          d: durableObjectsInvocationsAdaptiveGroups(limit:100, filter:{datetime_geq:$s, datetime_lt:$e,
          scriptName:$w, objectId:$o}){ quantiles{cpuTimeP99 cpuTimeP999} dimensions{type} } }}}'''
        d = [x for x in self.gql(qi, {**v, 'o': obj, 'w': svc})['d'] if x['dimensions']['type'] == 'http']
        p99, p999 = (d[0]['quantiles']['cpuTimeP99'], d[0]['quantiles']['cpuTimeP999']) if d else (None, None)
        heavy = collections.Counter()
        a, b = int(start.timestamp() * 1000), int(end.timestamp() * 1000)
        for t in range(a, b, 4000):
            body = {'queryId': 'q', 'timeframe': {'from': t, 'to': t + 4000}, 'view': 'events', 'limit': 100,
                    'parameters': {'filters': [
                        {'key': '$metadata.service', 'operation': 'eq', 'type': 'string', 'value': svc},
                        {'key': '$workers.durableObjectId', 'operation': 'eq', 'type': 'string', 'value': obj},
                        {'key': '$workers.cpuTimeMs', 'operation': 'gte', 'type': 'number', 'value': 80}]}}
            url = f'https://api.cloudflare.com/client/v4/accounts/{self.acct}/workers/observability/telemetry/query'
            for e in self.get(url, body).get('result', {}).get('events', {}).get('events', []):
                w = e['$workers']
                u = (w.get('event', {}).get('request', {}) or {}).get('url') or w.get('eventType') or '?'
                heavy[u.split('/')[-1].split('?')[0] if '/' in u else u] += 1
        return {'cpuP99ms': p99 and round(p99 / 1000, 1), 'cpuP999ms': p999 and round(p999 / 1000, 1),
                'commit80': heavy.get('commit', 0), 'heavy80': dict(heavy)}


def main():
    prefix = sys.argv[1]
    cpu = None if '--no-cpu' in sys.argv else Cpu()
    every = []
    files = sorted(glob.glob(f'{RAW}/{prefix}-*.bench.json'), key=lambda f: json.load(open(f))['at'])
    for f in files:
        b = json.load(open(f))
        lead = leader(json.load(open(f.replace('.bench.', '.stats.'))))
        pairs = confirms_by_size(lead)
        every += pairs
        row = {'run': b['run'], 'perS': b['throughputPerS'], 'commitP99': b['commitMs']['p99'],
               'confirmP99': b['leaderConfirmMs']['p99'], 'cmdsPerEntry': b['commandsPerEntry'],
               'leader': b['leader'], 'failed': b['failed'], 'verdict': b['verdict'],
               'confirmP99ByKB': {k: v[1] for k, v in binned(pairs).items() if v[0]}}
        if cpu:
            end = datetime.datetime.fromisoformat(b['at'].replace('Z', '+00:00')) - datetime.timedelta(seconds=4)
            row.update(cpu.run(SITES[b['leader']], end - datetime.timedelta(seconds=56), end) or {})
        print(json.dumps(row))
        sys.stdout.flush()
    print(json.dumps({'all runs, confirm (n, p99) by entry KB': binned(every)}))


if __name__ == '__main__':
    main()
