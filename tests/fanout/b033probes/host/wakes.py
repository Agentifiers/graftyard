"""Leader storage confirms without the leader's clock. Each Sync row counts the 100 ms ticker wakes that ran while the
sync() was pending (a timer cannot fire early, so wakes x 100 ms is a floor on the real duration), and the requests that
entered meanwhile. Steady window, transactions of >= 30 statements (an entry's)."""
import os, sys; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import common
from common import *
common.RUNS.update({'sabC': ('2026-11-21T11:48:41Z', 300), 'sabB2': ('2026-11-21T12:09:54Z', 300)}); COLO.update({'sabC': '?', 'sabB2': '?'})
print('run    colo | entry confirms | >=3 wakes inside (>=300 ms real): n, share | >=5 | >=10 (>=1 s real) | most wakes | stamped wall p50/p90 of all | of those with 0 wakes inside: stamped wall p90, share stamped >=200 ms | requests entering per confirm p50/p90')
for tag in sys.argv[1:]:
    a, b, rows = load(tag); L = [r for r in rows[0] if a + 90000 <= r[0] < b and r[1] == 31 and (I(r[4]) & 0xfffff) >= 30]
    n = len(L); w = [r[2] for r in L]; wall = [I(r[5]) for r in L]
    z = [I(r[5]) for r in L if r[2] == 0]
    inp = [I(r[3]) & 0xfffff for r in L]
    print(f'{tag:6} {COLO[tag]:3} | {n} | {sum(x >= 3 for x in w)}, {100 * sum(x >= 3 for x in w) / n:.0f}% | {sum(x >= 5 for x in w)} | {sum(x >= 10 for x in w)} | {max(w)} | {pc(wall, .5)}/{pc(wall, .9)} | {pc(z, .9)}, {100 * sum(x >= 200 for x in z) / max(1, len(z)):.0f}% of {len(z)} | {pc(inp, .5)}/{pc(inp, .9)}')
