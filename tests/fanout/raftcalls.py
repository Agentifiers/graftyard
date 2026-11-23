#!/usr/bin/env python3
"""raft-split: what the leader's learner `/raft` calls carry by stage, and their CPU on those stages, for
tests/am1/results/b033-profile.md.

Each test-faults `/raft` call logs one `Trace::RaftCall` row (kind 33, `RaftCall::row` in workers/site/src/host.rs):
the body it decoded, its envelopes and `ReadIndex` asks, the messages and bytes held for the caller that its answer
took, the messages and bytes it answered, and the wall time its answer waited on the output gate. The isolate's clock
does not move inside a turn, so the stages are sized, not timed. The leader's Workers Logs rows (`wlobj.py fetch`)
carry each call's CPU and its `x-graft-call` id, so a join on (caller, call id) gives CPU per call beside its sizes,
and a least-squares fit splits the CPU into a part per call, per KB decoded, per ask, per held message and per KB
answered [inference: a linear model of each stage].

Usage, from the repository root:
  raftcalls.py TSAVE.jsonl[.xz] WL.json[.xz] LOAD_START_ISO [FROM_S TO_S]   window default 90 to 300 s of the load
"""
import json, lzma, sys
from datetime import datetime, timezone


HEAVY_MS = 20


def opener(p):
    return lzma.open(p, 'rt') if p.endswith('.xz') else open(p)


def field(v, at, bits):
    return (v >> at) & ((1 << bits) - 1)


def unpack(index, ms):
    return {'body': field(index, 0, 22), 'envs': field(index, 22, 6), 'asks': field(index, 28, 6),
            'held': field(index, 34, 6), 'gate_ms': field(index, 40, 12), 'answer': field(ms, 0, 23),
            'held_bytes': field(ms, 23, 23), 'out': field(ms, 46, 6)}


def solve(rows, ys):
    # Normal equations by Gaussian elimination: no numpy in the orb.
    k = len(rows[0])
    a = [[sum(r[i] * r[j] for r in rows) for j in range(k)] + [sum(r[i] * y for r, y in zip(rows, ys))]
         for i in range(k)]
    for c in range(k):
        p = max(range(c, k), key=lambda r: abs(a[r][c]))
        a[c], a[p] = a[p], a[c]
        if abs(a[c][c]) < 1e-12:
            return None
        for r in range(k):
            if r != c:
                f = a[r][c] / a[c][c]
                a[r] = [x - f * y for x, y in zip(a[r], a[c])]
    return [a[i][k] / a[i][i] for i in range(k)]


def pc(v, q):
    v = sorted(v)
    return v[min(len(v) - 1, int(q * len(v)))] if v else 0


def main():
    tsave, wl, start = sys.argv[1], sys.argv[2], sys.argv[3]
    lo_s, hi_s = (int(sys.argv[4]), int(sys.argv[5])) if len(sys.argv) > 5 else (90, 300)
    t0 = int(datetime.fromisoformat(start.replace('Z', '+00:00')).timestamp() * 1000)
    calls, seen = {}, set()
    for line in opener(tsave):
        d = json.loads(line)
        if d.get('site') != 0 or 'learner' in d or 'rows' not in d:
            continue
        for r in d['rows']:
            if int(r[1]) != 33 or tuple(r) in seen:
                continue
            seen.add(tuple(r))
            calls[(int(r[2]), int(float(r[3])))] = unpack(int(float(r[4])), int(float(r[5])))
    events = json.load(opener(wl))
    joined = []
    for x in events:
        if x.get('path') != '/raft' or not x.get('call'):
            continue
        if not t0 + lo_s * 1000 <= x['ts'] < t0 + hi_s * 1000:
            continue
        f = int(x.get('from') or 0)
        c = calls.get((f, int(x['call'])))
        if c is not None:
            joined.append(((c, x.get('cpu') or 0, x.get('wall') or 0), f))
    # A learner's address is 192 or more (`graftyard_types::learner_addr`).
    learner = [(c, cpu, wall) for (c, cpu, wall), f in joined if f >= 192]
    print(f'{len(calls)} RaftCall rows on the leader, {len(joined)} joined to Workers Logs in {lo_s} to {hi_s} s, '
          f'{len(learner)} from learners')
    if not learner:
        sys.exit('no calls joined: is the trace from a test-faults build with kind 33?')
    cpu = [j[1] for j in learner]
    cols = ['body', 'asks', 'held', 'held_bytes', 'answer', 'gate_ms']
    print(f'calls {len(learner)}, CPU ms a call mean {sum(cpu) / len(cpu):.2f} p50 {pc(cpu, .5)} p90 {pc(cpu, .9)}'
          f' p99 {pc(cpu, .99)}')
    for k in cols:
        v = [j[0][k] for j in learner]
        print(f'  {k:10} mean {sum(v) / len(v):9.1f} p50 {pc(v, .5):7} p90 {pc(v, .9):7} max {max(v)}')
    # The heavy calls apart: 20 ms or more carried 36 to 86% of the baseline's learner `/raft` CPU (b033-profile.md).
    heavy = [j for j in learner if j[1] >= HEAVY_MS]
    print(f'calls of {HEAVY_MS} ms or more: {len(heavy)} of {len(learner)}, '
          f'{100 * sum(j[1] for j in heavy) / max(sum(cpu), 1):.0f}% of the CPU; stage means, heavy against the rest:')
    rest = [j for j in learner if j[1] < HEAVY_MS]
    for k in ['body', 'envs', 'asks', 'held', 'held_bytes', 'out', 'answer', 'gate_ms']:
        m = lambda js: sum(j[0][k] for j in js) / len(js) if js else 0
        print(f'  {k:10} {m(heavy):10.1f} {m(rest):10.1f}')
    # CPU per call by stage: a fixed part per call, per KB decoded, per ask and per other envelope stepped (a learner's
    # ack, whose step may build its next append), per message the step built (answered less held), per held message
    # taken, and per KB answered.
    x = [[1, c['body'] / 1024, c['asks'], c['envs'] - c['asks'], max(c['out'] - c['held'], 0), c['held'],
          c['answer'] / 1024] for c, _, _ in learner]
    names = ['per call', 'per KB decoded', 'per ask stepped', 'per other envelope', 'per message built',
             'per held message', 'per KB answered']
    # A stage that does not vary in the window is dropped from the fit, and said so.
    keep = [i for i in range(len(names)) if i == 0 or len({r[i] for r in x}) > 1]
    for i in range(len(names)):
        if i not in keep:
            print(f'  {names[i]}: constant in this window, folded into "per call"')
    x = [[r[i] for i in keep] for r in x]
    names = [names[i] for i in keep]
    b = solve(x, cpu)
    # One envelope a call makes "per other envelope" the intercept less "per ask": drop it and fit again.
    if b is None and 'per other envelope' in names:
        i = names.index('per other envelope')
        print('  per other envelope: every call carried one envelope, folded into "per call"')
        x, names = [r[:i] + r[i + 1:] for r in x], names[:i] + names[i + 1:]
        b = solve(x, cpu)
    if b is None:
        print('fit: singular')
        return
    light = [(r, y) for r, y in zip(x, cpu) if y < HEAVY_MS]
    bl = solve([r for r, _ in light], [y for _, y in light]) if light else None
    mean = [sum(r[i] for r in x) / len(x) for i in range(len(names))]
    print('least-squares CPU fit, ms (coefficient, and its share of the mean call):')
    for n, c, m in zip(names, b, mean):
        print(f'  {n:18} {c:8.4f}   {c * m:6.3f} ms a call')
    if bl:
        print(f'the same fit on calls under {HEAVY_MS} ms only:')
        for n, c, m in zip(names, bl, mean):
            print(f'  {n:18} {c:8.4f}   {c * m:6.3f} ms a call')


