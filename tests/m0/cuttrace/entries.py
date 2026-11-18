import json, lzma, sys, collections
# Per Events entry on the leader (site 0): append (Stepped 15, `index` = last entry queued) and apply (Applied 4,
# `index` = applied index), on the same member clock. Per entry: cycle from the previous append, the commit round
# (append -> apply), and the idle gap from the previous apply to this append (no entry in flight, nothing cut).
# Also each AppendQueued (12) to peers 1, 2: bytes per entry; and Confirm (10) ms on the leader.
path, t0 = sys.argv[1], int(sys.argv[2]); t1 = t0 + 600000
seen = set(); app = {}; aply = []; conf = []; byts = collections.defaultdict(int)
for l in lzma.open(path, 'rt'):
    d = json.loads(l)
    if 'rows' not in d or 'learner' in d or d.get('site') != 0: continue
    for r in d['rows']:
        t = tuple(r)
        if t in seen: continue
        seen.add(t)
        if not (t0 - 5000 <= r[0] < t1 + 5000): continue
        if r[1] == 15: app.setdefault(r[4], r[0])
        if r[1] == 4: aply.append((r[0], r[4]))
        if r[1] == 12 and r[2] == 1: byts[r[4]] = max(byts[r[4]], r[5])
aply.sort()
applied_at = {}
prev = 0
for at, idx in aply:
    for i in range(prev + 1, idx + 1): applied_at.setdefault(i, at)
    prev = max(prev, idx)
idx = sorted(i for i in app if i in applied_at and t0 <= app[i] < t1)
rows = []
for a, b in zip(idx, idx[1:]):
    if b != a + 1: continue
    rows.append({'i': b, 'round': applied_at[b] - app[b], 'gap': app[b] - applied_at[a], 'cycle': app[b] - app[a],
                 'bytes': byts.get(b, 0)})
def pc(xs, q): xs = sorted(xs); return xs[min(len(xs) - 1, int(len(xs) * q))]
n = len(rows)
for k in ('cycle', 'round', 'gap', 'bytes'):
    xs = [r[k] for r in rows]
    print(f"{k}: n {n} p10 {pc(xs,.1)} p50 {pc(xs,.5)} p90 {pc(xs,.9)} p99 {pc(xs,.99)} sum {sum(xs)/1000:.1f}" + ("s" if k != 'bytes' else "KB*1e-3"))
tot = sum(r['cycle'] for r in rows)
print(f"time in a round {100*sum(r['round'] for r in rows)/tot:.1f}%, idle between apply and next append {100*sum(r['gap'] for r in rows)/tot:.1f}%")
print(f"bytes per entry over 90 KB: {sum(r['bytes']>90000 for r in rows)} of {n}")
json.dump(rows, open(sys.argv[3] if len(sys.argv) > 3 else 'entries.json', 'w'))
