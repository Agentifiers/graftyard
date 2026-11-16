#!/usr/bin/env python3
"""spools.py b034warm-TAG.jsonl [BIN_S 5] -> the per-container R2 ceiling from a cold warm (LM-1 round 27b).

Reads each shard's rehydrate report (`stored.spools`: `[start, end, bytes, site]` per stored pack, from the catch-up
lines of kind "catchup" and, with POOL_PROBE, "pool") and prints, per container:
  - bytes, the spool span, and the rate per stream (each spool's bytes over its own time, p50 and bytes-weighted);
  - streams busy: the mean over the span and the most at once;
  - the total rate over time in BIN_S bins (MB/s), and its peak bin.
With a pool probe on the same site as a network shard, the two containers' overlapping window is compared: the sum of
their total rates in the bins both were busy, against each alone. A sum near twice one container's rate is a
per-container ceiling; a sum near one container's rate is a per-site or per-bucket one.
"""
import collections, json, statistics, sys

path = sys.argv[1]
bin_ms = int(float(sys.argv[2]) * 1000) if len(sys.argv) > 2 else 5000
shards = {}
for line in open(path):
    d = json.loads(line)
    if d.get("kind") not in ("catchup", "pool") or d.get("status") != 200:
        continue
    sp = ((d.get("v") or {}).get("stored") or {}).get("spools") or []
    if sp:
        name = f"site {d['site']}" + (f" pool:{d['pool']}" if d["kind"] == "pool" else "")
        shards[name] = sp


def bins(sp):
    """Bytes per bin, each spool's bytes spread evenly over its own span."""
    b = collections.Counter()
    for s, e, n, _ in sp:
        e = max(e, s + 1)
        for t in range(s - s % bin_ms, e, bin_ms):
            lo, hi = max(s, t), min(e, t + bin_ms)
            if hi > lo:
                b[t] += n * (hi - lo) / (e - s)
    return b


rates = {}
for name, sp in sorted(shards.items()):
    t0, t1 = min(s for s, *_ in sp), max(e for _, e, *_ in sp)
    total = sum(n for *_, n, _ in [(s, e, n, x) for s, e, n, x in sp])
    per = [n / max(e - s, 1) / 1e3 for s, e, n, _ in sp]
    weighted = sum(n for _, _, n, _ in sp) / sum(max(e - s, 1) for s, e, _, _ in sp) / 1e3
    edges = sorted([(s, 1) for s, *_ in sp] + [(e, -1) for _, e, *_ in sp])
    busy, most, last, area = 0, 0, t0, 0
    for t, k in edges:
        area += busy * (t - last)
        busy += k
        most = max(most, busy)
        last = t
    b = bins(sp)
    rates[name] = {t: v / bin_ms / 1e3 for t, v in b.items()}
    peak = max(rates[name].values())
    sites = collections.Counter(int(x) for *_, x in sp)
    print(f"{name}: {len(sp)} spools, {total / 1e9:.2f} GB in {(t1 - t0) / 1e3:.1f} s = {total / (t1 - t0) / 1e3:.1f} MB/s "
          f"in total; per stream p50 {statistics.median(per):.1f} MB/s, bytes-weighted {weighted:.1f} MB/s; streams "
          f"busy mean {area / (t1 - t0):.2f}, most {most}; peak {bin_ms // 1000} s bin {peak:.1f} MB/s; from site "
          f"{dict(sites)}")
    print("  MB/s by bin: " + " ".join(f"{rates[name][t]:.0f}" for t in sorted(rates[name])))

for name in [n for n in rates if " pool:" in n]:
    other = name.split(" pool:")[0]
    if other not in rates:
        continue
    both = sorted(set(rates[name]) & set(rates[other]))
    both = [t for t in both if rates[name][t] > 1 and rates[other][t] > 1]
    if not both:
        print(f"{name} and {other}: no bins where both spooled")
        continue
    a = statistics.mean(rates[other][t] for t in both)
    p = statistics.mean(rates[name][t] for t in both)
    alone = [rates[other][t] for t in rates[other] if t not in both and rates[other][t] > 1]
    print(f"{name} beside {other}, {len(both)} bins both busy: {other} {a:.1f} MB/s + {name} {p:.1f} MB/s = "
          f"{a + p:.1f} MB/s; {other} alone {statistics.mean(alone) if alone else float('nan'):.1f} MB/s")
