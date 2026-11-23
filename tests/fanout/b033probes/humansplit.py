"""Scratch ($0, second opinion on B0-33's Human commit p99): a per-Human split of commit time from a tsave trace.

    python3 humansplit.py TSAVE LOAD_START_ISO [FROM_S TO_S]

The leader's cut rows (Trace::Cut 29) carry the Human commands each cut took. Humans are taken first in every cut and
bounded only by bytes (crates/raft/src/member/leader.rs, `cut`), so a Human admitted between two cuts goes in the later
one. ESTIMATE: each cut's Humans are spread evenly over the gap since the cut before it; the push log has the real
arrival times and is not in the repository. Per Human, on the leader's clock:
  freeze wait   admission -> the cut that took it
  held          that cut -> its entry's first AppendQueued (the batch sits frozen behind the entry in flight)
  own round     AppendQueued -> first ok voter AppendEnded
  apply         that ack -> Applied
What-ifs replay the same cuts: `late join` lets a Human admitted at least DEPS ms before the previous batch's append ride
that batch; `no held` drops the held part (the bound on any freeze-rule change); `x k` scales every part by k.

LM-1's addition (2026-11-21): with PUSHES, an a1 push log (`tests/am1/raw/a1-pushes-b0-33-<run>.jsonl`), the Humans are
the real ones: each ok Human push's admission is its `end` less its `commit` stage (`server-timing`), both on
Cloudflare's clock, the leader's trace clock to within the orb offset's error. It rides the first cut at or after that
instant, as the rule above says. A Human whose instant falls past the last cut is dropped.

    python3 humansplit.py TSAVE LOAD_START_ISO [FROM_S TO_S] [--pushes PUSHES] [--kind merge]

Row B2's addition (2026-11-23): `--kind merge` splits a1's `merge` pushes instead (the admin PAT's pushes to `a1m-`
refs before each PR merge, Human-class at the edge, `crate::admission`), and with PUSHES each push whose edge `commit`
stage passed 2 s is printed with its leader-side split; `rest` is the edge stage less the leader's part (the call to
the leader and the outcome's return, plus the clock offset's error).
"""
import bisect, collections, json, lzma, re, sys
from datetime import datetime


def pc(xs, q):
    xs = sorted(xs)
    return round(xs[min(len(xs) - 1, int(q * len(xs)))]) if xs else None


pushes, kind = None, 'human'
if '--kind' in sys.argv:
    i = sys.argv.index('--kind'); kind = sys.argv[i + 1]; del sys.argv[i:i + 2]
if '--pushes' in sys.argv:
    i = sys.argv.index('--pushes'); pushes = sys.argv[i + 1]; del sys.argv[i:i + 2]
path = sys.argv[1]
a = int(datetime.fromisoformat(sys.argv[2].replace('Z', '+00:00')).timestamp() * 1000)
lo, hi = a + 1000 * int(sys.argv[3] if len(sys.argv) > 3 else 30), a + 1000 * int(sys.argv[4] if len(sys.argv) > 4 else 270)
rows, seen = [], set()
for line in (lzma.open if path.endswith('.xz') else open)(path, 'rt'):
    d = json.loads(line)
    if 'rows' not in d or 'learner' in d or d.get('site') != 0:
        continue
    for r in d['rows']:
        k = tuple(r)
        if k not in seen and a <= r[0] < a + 300000:
            seen.add(k)
            rows.append(r)
R = sorted(rows, key=lambda r: r[0])
app, first, acks = {}, {}, collections.defaultdict(list)
for pos, r in enumerate(R):
    if r[1] == 4:
        app.setdefault(int(float(r[4])), r[0])
    if r[1] == 12 and r[2] < 192:
        first.setdefault(int(float(r[4])), (r[0], int(float(r[3])), pos))
    if r[1] == 14 and r[2] < 192 and int(float(r[4])) == 0:
        acks[int(float(r[3]))].append(r[0])
