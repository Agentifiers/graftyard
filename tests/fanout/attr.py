#!/usr/bin/env python3
"""attr.py SHARDEV.jsonl[.xz] MISSLINES.jsonl T0_MS [SECOND_S HOSTILE_S] -> B0-34's validate and clone tails, split by
cause on each shard object, from the run's `GitShard` events (Workers Logs, every site, `$workers.entrypoint` GitShard)
and its `shard miss` lines (`misses.py`'s RAW).

T0_MS is the row's start (a1's "900 s from" time, epoch ms); the phases are j9 to the kill at 300 s, kill to SECOND_S
(420), second to HOSTILE_S (600), hostile to the end. Prints, per object of the network named in the events:
  - /validate walls by phase and status;
  - each catch-up whose stored-pack ingests waited (`stored.total_ms - index_ms`, the time before `indexIngest` ran,
    which is `/ingest`'s `adopt` under the lock alone) or held the lock alone (`lock_ms`) over 10 s, with the stock
    upload-packs open on the container meanwhile and the walls of the validates that started inside it;
  - 200 validates over 10 s in the second and hostile phases that overlapped such a catch-up;
  - the 503 miss lines split by what ran out: the 15 s catch-up wait right after the 409 (`only.ms + 15000`), the 60 s
    of a last site, or a wait for the running catch-up (`compose_then_catch_up`'s unbounded `idle`) then the 15 s.
tests/am1/results/b034-attr.md reads it for m1b34.
"""
import collections, json, lzma, sys

ev_path, lines_path, t0 = sys.argv[1], sys.argv[2], int(sys.argv[3])
second, hostile = (int(sys.argv[4]), int(sys.argv[5])) if len(sys.argv) > 5 else (420, 600)


def phase(t):
    s = (t - t0) / 1000
    return 'pre' if s < 0 else 'j9' if s < 300 else 'kill' if s < second else 'second' if s < hostile else 'hostile'


def pct(a, p):
    a = sorted(a)
    return a[min(len(a) - 1, int(p * len(a)))] if a else 0


opener = lzma.open if ev_path.endswith('.xz') else open
reqs, cus = [], collections.defaultdict(list)
for line in opener(ev_path, 'rt'):
    e = json.loads(line)
    w = e['$workers']
    obj = w.get('durableObjectId', '')[:8]
    ev = w.get('event') if isinstance(w.get('event'), dict) else {}
    s = ev.get('search') if isinstance(ev.get('search'), dict) else {}
    if ev.get('path'):
        reqs.append(dict(obj=obj, site=e.get('_site'), path=ev['path'], repo=s.get('repo'), t=e['timestamp'],
                         wall=w.get('wallTimeMs'), st=(ev.get('response') or {}).get('status')))
    m = (e.get('source') or {}).get('message')
    if isinstance(m, str) and m.startswith('shard catch-up stored '):
        j = json.loads(m.split(': ', 1)[1])
        st = j['stored']
        spools = [x[0] for x in st.get('spools', [])]
        start = min(spools) if spools else e['timestamp'] - dict(j['ms']).get('ingest', 0)
        cus[obj].append(dict(start=start, end=e['timestamp'], waited=st.get('total_ms', 0) - st.get('index_ms', 0),
                             lock=st.get('lock_ms', 0), index=st.get('index_ms', 0), packs=st.get('packs', 0)))

misses = [json.loads(l) for l in open(lines_path)]
objs = collections.Counter(r['obj'] for r in reqs if r['path'] == '/validate')
site_of = {r['obj']: r['site'] for r in reqs}
for obj, _ in objs.most_common(2):
    site = site_of[obj]
    print(f'== object {obj} (site {site})')
    for ph in ['j9', 'kill', 'second', 'hostile']:
        v = [r for r in reqs if r['obj'] == obj and r['path'] == '/validate' and r['wall'] is not None and phase(r['t']) == ph]
        c = collections.Counter(r['st'] for r in v)
        print(f'  validate {ph}: n {len(v)}, p50 {pct([r["wall"] for r in v], .5)}, p99 {pct([r["wall"] for r in v], .99)} ms, {dict(c)}')
    stock = [(l['arrived'] + l['caught_up_ms'], l['arrived'] + l['headers_ms']) for l in misses
             if l['site'] == site and l['answer'].startswith('stock') and l['caught_up_ms'] is not None and l['headers_ms']]
    stalls = [c for c in cus[obj] if c['waited'] > 10_000 or c['lock'] > 10_000]
    for c in stalls:
        v = [r['wall'] for r in reqs if r['obj'] == obj and r['path'] == '/validate' and r['wall'] and c['start'] <= r['t'] < c['end']]
        ov = sum(1 for a, b in stock if a < c['end'] and b > c['start'])
        print(f'  catch-up {c["start"]}..{c["end"]}: waited before indexIngest {c["waited"]} ms, lock alone {c["lock"]} ms, '
              f'index {c["index"]} ms, {c["packs"]} packs; stock readers open {ov}; validates started {len(v)}, '
              f'p50 {pct(v, .5)}, p99 {pct(v, .99)} ms')
    big = [r for r in reqs if r['obj'] == obj and r['path'] == '/validate' and r['st'] == 200 and (r['wall'] or 0) > 10_000
           and phase(r['t']) in ('second', 'hostile')]
    hit = [r for r in big if any(c['start'] <= r['t'] + r['wall'] and r['t'] <= c['end'] for c in stalls)]
    print(f'  200 validates over 10 s, second and hostile: {len(big)}, overlapping a stalled catch-up {len(hit)}')
    s503 = [l for l in misses if l['site'] == site and l['answer'] == 'status 503']
    split = collections.Counter()
    for l in s503:
        extra = l['headers_ms'] - ((l['only'] or {}).get('ms') or 0)
        split['15 s wait right after the 409' if 14_800 <= extra <= 15_300 else
              '60 s wait right after the 409' if 59_800 <= extra <= 60_300 else
              'no 409 first (a container restart)' if not l['only'] else
              'waited for the running catch-up, then 15 s'] += 1
    print(f'  503 miss lines: {len(s503)} {dict(split)}')
