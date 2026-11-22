"""B0-29 (m3fb29 and its two reruns) on the near follower's clock: cycle, leader excess per cycle
(follower entry of k+1 - its answer to k - idle wire RT), by commands in the entry; leader clock staleness."""
import sys, bisect, collections, json, lzma, os
RAW = os.environ.get('B033_RAW', 'tests/fanout/raw')
def raw_path(tag): return os.path.join(RAW, f'tsave-{tag}.jsonl.xz')
I = lambda x: int(float(x))
def pc(xs, q):
    xs = sorted(xs); return round(xs[min(len(xs) - 1, int(q * len(xs)))]) if xs else None
for tag in sys.argv[1:]:
    rows, seen = collections.defaultdict(list), set()
    for line in lzma.open(raw_path(tag), 'rt'):
        d = json.loads(line)
        if 'rows' not in d or 'learner' in d or d.get('site') not in (0, 1, 2): continue
        for r in d['rows']:
            k = (d['site'],) + tuple(r)
            if k not in seen: seen.add(k); rows[d['site']].append(r)
    # leader = site with most AppendQueued-with-entries rows
    ldr = max(rows, key=lambda s: sum(1 for r in rows[s] if r[1] == 12))
    L = sorted(rows[ldr], key=lambda r: r[0])
    f = min(s for s in rows if s != ldr)
    q12 = [r[0] for r in L if r[1] == 12 and r[2] == f]
    t0, t1 = q12[len(q12) // 20], q12[-len(q12) // 20]
    ent = {I(r[3]): r[0] for r in rows[f] if r[1] == 25 and I(r[4]) % 64 == 1 and r[2] == ldr}
    left = {I(r[3]): r[0] for r in rows[f] if r[1] == 24 and r[2] == ldr}
    ans = {I(r[3]): r[0] for r in L if r[1] == 21 and r[2] == f}
    sent = {I(r[3]): r[0] for r in L if r[1] == 23 and r[2] == f}
    rts = sorted(ans[c] - t for c, t in sent.items() if c in ans)
    rt = rts[len(rts) // 20]   # no idle window kept: the 5th percentile round trip stands in for the wire
    nc = [0]; ne = [0]
    for r in L:
        nc.append(nc[-1] + (sum((I(r[3]) >> (12 * i)) & 0xfff for i in range(4)) if r[1] == 29 else 0)); ne.append(ne[-1] + (r[1] == 25))
    sn, E = set(), []
    for i, r in enumerate(L):
        if r[1] == 12 and r[2] == f and I(r[4]) not in sn:
            sn.add(I(r[4]))
            for j in range(i + 1, min(len(L), i + 400)):
                if L[j][1] == 23 and L[j][2] == f: E.append((I(L[j][3]), i, I(r[5]) / 1000)); break
    V = []
    for n in range(len(E) - 1):
        (c, p, _), (c2, p2, kb) = E[n], E[n + 1]
        if c in left and c2 in ent and c in ent and t0 <= L[p][0] < t1:
            cyc = ent[c2] - ent[c]
            if 0 < cyc < 6000: V.append((cyc, ent[c2] - left[c] - rt, nc[p2] - nc[p], kb, left[c] - ent[c]))
    st = [ent[c] - t for c, t in sent.items() if c in ent and t0 <= t < t1]; base = pc(st, .02)
    T = (t1 - t0) / 1000
    print(f'{tag}: leader site {ldr}, follower {f}, window {T:.0f} s, leader inputs/s {(sum(1 for r in L if r[1] == 25 and t0 <= r[0] < t1)) / T:.0f}, entries/s {len(V) / T:.2f}, wire RT (p5) {rt}')
    print(f'   cycle p50/p90/p99 {pc([v[0] for v in V], .5)}/{pc([v[0] for v in V], .9)}/{pc([v[0] for v in V], .99)} | leader excess p50/p90/p99 {pc([v[1] for v in V], .5)}/{pc([v[1] for v in V], .9)}/{pc([v[1] for v in V], .99)} | follower handle p50/p90 {pc([v[4] for v in V], .5)}/{pc([v[4] for v in V], .9)} | leader clock behind (vs its 2nd percentile) p50/p90/p99 {pc(st, .5) - base}/{pc(st, .9) - base}/{pc(st, .99) - base}')
    for lo, hi in ((0, 1), (1, 3), (3, 6), (6, 12), (12, 999)):
        w = [v for v in V if lo <= v[2] < hi]
        if len(w) >= 5: print(f'   cmds {lo:2}-{hi - 1:3}: n {len(w):4} | KB p50 {pc([v[3] for v in w], .5):4} | excess p50/p90/p99 {pc([v[1] for v in w], .5)}/{pc([v[1] for v in w], .9)}/{pc([v[1] for v in w], .99)}')
    for lo, hi in ((0, 30), (30, 100), (100, 9999)):
        w = [v for v in V if lo <= v[3] < hi]
        if len(w) >= 5: print(f'   entry {lo:3}-{hi:4} KB: n {len(w):4} | excess p50/p90/p99 {pc([v[1] for v in w], .5)}/{pc([v[1] for v in w], .9)}/{pc([v[1] for v in w], .99)}')
