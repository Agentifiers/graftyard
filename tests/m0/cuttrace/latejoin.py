import bisect, sys, random, os
sys.path.insert(0, os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', '..', 'fanout')))
from tracerows import load
# freezesim.py's replay of B0-29's leader (the same loader: measured /commit arrivals, rounds append->apply in order,
# read-plus-decide samples of cuts taken at an apply), for `b029-tail`'s option (b): a command admitted after a batch
# froze joins that batch when its own dependency read, asked at its admission, answers before the batch's append. The
# append stays max(apply of the entry in flight, freeze + the batch's read), so a joiner never delays it.
# Usage: latejoin.py TSAVE LOADSTART_MS [WINDOW_MS]
path, t0 = sys.argv[1], int(sys.argv[2]); t1 = t0 + (int(sys.argv[3]) if len(sys.argv) > 3 else 600000)
app = {}; aply = []; ct = []; arr = []
for r in load(path, keep=lambda r: t0 - 5000 <= r[0] < t1 + 5000).get(0, []):
    if r[1] == 15: app.setdefault(r[4], r[0])
    if r[1] == 4: aply.append((r[0], r[4]))
    if r[1] == 29: ct.append((r[0], r[2] >= 64))
    if r[1] == 25 and r[4] % 64 == 3 and t0 <= r[0] < t1: arr.append(r[0])
aply.sort(); applied_at = {}; prev = 0
for at, idx in aply:
    for i in range(prev + 1, idx + 1): applied_at.setdefault(i, at)
    prev = max(prev, idx)
ct.sort(); cts = [c[0] for c in ct]; arr.sort()
idx = sorted(i for i in app if i in applied_at and t0 <= app[i] < t1)
rounds = [applied_at[i] - app[i] for i in idx]
D = []
for a, b in zip(idx, idx[1:]):
    k = bisect.bisect_right(cts, app[b]) - 1
    if k >= 0 and not ct[k][1]: D.append(app[b] - cts[k])
def pc(xs, q): xs = sorted(xs); return xs[min(len(xs) - 1, int(len(xs) * q))]

def sim(join, seed=1):
    # join: None (main), "own" (each joiner's own read, asked at its admission: an upper bound, one read per joiner),
    # or ("reread", x) (one second read for the whole batch, asked x ms after its freeze, taking every command queued
    # then; used only if it answers by the append the first read already allowed, so it never delays the append).
    rnd = random.Random(seed); lat = []; joined = 0
    i = 0; t = arr[0]; R = 150.0; Dm = 125.0; k = 0; pending = None
    while i < len(arr):
        d = rnd.choice(D)
        if pending is None:
            cut = max(t, arr[i]); j = bisect.bisect_right(arr, cut); batch = list(range(i, j)); i = j
            ap = cut + d
        else:
            ap0, pl = pending; f = ap0 + max(0, R - Dm)
            if f >= pl or bisect.bisect_right(arr, f) <= i:
                pending = None; t = pl; continue
            j = bisect.bisect_right(arr, f); batch = list(range(i, j)); i = j
            ap = max(pl, f + d)
            if isinstance(join, tuple):
                f2 = f + join[1]; d2 = rnd.choice(D)
                if f2 + d2 <= ap:
                    j2 = bisect.bisect_right(arr, f2); joined += j2 - i; batch += range(i, j2); i = j2
        while join == "own" and i < len(arr) and arr[i] < ap and arr[i] + rnd.choice(D) <= ap:
            batch.append(i); i += 1; joined += 1
        r = rounds[k % len(rounds)]; k += 1; pl = ap + r
        for x in batch: lat.append(pl - arr[x])
        R = 0.9 * R + 0.1 * r; Dm = 0.9 * Dm + 0.1 * d; pending = (ap, pl)
    return pc(lat, .5), pc(lat, .99), joined / len(lat)

print(f"arrivals {len(arr)}, entries {len(rounds)}, read samples {len(D)} (p50 {pc(D, .5)} p99 {pc(D, .99)}), "
      f"round p50 {pc(rounds, .5)} p99 {pc(rounds, .99)}")
for name, j in (("main's freeze, no join", None), ("late join, each joiner's own read (upper bound)", "own"),
                *((f"late join, one re-read {x} ms after the freeze", ("reread", x)) for x in (25, 50, 100))):
    out = [sim(j, s) for s in (1, 2, 3)]
    print(f"{name}: commit p50 {sum(o[0] for o in out) // 3} p99 {sum(o[1] for o in out) // 3}, "
          f"joined {sum(o[2] for o in out) / 3:.0%}")