def selftest():
    # Rows packed as `RaftCall::row` packs them, CPU from known per-stage costs, and one voter call that must not join:
    # the fit must give the costs back.
    import os, random, subprocess, tempfile
    pack = lambda b, e, a, h, g, ans, hb, o: (b | e << 22 | a << 28 | h << 34 | g << 40, ans | hb << 23 | o << 46)
    t0, rows, wl = 1795317870000, [], []
    rnd = random.Random(1)
    for i in range(600):
        body, asks, acks, built, held = (rnd.randint(100, 4000), rnd.randint(0, 3), rnd.randint(0, 2),
                                         rnd.randint(0, 2), rnd.randint(0, 4))
        ans = held * 20000 + built * 15000 + rnd.randint(50, 500)
        ix, ms = pack(body, asks + acks, asks, held, rnd.randint(0, 40), ans, held * 20000, held + built)
        cpu = 0.2 + 0.1 * body / 1024 + 0.05 * asks + 0.4 * acks + 1.5 * built + 0.03 * ans / 1024
        rows.append([t0 + 100000 + i * 100, 33, 192 + i % 4, i, ix, ms])
        wl.append({'ts': t0 + 100000 + i * 100, 'path': '/raft', 'from': str(192 + i % 4), 'call': str(i), 'cpu': cpu})
    rows.append([t0 + 5, 33, 1, 9999, *pack(10, 1, 0, 0, 0, 10, 0, 1)])
    wl.append({'ts': t0 + 100000, 'path': '/raft', 'from': '1', 'call': '9999', 'cpu': 50})
    d = tempfile.mkdtemp()
    open(f'{d}/t.jsonl', 'w').write(json.dumps({'site': 0, 'rows': rows}) + '\n')
    json.dump(wl, open(f'{d}/w.json', 'w'))
    out = subprocess.run([sys.executable, __file__, f'{d}/t.jsonl', f'{d}/w.json',
                          datetime.fromtimestamp(t0 / 1000, timezone.utc).isoformat(), '0', '999'],
                         capture_output=True, text=True, check=True).stdout
    for f in os.listdir(d):
        os.remove(f'{d}/{f}')
    os.rmdir(d)
    want = {'per call': 0.2, 'per KB decoded': 0.1, 'per ask stepped': 0.05, 'per other envelope': 0.4,
            'per message built': 1.5, 'per held message': 0.0, 'per KB answered': 0.03}
    got = {l[:20].strip(): float(l[20:].split()[0]) for l in out.splitlines() if l.startswith('  per ')}
    assert '600 from learners' in out, out
    assert all(abs(got[k] - v) < 1e-6 for k, v in want.items()), (got, out)
    print('selftest ok')


if __name__ == '__main__':
    selftest() if sys.argv[1:] == ['--selftest'] else main()
