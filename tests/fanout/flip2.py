#!/usr/bin/env python3
"""flip2.py TAG RUNID: `edge-refusal`'s flip 2 (over-refusal) as the lead ruled it on 2026-11-24, from B0-33 h1A
(`decisions/DECISIONS.md`, "`edge-refusal`"; `tests/am1/results/b033-h1-set.md`).

A refusing 10 s window counts only when its refused writes' budget left at the check (p50 of the `edgefull` lines'
`budgetMs`) exceeds the leader's expected_wait in that window: its agent queue at the cut drain
(`edgefull.leader_windows`) plus the leader's own round_ms in that window (a1's `admission` samples, every 10 s from the
load start). The larger of the window and the one before it, as edgefull reads the cut clock. It flips over 10% of
refusing windows.

It then prints `decisions/OPEN-QUESTIONS.md` A50's readings 2 and 3: the refused writes' budget against the refusing
advert's own wait (0 by construction), and A50's settle reading, each refusing window's advert p50 against the sent
agent writes' commit p50 (a1's pushes, `a1-pushes-b0-33-<RUNID>.jsonl` or `.xz`).

TAG is the row's tag (`tests/fanout/raw/edgefull-<TAG>.jsonl`, `tsave-<TAG>`); RUNID is a1's run id
(`tests/am1/raw/a1-b0-33-<RUNID>.jsonl` and `tests/am1/raw/runs/a1-b0-33-f<TAG>-b033.log`). Run from the repository
root.
"""
import sys, json, collections
sys.path.insert(0, 'tests/fanout')
import edgefull
from datetime import datetime
tag, run = sys.argv[1:3]
import lzma
import os
# A kept set's raws are committed under tests/fanout/raw/a1-<set>/ (tests/am1/raw/ is not kept); a live row's are local.
KEPT = [d for d in (f'tests/fanout/raw/{x}' for x in os.listdir('tests/fanout/raw') if x.startswith('a1-'))
        if os.path.isdir(d)]
def find(name):
    for d in ['tests/am1/raw', 'tests/am1/raw/runs', *KEPT]:
        if os.path.exists(f'{d}/{name}'):
            return f'{d}/{name}'
    raise SystemExit(f'{name}: not found')
row = json.loads([l for l in open(find(f'a1-b0-33-{run}.jsonl')) if l.strip()][-1])
log = open(find(f'a1-b0-33-f{tag}-b033.log')).read()
import re
t0 = re.search(r'300 s from (\S+Z)', log).group(1)
T0 = int(datetime.fromisoformat(t0.replace('Z', '+00:00')).timestamp() * 1000)
rounds = sorted((T0 + a['s'] * 1000, a['roundMs']) for a in row['admission'] if a.get('roundMs') is not None)
def round_at(ms):
    best = [r for t, r in rounds if t <= ms + 10_000]
    return best[-1] if best else None
lw = edgefull.leader_windows(tag)
ef = f'tests/fanout/raw/edgefull-{tag}.jsonl'
lines = [json.loads(x) for x in (open(ef) if os.path.exists(ef) else lzma.open(ef + '.xz', 'rt')) if x.strip()]
ref = [x for x in lines if x['edgefull'] in ('refused', 'held')]
W = 10_000
byw = collections.defaultdict(list)
for x in ref:
    byw[x['t'] // W].append(x)
over = n_over = 0
for k, xs in sorted(byw.items()):
    q = [w[0] for w in (lw.get(k), lw.get(k - 1)) if w and w[0] is not None]
    rd = [r for r in (round_at(k * W), round_at((k - 1) * W)) if r is not None]
    if not q or not rd:
        continue
    wait = max(q) + max(rd)
    b = sorted(x['budgetMs'] for x in xs)[len(xs) // 2]
    over += b > wait
    n_over += sum(1 for x in xs if x['budgetMs'] > wait)
share = over / len(byw) if byw else 0
print(f"flip 2 (as ruled 2026-11-24, budget left at the check over the leader's expected_wait with each window's own "
      f"round_ms): {over} of {len(byw)} refusing windows ({share:.0%}), {n_over} of {len(ref)} refusals; flips over 10%: "
      + ('FLIPPED' if share > 0.10 else 'not met'))
# A50's reading 2: budget left against the refusing advert's own wait. advert_says refuses only when that wait passes the
# budget, so it reads 0 by construction; printed once to close that question.
own = sum(1 for x in ref if x.get('waitMs') is not None and x['budgetMs'] > x['waitMs'])
print(f"A50 reading 2 (budget left over the refusing advert's own waitMs): {own} of {len(ref)} refusals "
      f"(0 by construction)")
# A50's reading 3, its settle reading (the lead's ruling of 2026-11-24, after clA): per refusing window, the advert's
# p50 wait against the commit p50 of agent pushes a1 saw sent and committed (`commit` in their server-timing) in the
# same 10 s window. An advert over what sent writes got refused writes that would have made it. Windows with fewer
# than MIN_SENT sent agent writes are excluded. Sent writes are only those whose budget passed the advert, so their
# commit times are a survivor sample: the sent count is printed beside each window.
MIN_SENT = 10
try:
    pf = find(f'a1-pushes-b0-33-{run}.jsonl')
    pushes = open(pf)
except SystemExit:
    pushes = lzma.open(find(f'a1-pushes-b0-33-{run}.jsonl.xz'), 'rt')
sent = collections.defaultdict(list)
for line in pushes:
    p = json.loads(line)
    if p['kind'] != 'agent' or p['result'] != 'ok' or not p.get('timing'):
        continue
    t = dict(a.split(';dur=') for a in p['timing'].split(', ') if ';dur=' in a)
    if 'commit' in t:
        sent[p['end'] // W].append(float(t['commit']))
p50 = lambda v: sorted(v)[len(v) // 2]
over3 = judged = 0
ratios = []
for k, xs in sorted(byw.items()):
    adv = p50([x['waitMs'] for x in xs if x.get('waitMs') is not None] or [0])
    cs = sent.get(k, [])
    if len(cs) < MIN_SENT:
        print(f"  window {k}: advert p50 {adv} ms, {len(cs)} sent agent writes: excluded (under {MIN_SENT})")
        continue
    judged += 1
    over3 += adv > p50(cs)
    ratios.append(adv / p50(cs))
    print(f"  window {k}: advert p50 {adv} ms, sent agent commit p50 {p50(cs):.0f} ms over {len(cs)} sent, "
          f"{len(xs)} refused")
share3 = over3 / judged if judged else 0
print(f"A50 reading 3 (advert p50 over sent agent writes' commit p50): {over3} of {judged} refusing windows "
      f"({share3:.0%}), {len(byw) - judged} excluded under {MIN_SENT} sent; advert/commit median "
      f"{p50(ratios) if ratios else 0:.2f}; flips over 10%: " + ('FLIPPED' if share3 > 0.10 else 'not met'))
