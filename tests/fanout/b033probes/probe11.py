import bisect, collections, json, lzma, sys
from datetime import datetime
def ms(s): return int(datetime.fromisoformat(s.replace('Z', '+00:00')).timestamp() * 1000)
def pc(xs, q):
    xs = sorted(xs); return round(xs[min(len(xs) - 1, int(q * len(xs)))]) if xs else None
path, a = sys.argv[1], ms(sys.argv[2]); lo, hi = a + 30000, a + 270000
rows, seen = [], set()
for line in lzma.open(path, 'rt'):
    d = json.loads(line)
    if 'rows' not in d or 'learner' in d or d.get('site') != 0: continue
    for r in d['rows']:
        k = tuple(r)
        if k not in seen and a <= r[0] < a + 300000: seen.add(k); rows.append(r)
R = sorted(rows, key=lambda r: r[0])
app, first, acks = {}, {}, collections.defaultdict(list)
for pos, r in enumerate(R):
    if r[1] == 4: app.setdefault(int(float(r[4])), r[0])
    if r[1] == 12 and r[2] < 192: first.setdefault(int(float(r[4])), (r[0], int(float(r[3]))))
    if r[1] == 14 and r[2] < 192 and int(float(r[4])) == 0: acks[int(float(r[3]))].append(r[0])
E = []
for i in sorted(first):
    t, rnd = first[i]
    ack = min((x for x in acks.get(rnd, []) if x >= t), default=None)
    if ack is not None: E.append((t, max(app.get(i, ack), ack)))
E.sort()
S = [e for e in E if lo <= e[0] < hi]
r = [e[1] - e[0] for e in S]
print(f'steady rounds (append -> applied) n {len(r)} p50 {pc(r, .5)} p90 {pc(r, .9)} p95 {pc(r, .95)} p99 {pc(r, .99)}')
# two-round model from the append/commit times alone: a Human admitted at t commits with the first entry appended
# at least D ms after t (it must be in a batch frozen before that entry's append), D = freeze lead
ts = [e[0] for e in E]
for D in (0, 150, 300):
    w = []
    for t in range(lo, hi, 7):
        j = bisect.bisect_left(ts, t + D)
        if j < len(E): w.append(E[j][1] - t)
    print(f'  two-round model, batch freezes {D} ms before its append: Human p50 {pc(w, .5)} p90 {pc(w, .9)} p99 {pc(w, .99)}')
print(f'  doc formula with these rounds: 2 x p99 + p50 = {2 * pc(r, .99) + pc(r, .5)}; p99 + p90 = {pc(r, .99) + pc(r, .9)}; 2 x p95 + 0 = {2 * pc(r, .95)}')