idxs = sorted(first, key=lambda i: first[i][2])
apos = [first[i][2] for i in idxs]
cut = {}
for pos, r in enumerate(R):
    if r[1] != 29:
        continue
    k = bisect.bisect_left(apos, pos) - 1
    if k >= 0:
        bt = int(float(r[3]))
        cut.setdefault(idxs[k] + (r[2] // 16) % 4 + 1, (r[0], [(bt >> (12 * i)) & 0xfff for i in range(4)], r[2] >= 64))
E = []
for i in idxs:
    t, rnd, _ = first[i]
    ack = min((x for x in acks.get(rnd, []) if x >= t), default=None)
    if ack is not None and i in cut:
        E.append({'cut': cut[i][0], 'q': t, 'ack': ack, 'ap': max(app.get(i, ack), ack), 'h': cut[i][1][0],
                  'n': sum(cut[i][1]), 'agents': cut[i][1][3], 'early': cut[i][2]})
E.sort(key=lambda e: e['cut'])
rounds = [e['ack'] - e['q'] for e in E if lo <= e['q'] < hi]
print(f'entry rounds {len(rounds)}: p50 {pc(rounds, .5)} p90 {pc(rounds, .9)} p95 {pc(rounds, .95)} p99 {pc(rounds, .99)}')
H = []
if pushes:
    real, stage = [], {}
    for line in open(pushes):
        d = json.loads(line)
        m = re.search(r'(?:^|, )commit;dur=(\d+)', d.get('timing') or '')
        if d['kind'] == kind and d['result'] == 'ok' and m:
            real.append(d['end'] - int(m.group(1)))
            stage[real[-1]] = int(m.group(1))
    cuts = [e['cut'] for e in E]
    for t in sorted(real):
        j = bisect.bisect_left(cuts, t)
        if j < 1 or j >= len(E) or not lo <= E[j]['cut'] < hi:
            continue
        p, e = E[j - 1], E[j]
        H.append({'t': t, 'p': p, 'e': e, 'now': e['ap'] - t, 'wait': e['cut'] - t, 'held': e['q'] - e['cut'],
                  'round': e['ack'] - e['q'], 'apply': e['ap'] - e['ack']})
    print(f'real {kind} arrivals {len(real)} in the push log, {len(H)} in the window')
    for h in sorted(H, key=lambda h: h['t']):
        if stage[h['t']] > 2000:
            print(f"  over 2 s at +{(h['t'] - a) / 1000:.0f} s: edge commit {stage[h['t']]} = freeze wait {h['wait']:.0f} + "
                  f"held {h['held']:.0f} + own round {h['round']:.0f} + apply {h['apply']:.0f} (leader {h['now']:.0f}); "
                  f"rest {stage[h['t']] - h['now']:.0f}")
for j in range(1, len(E)) if not pushes else ():
    p, e = E[j - 1], E[j]
    if not lo <= e['cut'] < hi:
        continue
    span = e['cut'] - p['cut']
    for k in range(e['h']):
        t = e['cut'] - span * (k + .5) / e['h']
        H.append({'t': t, 'p': p, 'e': e, 'now': e['ap'] - t, 'wait': e['cut'] - t, 'held': e['q'] - e['cut'],
                  'round': e['ack'] - e['q'], 'apply': e['ap'] - e['ack']})
H.sort(key=lambda h: h['now'])
n = len(H)
tot = [h['now'] for h in H]
print(f'Humans {n} (estimate): commit p50 {pc(tot, .5)} p90 {pc(tot, .9)} p99 {pc(tot, .99)}; within 1 s {100 * sum(x <= 1000 for x in tot) / n:.0f}%; '
      f'riding an entry with 64 agent commands {100 * sum(h["e"]["agents"] >= 64 for h in H) / n:.0f}%')
for l, u, name in ((.4, .6, 'p40-p60'), (.85, .95, 'p85-p95'), (.97, 1, 'top 3%')):
    xs = H[int(l * n):int(u * n)]
    f = lambda k: round(sum(x[k] for x in xs) / len(xs))
    print(f'  {name:8} n {len(xs):3}: {f("now")} = freeze wait {f("wait")} + held {f("held")} + own round {f("round")} + apply {f("apply")}')
fr = [(e['q'] - e['cut']) for e in E if lo <= e['cut'] < hi and e['early']]
print(f'batches cut while an entry was in flight {len(fr)}: then frozen p50 {pc(fr, .5)} p90 {pc(fr, .9)} p99 {pc(fr, .99)} ms; wall time with a frozen '
      f'batch waiting {100 * sum(e["q"] - e["cut"] for e in E if lo <= e["cut"] < hi) / (hi - lo):.0f}%')


def show(name, xs):
    print(f'  what-if {name:46}: p50 {pc(xs, .5):5} p90 {pc(xs, .9):5} p99 {pc(xs, .99):5} within 1 s {100 * sum(x <= 1000 for x in xs) / len(xs):3.0f}%')


show('no held (bound on any freeze-rule change)', [h['now'] - h['held'] for h in H])
lj = {}
for d in (0, 75, 150):
    lj[d] = [(h['p']['ap'] - h['t']) if h['t'] + d <= h['p']['q'] else h['now'] for h in H]
    show(f'late join, joiner deps read {d} ms ({100 * sum(x != h["now"] for x, h in zip(lj[d], H)) / n:.0f}% join)', lj[d])
for k in (.75, .5, .4, .33):
    show(f'every part x {k}', [h['now'] * k for h in H])
    show(f'every part x {k}, and late join (75 ms)', [x * k for x in lj[75]])
