"""mt15 verdict lines: colo on (read:cloneTtfb) against off (read:cloneOff), from the row's Load34 samples.
Usage: python3 tests/fanout/mt15v.py [samples.jsonl.gz]  (default: the newest a1-samples-b0-34-*.jsonl.gz in tests/am1/raw)"""
import collections, glob, gzip, json, os, sys

def pct(a, p):
    a = sorted(a)
    return a[min(len(a) - 1, int(p / 100 * len(a)))] if a else None

path = sys.argv[1] if len(sys.argv) > 1 else max(glob.glob('tests/am1/raw/a1-samples-b0-34-*.jsonl.gz'), key=os.path.getmtime)
print('samples', path)
MT14 = 'tests/am1/raw/a1-samples-b0-34-mwqhzvfd.jsonl.gz'

def load(p):
    # A sample's t is note time less ms, and a clone's notes are written in one tick, so t + ms groups a clone.
    g = collections.defaultdict(dict)
    other = collections.defaultdict(list)
    for l in gzip.open(p, 'rt'):
        t, k, ms, v = json.loads(l)
        base, _, ph = k.partition('@')
        for arm in ('read:cloneTtfb', 'read:cloneOff'):
            if base == arm or base.startswith(arm + ':'):
                g[(arm, t + ms, ph)][base[len(arm):] or 'all'] = (ms, v)
                break
        else:
            other[(base, ph)].append((t, ms, v))
    return g, other

def clones(g):
    # Group by (arm, phase, end time): a clone's stages are noted with the same t.
    out = collections.defaultdict(list)
    for (arm, t, ph), d in g.items():
        out[(arm, ph)].append((t, d))
    return out

g, other = load(path)
C = clones(g)
phases = ['j9', 'kill', 'second', 'hostile']
print('\n== 1. hits p99 and share under 1 s, colo on vs off')
for ph in phases + ['all']:
    row = []
    for arm in ('read:cloneTtfb', 'read:cloneOff'):
        v = [d[':hit'][0] for (a, p), L in C.items() if a == arm and (ph == 'all' or p == ph) for t, d in L if ':hit' in d]
        row.append(f"{arm.split(':')[1]}: n {len(v)} p50 {pct(v,50)} p99 {pct(v,99)} <1s {100*sum(x<1000 for x in v)/len(v):.1f}%" if v else f'{arm}: none')
    print(ph, ' | '.join(row))

print('\n== 2. colo hit share among hits (on arm): served from colo / isolate / member read')
for ph in phases + ['all']:
    n = colo = iso = read = 0
    for (a, p), L in C.items():
        if a != 'read:cloneTtfb' or not (ph == 'all' or p == ph):
            continue
        for t, d in L:
            if ':hit' not in d:
                continue
            n += 1
            if ':hit:gate_colo' in d: colo += 1
            elif ':hit:gate_iso' in d: iso += 1
            elif d.get(':hit:gate', (0,))[0] > 0: read += 1
            else: iso += 1  # no gate wall and no age stage: an isolate entry under 1 ms old
    if n:
        print(ph, f'hits {n}: colo {colo} ({100*colo/n:.1f}%), isolate {iso} ({100*iso/n:.1f}%), member read {read} ({100*read/n:.1f}%)')
for arm in ('read:cloneOff',):
    v = [d for (a, p), L in C.items() if a == arm for t, d in L if ':hit' in d]
    if v:
        print('off arm: hits', len(v), 'with gate_colo', sum(':hit:gate_colo' in d for d in v), '(must be 0)',
              'member read', sum(d.get(':hit:gate', (0,))[0] > 0 for d in v))

print('\n== 3. oldest served stamp (ms), must be under READ_CACHE 10,000')
for st in ('gate_colo', 'gate_iso'):
    for arm in ('read:cloneTtfb', 'read:cloneOff'):
        v = [d[k][0] for (a, p), L in C.items() if a == arm for t, d in L for k in d if k.endswith(':' + st)]
        print(arm, st, 'n', len(v), 'max', max(v) if v else None, 'p99', pct(v, 99))

print('\n== 4. chain_open p99 and header-set share')
for arm in ('read:cloneTtfb', 'read:cloneOff'):
    for kind in (':hit', ':miss'):
        v = [d[kind + ':chain_open'][0] for (a, p), L in C.items() if a == arm for t, d in L if kind + ':chain_open' in d]
        sets = sum(d[k][0] for (a, p), L in C.items() if a == arm for t, d in L for k in d if k == kind + ':chain_set')
        chk = sum(d[k][0] for (a, p), L in C.items() if a == arm for t, d in L for k in d if k == kind + ':chain_chk')
        share = f'{100*sets/(sets+chk):.1f}%' if sets + chk else 'n/a'
        print(arm, kind, 'chain_open n', len(v), 'p50', pct(v, 50), 'p99', pct(v, 99), '| packs from sets', sets, 'checked', chk, 'set share', share)

def gate_reads_per_min(Cx, arm='read:cloneTtfb'):
    m = collections.Counter(); span = set()
    for (a, p), L in Cx.items():
        if a != arm:
            continue
        for t, d in L:
            span.add(t // 60000)
            if any(d.get(k, (0,))[0] > 0 for k in (':hit:gate', ':miss:gate')) and not any(k.endswith(('gate_colo', 'gate_iso')) for k in d):
                m[t // 60000] += 1
    mins = max(1, len(span))
    return sum(m.values()), mins, sum(m.values()) / mins

print('\n== 5. probe gates that read members (client-visible), per minute, against mt14')
r = gate_reads_per_min(C)
print('mt15 on arm', r[0], 'over', r[1], 'min =', round(r[2], 2), '/min')
r = gate_reads_per_min(C, 'read:cloneOff')
print('mt15 off arm', r[0], 'over', r[1], 'min =', round(r[2], 2), '/min')
if os.path.exists(MT14):
    g14, _ = load(MT14)
    r = gate_reads_per_min(clones(g14))
    print('mt14', r[0], 'over', r[1], 'min =', round(r[2], 2), '/min')

print('\n== 6. kill, second, human push, 500s')
for arm in ('read:cloneTtfb', 'read:cloneOff'):
    for ph in phases:
        v = [d['all'] for (a, p), L in C.items() if a == arm and p == ph for t, d in L if 'all' in d]
        if v:
            ok = sum(s == '200' for _, s in v)
            fast = sum(s == '200' and ms <= 5000 for ms, s in v)
            print(arm, ph, 'clones', len(v), '200 within 5 s', fast, f'({100*fast/len(v):.1f}%)', 'codes', dict(collections.Counter(s for _, s in v)))
for ph in phases:
    v = [ms for t, ms, s in other.get(('push:human', ph), []) if s == 'ok']
    if v:
        print('push:human', ph, 'n', len(v), 'p50', pct(v, 50), 'p99', pct(v, 99))
fives = collections.Counter((b, p) for (b, p), L in other.items() for t, ms, s in L if s == '500')
fives.update((a, p) for (a, t, p), d in g.items() if d.get('all', (0, ''))[1] == '500')
print('500s', dict(fives) or 0)
