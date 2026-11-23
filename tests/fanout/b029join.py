#!/usr/bin/env python3
"""b029join.py TAG LOAD_START_ISO [SECONDS] -> B0-29's frozen-batch wait against the leader's CPU, for
tests/am1/results/b029-m3f.md ("The frozen-batch wait and the leader's CPU").

Inputs, run from the repository root: the row's trace `tests/fanout/raw/tsave-TAG.jsonl.xz` (site 0 leading) and the
leader object's Workers Logs `tests/fanout/raw/wl-29/TAG-f0.json.xz` (`wlobj.py fetch`).

1. Slices. heavyturns.py's join and classes (`tests/am1/results/b033-pfa.md`, "The join"): each leader `/raft` call
   joined on (caller, `x-graft-call`) to its `Entered` row (kind 25) owns the CPU of the trace rows up to the next
   `Entered`. Apply slices are split by whether the same slice also sent an AppendEntries (a Work row with appends).
2. The leader's loop: ticker lag rows (kind 8, 20 ms or more) and `CallSent`'s loop lag (kind 23).
3. Rounds to the near follower (site 1): `AppendQueued` (12) to its ok `AppendEnded` (14), by quantile, with the joined
   CPU of the slices between them.
4. Per command, arrival (`Entered`, route 3) to the leader's `Applied` (4) of its own entry, matched as slotsplit.py
   does, then three replays on today's entry times [inference]:
   - cpu: every joined send turn's CPU (the slice that holds an entry's first `AppendQueued`) taken off each cycle the
     command waits through, which bounds any cut to the leader's CPU on the commit path;
   - recut D: while the entry before it is in flight, the leader asks a new dependency read each time the last one
     answers, every D ms, and a batch takes every command admitted before the start of the newest read answered by its
     append (today: the batch frozen at its cut);
   - floor: every command rides the first entry appended after it arrives (no freeze, no read in series).
"""
import bisect, collections, json, lzma, sys
from datetime import datetime


def ms(s):
    return int(datetime.fromisoformat(s.replace('Z', '+00:00')).timestamp() * 1000)


def pc(xs, q):
    xs = sorted(xs)
    return round(xs[min(len(xs) - 1, int(q * len(xs)))]) if xs else None


def q3(xs):
    return f'{pc(xs, .5)}/{pc(xs, .9)}/{pc(xs, .99)}'


tag, start = sys.argv[1], sys.argv[2]
lo = ms(start)
hi = lo + 1000 * (int(sys.argv[3]) if len(sys.argv) > 3 else 600)
seen, rows = set(), []
for line in lzma.open(f'tests/fanout/raw/tsave-{tag}.jsonl.xz', 'rt'):
    d = json.loads(line)
    if d.get('site') != 0 or 'learner' in d or 'rows' not in d:
        continue
    for r in d['rows']:
        if tuple(r) not in seen:
            seen.add(tuple(r))
            rows.append([int(float(x)) for x in r])
n = len(rows)
ent = [i for i, r in enumerate(rows) if r[1] == 25]
pos = {(r[2], r[3]): i for i, r in enumerate(rows) if r[1] == 25 and r[3]}

# 1. Slices.
def slice_of(i, cpu):
    c = collections.Counter(cpu=cpu)
    j = bisect.bisect_right(ent, i)
    for r in rows[i + 1:ent[j] if j < len(ent) else n]:
        if r[1] == 30:
            c['appends'] += r[2]
            c['stmts'] += r[4] & 0xfffff
            c['applied'] += r[5] & 0xffff
            c['events'] += (r[5] >> 16) & 0xffff
            c['resolutions'] += r[5] >> 32
        elif r[1] in (10, 31):
            c['confirm'] += 1
        elif r[1] == 16:
            c['flushed'] += 1
            c['flushStmts'] += r[4]
        elif r[1] in (23, 29):
            c['sends'] += 1
    return c


