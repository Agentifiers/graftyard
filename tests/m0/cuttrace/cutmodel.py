import json, lzma, sys, bisect, collections
# 1) The leader's member-clock lead over its raw Date.now() during load, rebuilt from every stamp it received
#    (CallArrived 20, AnswerArrived 21: `index` is the sender's stamp; edge stamps untraced, so this is a lower bound).
# 2) A model of which entry each /commit rides under read-ahead's freeze rule (`cut_early`): while entry k is in flight
#    the next batch freezes at append_k + W, so a command arriving at e rides the entry after the first freeze past e.
#    Commit = apply of that entry - e. Compared with the leader's own commitMs (raw clock both ends).
path, t0 = sys.argv[1], int(sys.argv[2]); t1 = t0 + 600000
seen = set(); arr = []; app = {}; aply = []; stamps = []
for l in lzma.open(path, 'rt'):
    d = json.loads(l)
    if 'rows' not in d or 'learner' in d or d.get('site') != 0: continue
    for r in d['rows']:
        t = tuple(r)
        if t in seen: continue
        seen.add(t)
        if not (t0 - 60000 <= r[0] < t1 + 5000): continue
        if r[1] == 25 and r[4] % 64 == 3 and t0 <= r[0] < t1: arr.append(r[0])
        if r[1] == 15: app.setdefault(r[4], r[0])
        if r[1] == 4: aply.append((r[0], r[4]))
        if r[1] in (20, 21): stamps.append((r[0], r[4]))
def pc(xs, q): xs = sorted(xs); return xs[min(len(xs) - 1, int(len(xs) * q))]
stamps.sort(); floor = 0; lead = []
for at, st in stamps:
    if t0 <= at < t1: lead.append(max(0, floor - at))
    floor = max(floor, st)
print(f"member clock lead over Date.now() at the leader (n {len(lead)}): p50 {pc(lead,.5)} p90 {pc(lead,.9)} p99 {pc(lead,.99)} max {max(lead)}")
aply.sort(); applied_at = {}; prev = 0
for at, idx in aply:
    for i in range(prev + 1, idx + 1): applied_at.setdefault(i, at)
    prev = max(prev, idx)
ents = sorted((at, i) for i, at in app.items() if i in applied_at)
arr.sort()
for W in (0, 40, 80, 120, 160):
    fz = [a + W for a, _ in ents]
    com, split = [], collections.defaultdict(list)
    for e in arr:
        j = bisect.bisect_right(fz, e)          # first freeze after e belongs to entry j's flight
        if j + 1 >= len(ents): continue
        a1, i1 = ents[j + 1]                    # the batch frozen during entry j's flight is entry j+1
        com.append(applied_at[i1] - e)
        split['to freeze'].append(fz[j] - e); split['frozen to append'].append(a1 - fz[j])
        split['round'].append(applied_at[i1] - a1)
    tot = sum(com)
    shares = ", ".join(f"{k} {100*sum(v)/tot:.0f}%" for k, v in split.items())
    print(f"W {W:3} ms: modelled commit p50 {pc(com,.5)} p90 {pc(com,.9)} p99 {pc(com,.99)}; mean shares: {shares}")
