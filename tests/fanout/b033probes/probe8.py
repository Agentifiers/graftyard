import bisect, collections, json, lzma, sys
from datetime import datetime
def ms(s): return int(datetime.fromisoformat(s.replace('Z', '+00:00')).timestamp() * 1000)
def pc(xs, q):
    xs = sorted(xs); return xs[min(len(xs) - 1, int(q * len(xs)))] if xs else None
path, s, a = sys.argv[1], 0, ms(sys.argv[2]); lo, hi = a + 30000, a + 270000
rows, seen = [], set()
for line in lzma.open(path, 'rt'):
    d = json.loads(line)
    if 'rows' not in d or 'learner' in d or d.get('site') != s: continue
    for r in d['rows']:
        k = tuple(r)
        if k not in seen and a <= r[0] < a + 300000: seen.add(k); rows.append(r)
R = sorted(rows, key=lambda r: r[0])
first, acks = {}, collections.defaultdict(list)
for pos, r in enumerate(R):
    if r[1] == 12 and r[2] < 192: first.setdefault(int(float(r[4])), (r[0], int(float(r[3])), pos))
    if r[1] == 14 and r[2] < 192 and int(float(r[4])) == 0: acks[int(float(r[3]))].append(r[0])
idxs = sorted(first, key=lambda i: first[i][2]); apos = [first[i][2] for i in idxs]
cut = {}
for pos, r in enumerate(R):
    if r[1] != 29: continue
    k = bisect.bisect_left(apos, pos) - 1
    if k >= 0:
        bt, lf = int(float(r[3])), int(float(r[4]))
        cut.setdefault(idxs[k] + (r[2] // 16) % 4 + 1, ([(bt >> (12 * i)) & 0xfff for i in range(4)], [(lf >> (13 * i)) & 0x1fff for i in range(4)], r[2] % 16))
ent = sorted(r[0] for r in R if r[1] == 25); 
opn = sorted((r[0], int(float(r[5]))) for r in R if r[1] == 25); ot = [x[0] for x in opn]
E = []
for i in idxs:
    t, rnd, _ = first[i]
    ack = min((x for x in acks.get(rnd, []) if x >= t), default=None)
    if ack is None or i not in cut or not lo <= t < hi: continue
    j = bisect.bisect_right(ot, t) - 1
    E.append({'round': ack - t, 'agent': cut[i][0][3], 'cmds': sum(cut[i][0]), 'left': cut[i][1][3], 'end': cut[i][2],
              'prev': None, 'lopen': opn[j][1] if j >= 0 else 0, 'in1s': bisect.bisect_right(ent, t) - bisect.bisect_left(ent, t - 1000)})
for j in range(1, len(E)): E[j]['prev'] = E[j - 1]['cmds']
cap = [e for e in E if e['end'] == 4]
print(f'capped cuts (64 agent commands) n {len(cap)}: round by agent commands LEFT queued after the cut (instant load, same entry size):')
for lo_, hi_ in ((0, 10), (11, 40), (41, 100), (101, 9999)):
    v = [e['round'] for e in cap if lo_ <= e['left'] <= hi_]
    print(f'   left {lo_}-{hi_}: n {len(v)} round p50 {pc(v, .5)} p90 {pc(v, .9)}')
unc = [e for e in E if e['end'] != 4]
print(f'uncapped cuts n {len(unc)} (queue emptied): round by commands in the entry:')
for lo_, hi_ in ((0, 16), (17, 32), (33, 48), (49, 200)):
    v = [e['round'] for e in unc if lo_ <= e['cmds'] <= hi_]
    print(f'   cmds {lo_}-{hi_}: n {len(v)} round p50 {pc(v, .5)} p90 {pc(v, .9)}')
print('round by commands in the PREVIOUS entry (whose apply and resolutions run as this round starts), this entry capped at 64:')
for lo_, hi_ in ((0, 32), (33, 63), (64, 200)):
    v = [e['round'] for e in cap if e['prev'] is not None and lo_ <= e['prev'] <= hi_]
    print(f'   prev {lo_}-{hi_}: n {len(v)} round p50 {pc(v, .5)} p90 {pc(v, .9)}')
print('round by leader requests open at its append (capped cuts only):')
qs = sorted(e['lopen'] for e in cap)
for q0, q1 in ((0, .25), (.25, .5), (.5, .75), (.75, 1)):
    a_, b_ = qs[int(q0 * len(qs))], qs[min(len(qs) - 1, int(q1 * len(qs)))]
    v = [e['round'] for e in cap if a_ <= e['lopen'] <= b_]
    print(f'   open {a_}-{b_}: n {len(v)} round p50 {pc(v, .5)} p90 {pc(v, .9)}')
sent = [(r[0], int(float(r[4])), int(float(r[5]))) for r in R if r[1] == 23 and r[2] < 192 and lo <= r[0] < hi]
print(f'leader /raft fetches awaiting headers at CallSent: p50 {pc([x[1] for x in sent], .5)} p90 {pc([x[1] for x in sent], .9)} max {max(x[1] for x in sent)}; leader loop lag at send p99 {pc([x[2] for x in sent], .99)}')
