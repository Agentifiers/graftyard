"""Counterfactual for b034-colo: replay each row's j9 hits in start order through a colo-shared gate decision.

One key (Load34 sends one PAT to one repository) and one colo (every Load34 client runs from the orb's region). A hit
whose gate stage was 0 used its isolate's decision and stays 0. A hit whose gate read members finds a colo entry if one
was put (at read start + read wall) and is under READ_CACHE from its read start; it then pays MATCH ms. Otherwise it
reads as measured and, from the end of its read, the colo holds an entry aging from its start. Hits that start while
a read is in flight and no entry is up read too (no cross-isolate batching). chain_open is capped as named.
"Re-read ahead": a hit served from an entry, or from its isolate, whose newest entry is at least 5 s old starts one
background read (its own gate wall, or 600 ms for an isolate hit), at most one in flight.
Usage: python3 tests/colo/replay.py tests/am1/raw/a1-samples-b0-34-<run>.jsonl.gz [...]
"""
import collections, gzip, json, sys

READ_CACHE = 10_000


def pct(a, p):
    a = sorted(a)
    return a[min(len(a) - 1, int(p / 100 * len(a)))] if a else None


def hits(path):
    g = collections.defaultdict(dict)
    for l in gzip.open(path, "rt"):
        t, k, ms, v = json.loads(l)
        if "@" not in k or not k.startswith("read:cloneTtfb"):
            continue
        kind, ph = k.split("@")
        g[(t + ms, ph)][kind] = ms
        if kind == "read:cloneTtfb:hit":
            g[(t + ms, ph)]["_t"] = t
    h = sorted((d["_t"], d) for (e, p), d in g.items() if p == "j9" and "read:cloneTtfb:hit:gate" in d and "_t" in d)
    return h


def run(h, match, co_cap, drop_ms=1500, ahead=None):
    """`ahead`: a hit that matches an entry at least this old starts one background re-read (its own measured gate
    wall), whose put replaces the entry when it ends; at most one in flight."""
    t0 = h[0][0]
    entries = []  # [(up_at, read_start)]
    out, reads, colo = [], 0, 0
    pending = None
    for t, d in h:
        T, g = d["read:cloneTtfb:hit"], d["read:cloneTtfb:hit:gate"]
        co = d.get("read:cloneTtfb:hit:chain_open", 0)
        g2 = g
        if g == 0 and ahead is not None and entries:
            last = max(e[1] for e in entries)
            if t - last >= ahead and not (pending and pending > t):
                reads += 1
                entries.append((t + 600, t))  # a background read of about gate_rows' p50
                pending = t + 600
        if g > 0:
            up = [e for e in entries if e[0] <= t and t - e[1] < READ_CACHE]
            best = max(up, key=lambda e: e[1]) if up else None
            if best:
                g2, colo = match, colo + 1
                if ahead is not None and t - best[1] >= ahead and not (pending and pending > t):
                    reads += 1
                    entries.append((t + g, t))
                    pending = t + g
            else:
                reads += 1
                entries.append((t + g, t))
        co2 = min(co, co_cap)
        v = T - (max(g, co) - max(g2, co2))
        if t - t0 > drop_ms:
            out.append(v)
    return out, reads, colo


for path in sys.argv[1:]:
    h = hits(path)
    gated = sum(1 for _, d in h if d["read:cloneTtfb:hit:gate"] > 0)
    print(path.split("-")[-1].split(".")[0], "j9 hits", len(h), "gate read members", gated)
    for name, match, cap, *ahead in (
        ("measured", None, 1e9),
        ("colo gate (12 ms), chain_open as measured", 12, 1e9),
        ("colo gate (12 ms), chain_open <= 300", 12, 300),
        ("colo gate (12 ms), chain_open <= 150", 12, 150),
        ("colo gate (25 ms), chain_open <= 150", 25, 150),
        ("colo + re-read ahead at 5 s, chain_open as measured", 12, 1e9, 5000),
        ("colo + re-read ahead at 5 s, chain_open <= 300", 12, 300, 5000),
        ("colo + re-read ahead at 5 s, chain_open <= 150", 12, 150, 5000),
        ("gate hidden (note's bound), chain_open <= 300", -1, 300),
    ):
        if match == -1:
            t0 = h[0][0]
            v = [d["read:cloneTtfb:hit"] - (max(d["read:cloneTtfb:hit:gate"], d.get("read:cloneTtfb:hit:chain_open", 0)) - min(d.get("read:cloneTtfb:hit:chain_open", 0), cap)) for t, d in h if t - t0 > 1500]
            print(f"  {name:52s} p99 {pct(v, 99):>5} over 1 s {sum(x > 1000 for x in v):>3} of {len(v)}")
            continue
        if match is None:
            t0 = h[0][0]
            v = [d["read:cloneTtfb:hit"] for t, d in h if t - t0 > 1500]
            print(f"  {name:52s} p99 {pct(v, 99):>5} over 1 s {sum(x > 1000 for x in v):>3} of {len(v)}")
            continue
        v, reads, colo = run(h, match, cap, ahead=ahead[0] if ahead else None)
        print(f"  {name:52s} p99 {pct(v, 99):>5} over 1 s {sum(x > 1000 for x in v):>3} of {len(v)}"
              f"  (member reads {reads}, colo hits {colo})")
