"""Counterfactual for b034-tail: mt15's j9 hits (the colo arm) under the levers in tests/am1/results/b034-tail.md.
  python3 tests/fanout/tailcf.py [samples.jsonl.gz]   (default: mt15's run mwr1wmg9)

A hit is modeled from its own stages as client + front + max(gate, chain_open) + rest: `gate` and `chain_open` run
at once (`hit-gate`), `rest` is what is left of the edge's wall, and `client` (the client's wall less the edge's) is
kept as measured. The model reproduces every hit's measured total. A cold hit is one whose front is over 600 ms
(the row's first second: no `rcache` entry, so `resolve` reads login, placement and name in turn).
Levers:
- `carry`: a new head version's header set carries the previous set's entries whose etag a `list` of the bucket still
  shows, so only new or changed packs are checked: a hit that checked more than one pack has `chain_open` at most
  CO_NEW (550 or 350 ms; mt15's opens that checked one pack: p99 449 ms).
- `head`: the head names each file's etag, count and length per bucket, so no named pack is checked: every hit that
  checked a pack has `chain_open` at most the head get (mt15's opens that checked none: p99 194 ms; 200).
- `warm`: a cold hit's gate is a data-center match (10 ms) instead of a member read, as if an entry under 9 s old
  were there; the front stays cold.
- `links`: a cold hit's front is one read (a third of it), as if the login's account and the placement outlived
  `READ_CACHE`; the gate still starts after the front.
- `beside`: also the name: a cold hit's gate reads start with the token read, so front + gate becomes their max.
p99 is clonelines' rule: sorted ascending, index int(0.99 n). `removed` drops the row's first 1.1 s, as clonelines'
outage cut does on mt15."""
import collections, gzip, json, sys

path = sys.argv[1] if len(sys.argv) > 1 else 'tests/am1/raw/a1-samples-b0-34-mwr1wmg9.jsonl.gz'
g = collections.defaultdict(dict)
for line in gzip.open(path, 'rt'):
    t, k, ms, v = json.loads(line)
    base, _, ph = k.partition('@')
    if ph == 'j9' and (base == 'read:cloneTtfb' or base.startswith('read:cloneTtfb:hit')):
        g[t + ms][base[len('read:cloneTtfb'):] or 'all'] = (ms, t)
hits = [d for d in g.values() if ':hit' in d and ':hit:edge' in d]
t0 = min(d[':hit'][1] for d in hits)


def model(d, carry=None, warm=False, links=False, beside=False, div=3, head=None):
    s = lambda k: d.get(':hit:' + k, (0, 0))[0]
    front, gate, co, edge = s('front'), s('gate'), s('chain_open'), s('edge')
    rest = max(0, edge - front - max(gate, co))
    client = d[':hit'][0] - edge
    cold = front > 600
    if carry and s('chain_chk') > 1:
        co = min(co, carry)
    if head and s('chain_chk') > 0:
        co = min(co, head)
    if cold and warm:
        gate = min(gate, 10)
    if cold and (links or beside):
        front /= div
    core = max(front, gate, co) if cold and beside else front + max(gate, co)
    return client + core + rest


def p99(v):
    v = sorted(v)
    return round(v[int(0.99 * len(v))])


def show(name, **kw):
    v = [model(d, **kw) for d in hits]
    r = [model(d, **kw) for d in hits if d[':hit'][1] - t0 >= 1100]
    print(f'{name:34} measured p99 {p99(v):5}  over 1.1 s {sum(x > 1100 for x in v)}  | removed p99 {p99(r):5}'
          f'  top three {[round(x) for x in sorted(v)[-3:]]}')


print('j9 hits', len(hits), 'measured p99', p99([d[':hit'][0] for d in hits]))
show('model, no lever')
show('carry 550', carry=550)
show('carry 350', carry=350)
show('warm (harness only)', warm=True)
show('warm + carry 550', warm=True, carry=550)
show('links', links=True)
show('links + carry 550', links=True, carry=550)
show('beside', beside=True)
show('beside + carry 550', beside=True, carry=550)
show('beside + carry 350', beside=True, carry=350)
show('links + warm (a real deploy)', links=True, warm=True)
show('links + warm + carry 550', links=True, warm=True, carry=550)
show('beside + carry 550, front/2', beside=True, carry=550, div=2)
show('links + warm + carry 550, front/2', links=True, warm=True, carry=550, div=2)
show('head', head=200)
show('beside + head', beside=True, head=200)
show('beside + head, front/2', beside=True, head=200, div=2)
show('links + head', links=True, head=200)
show('links + warm + head', links=True, warm=True, head=200)