def kind(c):
    if c['applied']:
        return 'applies entries'
    if c['flushed'] or c['stmts']:
        return 'deferred transaction'
    if c['confirm']:
        return 'storage confirm'
    if c['sends'] or c['appends']:
        return 'cuts or sends'
    if c['resolutions']:
        return 'resolutions only'
    return 'no traced work'


allcpu, cs = 0, {}
for x in json.load(lzma.open(f'tests/fanout/raw/wl-29/{tag}-f0.json.xz', 'rt')):
    if not lo <= x['ts'] < hi:
        continue
    allcpu += x.get('cpu') or 0
    if x.get('path') == '/raft' and x.get('call'):
        i = pos.get((int(x.get('from') or 0), int(x['call'])))
        if i is not None:
            cs[i] = slice_of(i, x.get('cpu') or 0)
tot = sum(c['cpu'] for c in cs.values())
secs = (hi - lo) / 1000
print(f'{tag}: leader CPU {allcpu / 1000:.1f} s in {secs:.0f} s ({allcpu / secs / 1000:.2f} vCPU); '
      f'{len(cs)} /raft slices joined, {100 * tot / max(allcpu, 1):.0f}% of it')
by = collections.defaultdict(list)
for c in cs.values():
    by[kind(c)].append(c)
for k, v in sorted(by.items(), key=lambda kv: -sum(c['cpu'] for c in kv[1])):
    cpu = sum(c['cpu'] for c in v)
    line = f'  {k:21} n {len(v):6}  CPU {100 * cpu / tot:3.0f}%  mean {cpu / len(v):5.1f}  p50/p90 {pc([c["cpu"] for c in v], .5)}/{pc([c["cpu"] for c in v], .9)}'
    if k == 'applies entries':
        e, ev = sum(c['applied'] for c in v), sum(c['events'] for c in v)
        line += f'  | {ev / e:.1f} events and {cpu / e:.1f} ms an entry, {cpu / ev:.2f} ms an event'
        for lab, keep in (('with an append', lambda c: c['appends'] > 0), ('apply only', lambda c: c['appends'] == 0)):
            w = [c for c in v if keep(c)]
            line += f'; {lab} {sum(c["cpu"] for c in w) / max(sum(c["events"] for c in w), 1):.2f}'
    if k == 'deferred transaction':
        fl = [c for c in v if c['flushed']]
        if fl:
            st, fc = sum(c['flushStmts'] for c in fl), sum(c['cpu'] for c in fl)
            line += f'  | {len(fl)} flushed: {st / len(fl):.0f} statements, {fc / len(fl):.1f} ms, {fc / max(st, 1):.2f} ms a statement'
    print(line)

# 2. The leader's loop.
win = [r for r in rows if lo <= r[0] < hi]
lag = [r[5] for r in win if r[1] == 8]
sent = [r[5] for r in win if r[1] == 23 and r[2] < 192]
print(f'  loop: {len(lag)} ticker lags of 20 ms or more; CallSent loop lag p50/p99/max {pc(sent, .5)}/{pc(sent, .99)}/{max(sent, default=0)}')

# 3. Near rounds.
qd, ed, size = {}, {}, {}
for i, r in enumerate(rows):
    if r[2] == 1 and lo <= r[0] < hi:
        if r[1] == 12:
            qd.setdefault(r[3], i)
            size[r[3]] = r[5]
        elif r[1] == 14 and r[4] == 0:
            ed.setdefault(r[3], i)
cpos = sorted(cs)
R = []
for rnd, i in qd.items():
    j = ed.get(rnd)
    if j is not None:
        inside = [cs[p] for p in cpos[bisect.bisect_left(cpos, i):bisect.bisect_left(cpos, j)]]
        R.append((rows[j][0] - rows[i][0], size[rnd], sum(c['cpu'] for c in inside),
                  sum(c['cpu'] for c in inside if kind(c) in ('applies entries', 'deferred transaction'))))
R.sort()
under = [r[0] for r in R if r[1] <= 65536]
over = [r[0] for r in R if r[1] > 65536]
print(f'  near rounds {len(R)}: p50/p90/p99 {q3([r[0] for r in R])}; AppendEntries up to 64 KiB {q3(under)} (n {len(under)}), '
      f'over {q3(over)} (n {len(over)})')
