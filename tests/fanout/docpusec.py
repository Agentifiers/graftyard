#!/usr/bin/env python3
"""docpusec.py OBJECT_ID FROM_ISO TO_ISO -> one Durable Object's CPU-seconds per second, estimated.

`docpumin.py` gives CPU per minute (durableObjectsPeriodicGroups), which averages a 5 to 10 s burst over 60 s. This
reads durableObjectsInvocationsAdaptiveGroups per second: request count and CPU-time quantiles (P25 to P99.9). The
mean CPU per request is bounded below and above from those quantiles (each band at its lower or upper edge), times the
requests, then scaled so the minutes in the window match the periodic table's `cpuTime`. The result is an estimate
(the band between the two bounds is printed), not a measured per-second figure. Read-only; account and token from the
environment, the token only as a header.
"""
import collections, json, os, sys, urllib.request

ACCT, TOK = os.environ['CLOUDFLARE_ACCOUNT_ID'], os.environ['CLOUDFLARE_API_TOKEN']
Q = ('cpuTimeP25', 'cpuTimeP50', 'cpuTimeP75', 'cpuTimeP90', 'cpuTimeP95', 'cpuTimeP99', 'cpuTimeP999')
W = (.25, .25, .25, .15, .05, .04, .009)


def gql(query, var):
    r = urllib.request.Request('https://api.cloudflare.com/client/v4/graphql', method='POST',
                               data=json.dumps({'query': query, 'variables': var}).encode(),
                               headers={'authorization': f'Bearer {TOK}', 'content-type': 'application/json'})
    v = json.load(urllib.request.urlopen(r, timeout=90))
    if v.get('errors'):
        sys.exit(json.dumps(v['errors']))
    return v['data']['viewer']['accounts'][0]['p']


def main(obj, frm, to):
    # Both tables are read over whole minutes, so each minute's scale uses all of its seconds; only the window prints.
    whole = lambda t, up: t[:16] + ':00Z' if not up or t[17:19] == '00' else t[:14] + f'{int(t[14:16]) + 1:02d}:00Z'
    var = {'a': ACCT, 's': whole(frm, False), 'e': whole(to, True), 'o': obj}
    sec = gql("""query($a:String!,$s:Time!,$e:Time!,$o:String!){viewer{accounts(filter:{accountTag:$a}){
      p: durableObjectsInvocationsAdaptiveGroups(limit: 10000, filter:{datetime_geq:$s, datetime_lt:$e, objectId:$o}) {
       sum { requests } quantiles { %s } dimensions { datetime } } }}}""" % ' '.join(Q), var)
    minute = gql("""query($a:String!,$s:Time!,$e:Time!,$o:String!){viewer{accounts(filter:{accountTag:$a}){
      p: durableObjectsPeriodicGroups(limit: 10000, filter:{datetime_geq:$s, datetime_lt:$e, objectId:$o}) {
       sum { cpuTime } dimensions { datetimeMinute } } }}}""", var)
    per_min = collections.Counter()
    for g in minute:
        per_min[g['dimensions']['datetimeMinute'][:16]] += g['sum']['cpuTime'] / 1e6
    rows = {}
    for g in sec:
        p = [g['quantiles'][k] for k in Q]
        lo = p[0] / 2 * W[0] + sum((p[i - 1] + p[i]) / 2 * W[i] for i in range(1, 7)) + p[6] * .001
        hi = sum(p[i] * W[i] for i in range(7)) + p[6] * .001
        n = g['sum']['requests']
        rows[g['dimensions']['datetime']] = (n, n * lo / 1e6, n * hi / 1e6)
    est = collections.defaultdict(lambda: [0.0, 0.0])
    for t, (_, lo, hi) in rows.items():
        est[t[:16]][0] += lo
        est[t[:16]][1] += hi
    print('second | requests | CPU-s (scaled to the minute\'s periodic cpuTime)')
    for t in sorted(rows):
        if not frm[:19] <= t[:19] < to[:19]:
            continue
        n, lo, hi = rows[t]
        m = per_min.get(t[:16])
        if m is None or not est[t[:16]][0]:
            print(t[11:19], n, f'{lo:.2f}..{hi:.2f} (unscaled)')
            continue
        a, b = lo * m / est[t[:16]][0], hi * m / est[t[:16]][1]
        print(t[11:19], n, f'{min(a, b):.2f}..{max(a, b):.2f}')


if __name__ == '__main__':
    main(*sys.argv[1:4])
