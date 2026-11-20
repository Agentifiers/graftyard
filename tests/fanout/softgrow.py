import bisect, json, lzma, sys
# softgrow.py TRACE.jsonl[.xz] LOAD_START_MS SECONDS [SITE]: replays a row's leader queue through the flat cut
# (ENTRY_SOFT) and soft-grow's (ENTRY_SOFT_DEEP while more than DEEP_QUEUE * ENTRY_SOFT waits), from the leader's trace.
#   Arrivals: each cut (`Cut`, kind 29) took `taken` and left `left` Human commands, so cumulative arrivals at the cut
#   are taken-so-far + left; commands arrive in FIFO order between cuts at an even rate, each with the mean soft bytes
#   of the cut that took it.
#   Cycle: the row's own append-to-append time at that moment (the cut before each `AppendQueued`, kind 12, to the
#   next append), moved by a least-squares fit of cycle against entry bytes over the row, so a bigger entry pays what
#   the row's bigger entries paid and a slow minute stays slow.
# Prints the fit, then for each rule: entries, commands committed, wait (arrival to its entry's commit, taken as the
# end of its cycle) p50/p99, the queue's peak, and how long after the peak it was back under one entry.
# The replay holds offered load as traced: it does not credit a lighter leader [inference].
ENTRY_SOFT, ENTRY_SOFT_DEEP, DEEP_QUEUE = 100_000, 150_000, 2
path, t0 = sys.argv[1], int(sys.argv[2])
t1 = t0 + 1000 * int(sys.argv[3])
SITE = int(sys.argv[4]) if len(sys.argv) > 4 else 0
seen, cuts, appends = set(), [], {}
for l in (lzma.open if path.endswith('.xz') else open)(path, 'rt'):
    d = json.loads(l)
    if 'rows' not in d or 'learner' in d or d.get('site') != SITE:
        continue
    for r in d['rows']:
        k = tuple(r)
        if k in seen or not t0 - 30_000 <= r[0] < t1 + 30_000:
            continue
        seen.add(k)
        at, kind, peer = r[0], r[1], r[2]
        b, i, ms = int(float(r[3])), int(float(r[4])), int(float(r[5]))
        if kind == 29:
            cuts.append((at, b & 0xfff, i & 0x1fff, ms & 0x1fffff))
        elif kind == 12 and peer < 192:
            appends.setdefault(i, at)
cuts.sort()
cuts = [c for c in cuts if t0 <= c[0] < t1]
ap = sorted(appends.values())
# Fit cycle(bytes): for each append, the cut just before it, and the next append.
ct = [c[0] for c in cuts]
xs, ys, obs = [], [], []
for a, n in zip(ap, ap[1:]):
    j = bisect.bisect_right(ct, a) - 1
    if j >= 0 and t0 <= a < t1 and n - a < 5_000:
        xs.append(cuts[j][3]), ys.append(n - a), obs.append((a, n - a, cuts[j][3]))
obs_t = [o[0] for o in obs]
mx, my = sum(xs) / len(xs), sum(ys) / len(ys)
slope = sum((x - mx) * (y - my) for x, y in zip(xs, ys)) / sum((x - mx) ** 2 for x in xs)
base = my - slope * mx
print(f'cycle fit over {len(xs)} entries: {base:.0f} ms + {slope * 1000:.2f} ms per KB '
      f'(at 100 KB {base + slope * 1e5:.0f} ms, at 150 KB {base + slope * 1.5e5:.0f} ms)')
# Arrivals: times from the cuts' running counts; sizes in FIFO order, each command the mean of the cut that took it.
sizes = [soft / taken for at, taken, left, soft in cuts for _ in range(taken)]
times = []
prev_t, prev_cum, taken_cum = t0, 0, 0
for at, taken, left, soft in cuts:
    cum = taken_cum + taken + left
    n = cum - prev_cum
    times += [prev_t + (at - prev_t) * (m + 1) / n for m in range(max(0, n))]
    prev_t, prev_cum, taken_cum = at, max(cum, prev_cum), taken_cum + taken
arr = list(zip(times, sizes))


def replay(grow):
    t, q, done, waits, peak, peak_at, drained, entries = arr[0][0], [], 0, [], 0, 0, None, 0
    k = 0
    end = arr[-1][0] + 600_000
    while (k < len(arr) or q) and t < end:
        while k < len(arr) and arr[k][0] <= t:
            q.append(arr[k])
            k += 1
        if not q:
            t = arr[k][0]
            continue
        queued = sum(b for _, b in q)
        bound = ENTRY_SOFT_DEEP if grow and queued > DEEP_QUEUE * ENTRY_SOFT else ENTRY_SOFT
        batch, nb = [], 0
        while q and (not batch or nb + q[0][1] <= bound):
            nb += q[0][1]
            batch.append(q.pop(0))
        # The cycle the row had at this moment, moved by the fitted cost of this entry's bytes against that one's.
        o = obs[min(len(obs) - 1, max(0, bisect.bisect_right(obs_t, t) - 1))]
        cyc = max(50, o[1] + slope * (nb - o[2]))
        t += cyc
        entries += 1
        waits += [t - a for a, _ in batch]
        done += len(batch)
        if len(q) > peak:
            peak, peak_at, drained = len(q), t, None
        if drained is None and sum(b for _, b in q) <= ENTRY_SOFT:
            drained = t
    waits.sort()
    pc = lambda p: waits[min(len(waits) - 1, int(len(waits) * p))]
    return entries, done, pc(.5), pc(.99), peak, (peak_at - t0) / 1000, ((drained or t) - peak_at) / 1000


offered = sum(b for _, b in arr) / ((arr[-1][0] - arr[0][0]) / 1000)
print(f'{len(cuts)} cuts, {len(arr)} commands arrived, {offered / 1000:.0f} KB/s offered')
for name, grow in (('flat ENTRY_SOFT', False), ('soft-grow', True)):
    e, n, p50, p99, peak, at, drain = replay(grow)
    print(f'{name}: {e} entries, {n} committed, wait p50 {p50 / 1000:.1f} s p99 {p99 / 1000:.1f} s, peak queue {peak} '
          f'commands at {at:.0f} s, back under one entry {drain:.1f} s after the peak')