for lab, v in (('fastest half', R[:len(R) // 2]), ('p50 to p90', R[len(R) // 2:int(.9 * len(R))]),
               ('slowest 10%', R[int(.9 * len(R)):]), ('slowest 2%', R[int(.98 * len(R)):])):
    m = lambda k: sum(r[k] for r in v) / len(v)
    print(f'    {lab:12} round mean {m(0):4.0f} ms, bytes p50 {pc([r[1] for r in v], .5):6}, '
          f'joined leader CPU in it {m(2):4.1f} ms (apply and transaction {m(3):4.1f})')

# 4. Per command, and the replays.
arr, cuts, app, applied = [], [], {}, {}
for r in rows:
    if r[1] == 25 and r[4] % 64 == 3:
        arr.append(r[0])
    elif r[1] == 29:
        cuts.append((r[0], (r[2] // 16) % 4))
    elif r[1] == 12 and r[2] < 192:
        app.setdefault(r[4], r[0])
    elif r[1] == 4:
        applied.setdefault(r[4], r[0])
cuts.sort()
A = sorted((t, i) for i, t in app.items())
At = [t for t, _ in A]
ai = sorted(applied)


def apply_of(i):
    j = bisect.bisect_left(ai, i)
    return applied[ai[j]] if j < len(ai) else None


ck = []  # each cut's entry, as the (held + 1)-th append after it
for at, held in cuts:
    k = bisect.bisect_right(At, at) + held
    ck.append(k if k < len(A) else None)
ct = [c[0] for c in cuts]
send_cpu = {}
for idx, i in ((r[4], i) for i, r in enumerate(rows) if r[1] == 12 and r[2] < 192):
    if idx in send_cpu:
        continue
    j = bisect.bisect_right(ent, i) - 1
    send_cpu[idx] = cs[ent[j]]['cpu'] if j >= 0 and ent[j] in cs else None
known = [v for v in send_cpu.values() if v is not None]
mean_send = sum(known) / max(len(known), 1)
base, cpu_cut = [], []
for a in sorted(arr):
    if not lo <= a < hi:
        continue
    j = bisect.bisect_left(ct, a)
    if j >= len(cuts) or ck[j] is None or apply_of(A[ck[j]][1]) is None:
        continue
    k, k0 = ck[j], max(0, bisect.bisect_right(At, a) - 1)
    w = apply_of(A[k][1]) - a
    base.append(w)
    cpu_cut.append(w - sum(send_cpu.get(A[x][1]) or mean_send for x in range(k0, k + 1)))
print(f'  commands {len(base)}: arrival to its entry applied p50/p90/p99 {q3(base)}; send turns joined {len(known)} of '
      f'{len(send_cpu)}, CPU p50/p99 {pc(known, .5)}/{pc(known, .99)}')
print(f'    cpu: every send turn\'s CPU off each cycle waited {q3(cpu_cut)}')
bound = {}
for (at, _), k in zip(cuts, ck):
    if k is not None:
        bound.setdefault(k, at)
ks = sorted(bound)
cmds = [a for a in sorted(arr) if lo <= a < hi]
for D in (40, 60, 80, 120, None):
    m, b = 0, []
    for k in ks:
        tc, td = bound[k], A[k][0]
        nr = int((td - tc) // D) - 1 if D else 0
        m = max(m, tc + nr * D if nr >= 1 else tc)
        b.append(m)
    v = []
    for a in cmds:
        j = bisect.bisect_left(b, a)
        if j < len(ks) and apply_of(A[ks[j]][1]):
            v.append(apply_of(A[ks[j]][1]) - a)
    print(f'    recut {D or "never (today)":>5}: {q3(v)}')
v = []
for a in cmds:
    j = bisect.bisect_left(At, a)
    if j < len(A) and apply_of(A[j][1]):
        v.append(apply_of(A[j][1]) - a)
print(f'    floor (rides the next entry appended): {q3(v)}')
