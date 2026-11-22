"""cycsplit.py TAG WL_F0.json [WL_F1.json]: one B0-33 leader's cycle, split on clocks that hold under load
(`tests/am1/results/b033-percmd.md`). WL files are `tests/fanout/wlobj.py fetch` extracts of the leader object (f0) and
the near voter (f1).

- The cycle (near voter's handler entry for entry k+1 minus entry k), the voter's handling, and the idle wire round
  trip, as second.py: voter clock and the leader's fresh idle clock.
- The leader's CPU per cycle: Workers Logs' `cpuTimeMs` summed over every leader invocation in 90 to 300 s, over the
  entries sent to the near voter in that window (ring count). One isolate runs one turn at a time, so every input's CPU
  is in series with the entry path, whatever route carried it.
- The rest of the leader's excess: time between the voter's answer and the next entry in which the leader ran no
  CPU (waiting to be scheduled, or on its own I/O).
- Per command: the same CPU over the commands cut (Cut rows, ring count).
- The timestamp check: on the voter, Workers Logs' `timestamp - wallTimeMs` against the trace's `Entered` (same
  isolate, fresh clock); on the leader, `timestamp - wallTimeMs` against the sender's `x-graft-sent`, idle and loaded.
  A start stamped before its sender sent means the leader's events are stamped on its own late clock."""
import os, sys; sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import *
tag, wl0 = sys.argv[1], sys.argv[2]
wl1 = sys.argv[3] if len(sys.argv) > 3 else None
a = ms(RUNS[tag][0]); lo, hi = a + 90000, a + 1000 * RUNS[tag][1]; secs = (hi - lo) / 1000
_, _, rows = load(tag, (0, 1), pad=120000)
L, F = rows[0], rows[1]
ent = {I(r[3]): r[0] for r in F if r[1] == 25 and I(r[4]) % 64 == 1 and r[2] == 0}
left = {I(r[3]): r[0] for r in F if r[1] == 24 and r[2] == 0}
sent = {I(r[3]): r[0] for r in L if r[1] == 23 and r[2] == 1}
ans = {I(r[3]): r[0] for r in L if r[1] == 21 and r[2] == 1}
rt = pc([ans[c] - t for c, t in sent.items() if c in ans and t < a - 2000], .5)
seen, E = set(), []
for i, r in enumerate(L):
    if r[1] == 12 and r[2] == 1 and I(r[4]) not in seen:
        seen.add(I(r[4]))
        for j in range(i + 1, min(len(L), i + 400)):
            if L[j][1] == 23 and L[j][2] == 1: E.append(I(L[j][3])); break
V = [(ent[c2] - ent[c], ent[c2] - left[c] - rt, left[c] - ent[c]) for c, c2 in zip(E, E[1:])
     if c in left and c in ent and c2 in ent and lo <= ent[c] < hi and ent[c2] - left[c] < 6000]
cyc, exc, hand = (sum(v[i] for v in V) / len(V) for i in range(3))
entries = len({I(r[4]) for r in L if r[1] == 12 and r[2] == 1 and lo <= r[0] < hi}) / secs
cuts = [r for r in L if r[1] == 29 and lo <= r[0] < hi]
cmds = sum(sum((I(r[3]) >> (12 * i)) & 4095 for i in range(4)) for r in cuts) / secs
W0 = json.load((lzma.open if wl0.endswith(".xz") else open)(wl0, "rt"))
cpu = sum(e['cpu'] or 0 for e in W0 if lo <= e['ts'] < hi) / secs
inv = sum(1 for e in W0 if lo <= e['ts'] < hi) / secs
cpe = cpu / entries
print(f"{tag} {COLO.get(tag)} 90-{RUNS[tag][1]} s: {len(V)} cycles, entries {entries:.2f}/s, commands cut {cmds:.1f}/s ({cmds / entries:.0f} an entry)")
print(f"  cycle mean {cyc:.0f} (p50 {pc([v[0] for v in V], .5)}) = idle wire {rt} + voter handling {hand:.0f} + leader excess {exc:.0f}")
print(f"  leader excess {exc:.0f} = leader CPU {cpe:.0f} ({cpu:.0f} ms/s over {entries:.2f} entries/s; {inv:.0f} invocations/s) "
      f"+ leader not on CPU {exc - cpe:.0f}")
print(f"  shares of the cycle: wire {100 * rt / cyc:.0f}%, voter {100 * hand / cyc:.0f}%, leader CPU {100 * cpe / cyc:.0f}%, leader off CPU {100 * (exc - cpe) / cyc:.0f}%")
print(f"  leader CPU per command cut {cpu / cmds:.2f} ms; CPU {cpu / 10:.0f}% of wall")
ld = {(int(e['from']), int(e['call'])): e for e in W0 if e.get('path') == '/raft' and e.get('call') and e.get('sent')}
for lab, f in (('idle', lambda e: e['ts'] - e['wall'] < a - 2000), ('load', lambda e: lo <= e['ts'] - e['wall'] < hi)):
    v = [e['ts'] - e['wall'] - int(e['sent']) for e in ld.values() if int(e['from']) in (1, 2) and f(e)]
    print(f"  leader WL start - voter's x-graft-sent, {lab}: n {len(v)} p10/p50/p90 {pc(v, .1)}/{pc(v, .5)}/{pc(v, .9)}")
if wl1:
    W1 = {int(e['call']): e for e in json.load((lzma.open if wl1.endswith(".xz") else open)(wl1, "rt")) if e.get('path') == '/raft' and str(e.get('from')) == '0' and e.get('call')}
    j = [(W1[c]['ts'] - W1[c]['wall'] - ent[c], W1[c]['wall'], left[c] - ent[c]) for c in W1 if c in ent and c in left]
    print(f"  voter WL start - its Entered: n {len(j)} p10/p50/p90 {pc([x[0] for x in j], .1)}/{pc([x[0] for x in j], .5)}/{pc([x[0] for x in j], .9)}; "
          f"its wall p50 {pc([x[1] for x in j], .5)} against handler p50 {pc([x[2] for x in j], .5)} and idle wire RT {rt}")
