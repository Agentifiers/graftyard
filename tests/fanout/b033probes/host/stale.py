"""How far behind the leader's event loop runs, per run: its Date.now() at a /raft send against the near follower's
handler-entry stamp, net of the idle value of the same difference (clock offset + one-way wire). Steady window.
lease-clock.md's probe: an isolate whose turns are 400 ms of CPU reads 0.7-1.7 s behind; idle about 0."""
import os, sys; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import common
from common import *
common.RUNS.update({'sabC': ('2026-11-21T11:48:41Z', 300), 'sabB2': ('2026-11-21T12:09:54Z', 300)}); COLO.update({'sabC': '?', 'sabB2': '?'})
print('run    colo | leader inputs/s | leader clock behind, p10/p50/p90 ms | entries/s | agent cmds/s cut')
for tag in sys.argv[1:]:
    a, b, rows = load(tag, sites=(0, 1), pad=30000); L = rows[0]
    sent = {I(r[3]): r[0] for r in L if r[1] == 23 and r[2] == 1}
    ent = {I(r[3]): r[0] for r in rows[1] if r[1] == 25 and I(r[4]) % 64 == 1 and r[2] == 0}
    idle = pc([ent[c] - t for c, t in sent.items() if c in ent and t < a - 2000], .5)
    st = [ent[c] - t - idle for c, t in sent.items() if c in ent and a + 90000 <= t < b]
    T = (b - a - 90000) / 1000
    W = [r for r in L if a + 90000 <= r[0] < b]
    print(f'{tag:6} {COLO[tag]:3} | {sum(r[1] == 25 for r in W) / T:.0f} | {pc(st, .1)}/{pc(st, .5)}/{pc(st, .9)} | {len({I(r[4]) for r in W if r[1] == 12 and r[2] == 1}) / T:.2f} | {sum(sum((I(r[3]) >> (12 * i)) & 0xfff for i in range(4)) for r in W if r[1] == 29) / T:.0f}')
