"""B0-29's merge commit stage split on the leader's trace, as humansplit.py splits a Human push ($0, 2026-11-23).

    python3 tests/fanout/mergecommit.py TAG LOAD_START_ISO      e.g. b9m3f4 2026-11-23T21:59:44Z

Inputs: a1's summary (`tests/fanout/raw/a1-TAG.jsonl`, `mergeSteps`: each merge's client time and the edge's
`server-timing`), a1's `merge:only` sample rows in the same order, each with its start on Cloudflare's clock
(`tests/fanout/raw/mergestarts-TAG.jsonl`, grepped from the git-ignored `tests/am1/raw/a1-samples-b0-29-RUN.jsonl.gz`),
and the leader's trace (`tests/fanout/raw/tsave-TAG.jsonl.xz`, site 0).

ESTIMATE: a merge's `commit` stage ends at start + client time less half the client leg (client time less `total`), and
began `commit` before that: the edge's send. It rides the first cut at or after that instant. A merge is a Human command, taken first in every cut and bounded
only by bytes (`crates/raft/src/member/leader.rs`, `cut`). On the leader's clock, per merge:
  freeze wait   the edge's send -> the cut that took it (so it holds the call's leg to the leader and its input queue)
  held          that cut -> its entry's first AppendQueued (frozen behind the entry in flight)
  own round     AppendQueued -> first ok voter AppendEnded
  apply         that ack -> Applied. A busy isolate's clock reads its last I/O, so apply's CPU shows here as about 0 and
                lands in the next column
  confirm       the first storage confirm (Trace::Confirm 10) at or after Applied covering the entry: its `ms`, the
                output gate the outcome waits behind
  rest          the edge's commit stage less those five: after the confirm, the outcome's return to the edge, plus the
                orb clock offset's error (`errMs` about 30)
"""
import bisect, collections, json, lzma, re, sys
from datetime import datetime

tag, iso = sys.argv[1:3]
a = int(datetime.fromisoformat(iso.replace('Z', '+00:00')).timestamp() * 1000)
steps = json.loads(open(f'tests/fanout/raw/a1-{tag}.jsonl').read().strip().split('\n')[-1])['mergeSteps']
starts = [json.loads(l)[0] for l in open(f'tests/fanout/raw/mergestarts-{tag}.jsonl')]
assert len(starts) == len(steps), (len(starts), len(steps))

rows, seen = [], set()
for line in lzma.open(f'tests/fanout/raw/tsave-{tag}.jsonl.xz', 'rt'):
    d = json.loads(line)
    if 'rows' not in d or 'learner' in d or d.get('site') != 0:
        continue
    for r in d['rows']:
        k = tuple(r)
        if k not in seen:
            seen.add(k)
            rows.append(r)
R = sorted(rows, key=lambda r: r[0])
app, first, acks, conf = {}, {}, collections.defaultdict(list), []
for pos, r in enumerate(R):
    if r[1] == 10:
        conf.append((r[0], int(float(r[4])), int(float(r[5]))))
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
        cut.setdefault(idxs[k] + (r[2] // 16) % 4 + 1, r[0])
E = []
for i in idxs:
    t, rnd, _ = first[i]
    ack = min((x for x in acks.get(rnd, []) if x >= t), default=None)
    if ack is not None and i in cut:
        ap = max(app.get(i, ack), ack)
        cf = next((c for c in conf[bisect.bisect_left(conf, (ap,)):] if c[1] >= i), None)
        E.append({'cut': cut[i], 'q': t, 'ack': ack, 'ap': ap, 'conf': cf[2] if cf and cf[0] - ap < 5000 else 0})
E.sort(key=lambda e: e['cut'])
cuts = [e['cut'] for e in E]

M = []
for s, st in zip(starts, steps):
    t = {k: int(v) for k, v in re.findall(r'(\w+);dur=(\d+)', st.get('timing') or '')}
    if 'total' not in t or 'commit' not in t:
        continue
    net = st['ms'] - t['total']
    adm = s + st['ms'] - net / 2 - t['commit']
    j = bisect.bisect_left(cuts, adm)
    if j >= len(E):
        continue
    e = E[j]
    lead = {'wait': e['cut'] - adm, 'held': e['q'] - e['cut'], 'round': e['ack'] - e['q'], 'apply': e['ap'] - e['ack'], 'confirm': e['conf']}
    M.append({'client': st['ms'], 'commit': t['commit'], 'at': (adm - a) / 1000, **lead, 'rest': t['commit'] - sum(lead.values())})

K = ('wait', 'held', 'round', 'apply', 'confirm', 'rest')
M.sort(key=lambda m: -m['client'])
tail = M[:max(1, round(len(M) / 10))]
mean = lambda xs, k: round(sum(x[k] for x in xs) / len(xs))
print(f'{tag}: {len(M)} merges joined of {len(steps)}; entries {len(E)}')
for name, xs in (('all', M), ('slowest 10%', tail)):
    c = mean(xs, 'commit')
    print(f'  {name:11} n {len(xs):2}: client {mean(xs, "client")}, commit stage {c} = ' +
          ' + '.join(f'{k} {mean(xs, k)} ({100 * sum(x[k] for x in xs) / max(1, sum(x["commit"] for x in xs)):.0f}%)' for k in K))
for m in tail:
    print(f'    +{m["at"]:5.0f} s client {m["client"]:5}: commit {m["commit"]:5} = ' + ' + '.join(f'{k} {m[k]:.0f}' for k in K))
med = lambda xs: sorted(xs)[len(xs) // 2]
for name, drop in (('no held (bound on any freeze-rule change)', ('held',)), ('no held and no freeze wait', ('held', 'wait')),
                   ('own round x 0.9 (a 10% faster leader turn)', ())):
    xs = [m['client'] - sum(m[k] for k in drop) - (0.1 * m['round'] if not drop else 0) for m in M]
    print(f'  what-if {name:44}: merge:only p99 (slowest) {max(xs):5.0f}, p50 {med(xs):5.0f}, against now {max(m["client"] for m in M)}')
