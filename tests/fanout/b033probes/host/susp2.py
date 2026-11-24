import os, sys; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import *
from tracerows import load
for tag in sys.argv[1:]:
    a = ms(RUNS[tag][0])
    rows = load(raw_path(tag), keep=lambda r: r[0] >= a and r[1] in (8, 28, 31))
    rep = sorted({r[0] // 1000 * 1000 for s in (1, 2) for r in rows[s] if r[1] == 28 and r[2] == 0})
    sy = sorted((r[0] - I(r[5]), r[0], I(r[5])) for r in rows[0] if r[1] == 31)
    lg = sorted((r[0] - I(r[5]), r[0], I(r[5])) for r in rows[0] if r[1] == 8 and I(r[5]) >= 500)
    big = [s for s in sy if s[2] >= 1000]
    hit = 0; out = []
    for t in rep:
        c = max((s[2] for s in sy if s[0] <= t + 1000 and s[1] >= t - 4000), default=0)
        l = max((s[2] for s in lg if s[0] <= t + 1000 and s[1] >= t - 4000), default=0)
        out.append((round((t - a) / 1000), c, l)); hit += c >= 1000 or l >= 500
    print(f'{tag:6} {COLO[tag]}: leader confirms >=1 s in load: {len(big)} (>=2 s: {sum(s[2] >= 2000 for s in big)}); seconds with a report about the leader: {len(rep)}; of them with a leader confirm >=1 s or a ticker stall >=500 ms overlapping the 4 s before: {hit}')
    if len(out) <= 14: print('      (s into load, longest leader confirm overlapping the 4 s before, longest stall):', out)
