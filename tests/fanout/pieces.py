"""catchup-pieces verdict lines from a shard-events jsonl (wl.py output, xz or plain).
Per shard object: pieces (catch-up stored lines), grouped into runs (a run ends at a piece with behind false or absent),
each run's piece count, packs, MB, wall from its first piece's start to its last's end; per-piece packs and MB.
Validates and merges of >= 2 s on each site with the run they overlapped; 429s."""
import json, lzma, re, sys, collections

f = sys.argv[1]
T0 = int(sys.argv[2]) if len(sys.argv) > 2 else None
op = lzma.open if f.endswith('.xz') else open
rows = [json.loads(l) for l in op(f, 'rt')]
pieces = collections.defaultdict(list)
for e in rows:
    m = e.get('msg') or ''
    if not m.startswith('shard catch-up stored'):
        continue
    g = lambda k: (re.search('"' + k + r'":(null|true|false|-?\d+)', m) or [None, None])[1]
    ing = re.search(r'\["ingest",(\d+)\]', m)
    dur = int(ing[1]) if ing else 0
    obj = re.search(r'"obj":"[^"]*?(\w{6})"', m)[1]
    pieces[(e['site'], obj)].append(dict(t=e['t'], start=e['t'] - dur, packs=int(g('packs') or 0),
                                          mb=int(g('bytes') or 0) / 1e6, entries=int(g('entries') or 0),
                                          behind=g('behind') == 'true', has=g('behind') is not None))
rel = lambda t: (t - T0) / 1000 if T0 else t
runs = []
for k, ps in pieces.items():
    ps.sort(key=lambda p: p['t'])
    cur = []
    for p in ps:
        cur.append(p)
        if not p['behind']:
            runs.append((k, cur)); cur = []
    if cur:
        runs.append((k, cur))
big = [(k, r) for k, r in runs if sum(p['packs'] for p in r) >= 20]
print('runs with >= 20 packs:', len(big))
for (site, obj), r in big:
    packs = sum(p['packs'] for p in r); mb = sum(p['mb'] for p in r)
    wall = (r[-1]['t'] - r[0]['start']) / 1000; busy = sum(p['t'] - p['start'] for p in r) / 1000
    print(f"  {site} {obj} {rel(r[0]['start']):7.2f}-{rel(r[-1]['t']):7.2f} pieces {len(r)} packs {packs} MB {mb:.1f} "
          f"wall {wall:.1f} s busy {busy:.1f} s; per piece packs {[p['packs'] for p in r]} MB "
          f"{[round(p['mb'], 1) for p in r]}")
allp = [p for _, r in runs for p in r if p['packs'] > 0]
print('pieces with packs', len(allp), 'max packs', max((p['packs'] for p in allp), default=0),
      'max MB', round(max((p['mb'] for p in allp), default=0), 1), 'behind field present', any(p['has'] for p in allp))
slow = [e for e in rows if e.get('path') in ('/validate', '/merge') and (e.get('wall') or 0) >= 2000]
print('validates/merges >= 2 s:', len(slow), collections.Counter((e['site'], e['path'], e.get('status')) for e in slow))
for e in sorted(slow, key=lambda e: e['t'])[:80]:
    s, t = e['t'] - e['wall'], e['t']
    over = [(obj, len(r)) for (site, obj), r in big if site == e['site'] and r[0]['start'] < t and r[-1]['t'] > s]
    print(f"  {e['site']} {e['path']} {e.get('q')} {e.get('status')} {rel(s):7.2f}-{rel(t):7.2f} wall {e['wall']} runs {over}")
print('429s', sum(1 for e in rows if e.get('status') == 429), collections.Counter(e['path'] for e in rows if e.get('status') == 429))
for e in rows:
    m = e.get('msg') or ''
    if m and not m.startswith('shard catch-up stored') and ('rror' in m or 'catch-up' in m):
        print('  msg', e['site'], rel(e['t']), m[:160])
