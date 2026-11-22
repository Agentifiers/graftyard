"""Q2: entries/s x agent commands per entry, split ramp (0-90 s) and steady (90 s to end of site 0's leadership)."""
import os, sys; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import *
print('run    colo | 0-90 s: agent cmds/s reaching cuts, backlog p50 | steady 90 s+: entries/s x agent per entry = agent/s | capped share | cycle after a capped cut: p50, mean | all-capped ceiling 64/mean | needed per entry for 100/s at this entry rate | AgentTokens ends')
for tag in sys.argv[1:]:
    a, b, rows = load(tag); R = rows[0]
    cuts = [(r[0], (I(r[3]) >> 36) & 0xfff, r[2] % 16, (I(r[4]) >> 39) & 0x1fff) for r in R if r[1] == 29]
    ramp = [c for c in cuts if c[0] < a + 90000]; st = [c for c in cuts if c[0] >= a + 90000]
    d = (b - a - 90000) / 1000
    if d <= 0 or not st: continue
    capg = [st[i + 1][0] - st[i][0] for i in range(len(st) - 1) if st[i][2] == 4]
    mean = sum(capg) / max(1, len(capg)); eps = len(st) / d; ag = sum(c[1] for c in st)
    print(f'{tag:6} {COLO[tag]} | {sum(c[1] for c in ramp) / 90:5.1f}, {pc([c[3] for c in ramp], .5):3} | {eps:4.2f} x {ag / len(st):4.1f} = {ag / d:5.1f} | {100 * sum(c[2] == 4 for c in st) / len(st):3.0f}% | {pc(capg, .5)}, {mean:4.0f} | {64000 / mean:5.1f} | {100 / eps:5.1f} | {sum(c[2] == 3 for c in st)}')
