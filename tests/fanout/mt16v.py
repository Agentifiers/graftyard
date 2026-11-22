"""mt16 verdict lines for beside and head-etags: resolve_guess, gate_beside, beside_bad, first-second front, chain_open by head records.
Usage: python3 tests/fanout/mt16v.py tests/am1/raw/a1-samples-b0-34-<run>.jsonl.gz"""
import collections, gzip, json, sys
path = sys.argv[1]
g = collections.defaultdict(dict)
for line in gzip.open(path, 'rt'):
    t, k, ms, v = json.loads(line)
    base, _, ph = k.partition('@')
    for arm in ('read:cloneTtfb', 'read:cloneOff'):
        if base == arm or base.startswith(arm + ':'):
            g[(arm, ph, t + ms)][base[len(arm):] or 'all'] = (ms, t, v); break
def st(d, k):
    hk = ':hit:' + k if ':hit' in d else ':miss:' + k if ':miss' in d else ':' + k
    return d.get(hk, d.get(':' + k, (None,)))[0]
def pct(a, p):
    a = sorted(x for x in a if x is not None); return a[min(len(a)-1, int(p/100*len(a)))] if a else None
cl = [(a, ph, e, d) for (a, ph, e), d in g.items() if 'all' in d]
t0 = min(d['all'][1] for a, ph, e, d in cl if ph == 'j9')
print('clones', len(cl), 't0', t0)
rg = collections.Counter((a, st(d, 'resolve_guess')) for a, ph, e, d in cl)
print('resolve_guess by arm:', dict(rg))
print('gate_beside present:', sum(st(d, 'gate_beside') is not None for *_, d in cl), 'beside_bad:', sum((st(d, 'beside_bad') or 0) for *_, d in cl), 'beside_rows:', sum(st(d, 'beside_rows') is not None for *_, d in cl), 'beside_match:', sum(st(d,'beside_match') is not None for *_, d in cl))
# first second: j9 clones starting within 1.1 s of t0 (as mt15's (a))
fs = sorted([(d['all'][1]-t0, a, d) for a, ph, e, d in cl if ph == 'j9' and d['all'][1]-t0 < 1100], key=lambda x: x[0])
print('first-second j9 clones', len(fs))
for dt, a, d in fs:
    print(f"  +{dt:5}ms {a[10:]:8} {'hit' if ':hit' in d else 'miss'} total {d['all'][0]} front {st(d,'front')} gate {st(d,'gate')} rg {st(d,'resolve_guess')} gate_iso {st(d,'gate_iso')} gate_match {st(d,'gate_match')} gate_colo {st(d,'gate_colo')} chain_open {st(d,'chain_open')} beside_rows {st(d,'beside_rows')}")
print('first-second front p50', pct([st(d,'front') for _,_,d in fs], 50))
cold = [d for a, ph, e, d in cl if (st(d,'front') or 0) > 600]
print('cold (front>600) clones', len(cold), 'front p50', pct([st(d,'front') for d in cold], 50), 'gate p50', pct([st(d,'gate') for d in cold],50))
for k in ('all',):
    pass
# gate p50 by resolve_guess
for rgv in (0, 1, 2):
    ds = [d for a, ph, e, d in cl if st(d,'resolve_guess') == rgv]
    print(f'resolve_guess {rgv}: n {len(ds)} gate p50 {pct([st(d,"gate") for d in ds],50)} p99 {pct([st(d,"gate") for d in ds],99)} front p50 {pct([st(d,"front") for d in ds],50)}')
# chain_open on a new head version: chain_head 1 means header set from head
for a in ('read:cloneTtfb','read:cloneOff'):
    ds = [d for aa, ph, e, d in cl if aa == a and st(d,'chain_open') is not None]
    hh = [d for d in ds if (st(d,'chain_head') or 0) > 0]
    chk = [d for d in ds if (st(d,'chain_chk') or 0) > 0]
    print(f"{a}: chain_open n {len(ds)} p99 {pct([st(d,'chain_open') for d in ds],99)} | chain_head>0 n {len(hh)} share {len(hh)/max(1,len(ds)):.1%} p99 {pct([st(d,'chain_open') for d in hh],99)} | chain_chk>0 n {len(chk)} p50 {pct([st(d,'chain_open') for d in chk],50)} p99 {pct([st(d,'chain_open') for d in chk],99)} max {max([st(d,'chain_open') for d in chk] or [None])}")
    print('   chain_head values', collections.Counter(st(d,'chain_head') for d in ds), 'chain_set', collections.Counter(st(d,'chain_set') for d in ds), 'chain_chk', collections.Counter(st(d,'chain_chk') for d in ds))
