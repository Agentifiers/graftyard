"""Is the entry's own AppendEntries fetch issued before or after its own transaction? Ring order, not clocks: Trace rows
are pushed in execution order. Per entry on site 0: position of the first voter CallSent (23) at/after its AppendQueued
(12), of the next Flushed (16), and of the Sync (31) row; also ms from AppendQueued stamp to CallSent stamp."""
import os, sys; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import *
def raw(tag):
    a = ms(RUNS[tag][0]); b = a + 1000 * RUNS[tag][1]
    out, seen = [], set()
    for line in lzma.open(raw_path(tag), 'rt'):
        d = json.loads(line)
        if 'rows' not in d or 'learner' in d or d.get('site') != 0: continue
        for r in d['rows']:
            k = tuple(r)
            if k not in seen and a <= r[0] < b: seen.add(k); out.append(r)
    return a, out
for tag in sys.argv[1:]:
    a, L = raw(tag)
    # out-of-order stamps in the ring?
    back = sum(1 for i in range(1, len(L)) if L[i][0] < L[i - 1][0] - 2)
    n = before = after = 0; gap = []; issued = []
    i = 0
    while i < len(L):
        r = L[i]
        if r[1] == 12 and r[2] < 192 and r[0] >= a + 30000:
            # scan forward for first 23 to a voter and first 16
            p23 = p16 = None
            for j in range(i + 1, min(len(L), i + 400)):
                if p23 is None and L[j][1] == 23 and L[j][2] == r[2]: p23 = j
                if p16 is None and L[j][1] == 16: p16 = j
                if p23 is not None and p16 is not None: break
            if p23 is not None and p16 is not None:
                n += 1
                if p23 < p16: before += 1
                else: after += 1
                gap.append(L[p23][0] - r[0])
        if r[1] == 13 and r[2] < 192: issued.append(I(r[5]))
        i += 1
    print(f'{tag:6} {COLO[tag]} | rows {len(L)} back-steps {back} | voter appends {n}: fetch before own Flushed {before}, after {after} | CallSent - AppendQueued ms p50/p90/p99 {pc(gap,.5)}/{pc(gap,.9)}/{pc(gap,.99)} | AppendIssued ms p50/p90/p99 {pc(issued,.5)}/{pc(issued,.9)}/{pc(issued,.99)}')
