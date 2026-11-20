#!/usr/bin/env python3
"""slotdeps.py TRACE.jsonl[.xz] DEPS_LOG.json LOAD_START_MS SECONDS [LEADER_SITE 0] -> what a repository leader's
`queue:slot` waited on in one window (B0-31 m64r31's first half; `tests/m4/results/m4-wp4.md`, "B0-31 m64r31").

- Cycles. Each leader `Events` append (`AppendQueued`, kind 12, to a voter) to the next, split at the apply of the first
  (`Applied`, kind 4): `round`, append to apply, and `gap`, apply to the next append. With `read-ahead` the next batch's
  dependency reads overlap the round in flight, so the gap is where they outlast it.
- Reads. DEPS_LOG is the leader's test-faults `/deps_log` (each entry's per-chain reads, keyed by the entry's index and
  append time). Each cycle is joined to the reads of the entry that ended it, so a long gap names its read.
- Slot model. Commands are spread evenly between cuts (`Cut`, kind 29); each rides the entry `held` appends after its
  cut, and waits from admission to the apply before that entry. Run as measured, then without the commands admitted
  in the cycles whose install `Authz` read (`authz:1`) took over 1 s. It is a model, not the host's sample: it
  reproduces the shape, not the measured p99.
"""
import bisect, json, lzma, random, sys

path, deps_path, t0, secs = sys.argv[1], sys.argv[2], int(sys.argv[3]), int(sys.argv[4])
lead = int(sys.argv[5]) if len(sys.argv) > 5 else 0
t1 = t0 + 1000 * secs
seen, rows = set(), {4: [], 12: [], 29: []}
for line in (lzma.open if path.endswith(".xz") else open)(path, "rt"):
    d = json.loads(line)
    if "rows" not in d or "learner" in d or d.get("site") != lead:
        continue
    for r in d["rows"]:
        k = tuple(r)
        if k in seen or int(r[1]) not in rows or not t0 - 10_000 <= r[0] < t1 + 10_000:
            continue
        seen.add(k)
        rows[int(r[1])].append([int(float(x)) for x in r])
apps = sorted({r[0] for r in rows[12] if r[2] < 192})
applies = sorted(r[0] for r in rows[4])
deps = {d["at"]: {x["chain"]: x.get("ms", 0) for x in d["reads"]} for d in json.load(open(deps_path))}


def pct(xs, p):
    xs = sorted(xs)
    return round(xs[min(len(xs) - 1, int(p * len(xs)))]) if xs else None


# Each append's last index, and the first time the leader applied it or past it (an apply in the same step as the next
# append frees that append's slot, so the apply is matched by index, not by time).
last = {}
for r in sorted(rows[12]):
    if r[2] < 192:
        last.setdefault(r[0], r[4])
applied_at = {}
for r in sorted(rows[4]):
    applied_at.setdefault(r[4], r[0])
cycles = []
for a, b in zip(apps, apps[1:]):
    if not t0 <= a < t1:
        continue
    ap = min((t for i, t in applied_at.items() if i >= last[a] and t >= a), default=None)
    if ap is not None and ap <= b:
        cycles.append((a, ap - a, b - ap, b - a, deps.get(b, {})))
print(f"cycles {len(cycles)}: round p50 {pct([c[1] for c in cycles], .5)} p99 {pct([c[1] for c in cycles], .99)}; "
      f"gap p50 {pct([c[2] for c in cycles], .5)} p99 {pct([c[2] for c in cycles], .99)} max {max(c[2] for c in cycles)}; "
      f"cycle p99 {pct([c[3] for c in cycles], .99)}")
print(f"wall {sum(c[3] for c in cycles)} ms: round {sum(c[1] for c in cycles)}, gap {sum(c[2] for c in cycles)}; "
      f"cycles with a gap over 20 ms {sum(1 for c in cycles if c[2] > 20)}")
for c in sorted(cycles, key=lambda c: -c[3])[:6]:
    slow = max(c[4].items(), key=lambda kv: kv[1], default=("?", 0))
    print(f"  slow cycle round {c[1]} gap {c[2]}: slowest read {slow[0]} {slow[1]} ms")
reads = [r for at, r in deps.items() if t0 <= at < t1]
by = {}
for r in reads:
    for chain, ms in r.items():
        by.setdefault("authz:1" if chain == "authz:1" else chain.split(":")[0], []).append(ms)
slowest = {}
for r in reads:
    chain = max(r, key=r.get)
    k = "authz:1" if chain == "authz:1" else chain.split(":")[0]
    slowest[k] = slowest.get(k, 0) + 1
print(f"entries with reads {len(reads)}; slowest read per entry by chain {slowest}")
for k, v in sorted(by.items()):
    print(f"  {k}: n {len(v)} p50 {pct(v, .5)} p90 {pct(v, .9)} p99 {pct(v, .99)} max {max(v)}, over 1 s {sum(1 for x in v if x > 1000)}")

rnd = random.Random(1)
samples, prev = [], t0 - 10_000
for c in sorted(rows[29]):
    taken = sum((c[3] >> (12 * i)) & 0xFFF for i in range(4))
    k = bisect.bisect_left(apps, c[0]) + ((c[2] >> 4) & 3)
    if k >= len(apps):
        break
    j = bisect.bisect_right(applies, apps[k]) - 1
    last = applies[j] if j >= 0 else apps[k]
    if t0 <= c[0] < t1:
        for _ in range(taken):
            adm = rnd.uniform(prev, c[0])
            samples.append((adm, max(0, last - adm), apps[k] - adm))
    prev = c[0]
stalls = [at for at, r in deps.items() if t0 <= at < t1 and r.get("authz:1", 0) > 1000]
wins = [(apps[bisect.bisect_left(apps, at) - 1] - 3000, at) for at in stalls]
keep = [s for s in samples if not any(a <= s[0] < b for a, b in wins)]
print(f"slot model: {len(samples)} commands, slot p50 {pct([s[1] for s in samples], .5)} p99 "
      f"{pct([s[1] for s in samples], .99)}, queue p99 {pct([s[2] for s in samples], .99)}")
print(f"  without the {len(stalls)} entries whose authz:1 read took over 1 s ({len(samples) - len(keep)} commands): "
      f"slot p99 {pct([s[1] for s in keep], .99)}, queue p99 {pct([s[2] for s in keep], .99)}")
