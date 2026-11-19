#!/usr/bin/env python3
"""lsrefsab.py TSAVE [EVERY_S]: the leader's storage confirm (trace kind 10) for B0-29 and B0-33 reports, from a
row's tsave (tests/nr3/tsave.mjs, LEARNERS=1).

- **Setup baseline:** the leader's confirms from 60 s after its first row until the first 30 s in which learners
  took more than 30 `/read`/s, so before load, split as `confirmstmts.py` splits them. A confirm whose start (`at` -
  `ms`) has a `Flushed` row within PAIR_MS before it ran that deferred flush of `index` statements; one with none ran
  in the step itself (`unpaired`). The baseline is the **1-statement** p50. Setup confirms are two near-equal groups,
  1-statement flushes (p50 23 to 27) and unpaired (51 to 59), and the pooled median flips between them on one sample
  (tests/am1/results/b033-m54.md, "4. Setup baseline"). Unpaired and pooled are printed beside it. Each row is a new
  network on new Durable Objects, so a load confirm is read against its own network's.
- **Load:** the confirm p50 and p99 over the 30 s blocks with that learner load.
- **With EVERY_S** (a1.mjs `--ls-refs-shard-every`): each load window's arm from Cloudflare's clock, as a1.mjs decides
  it (floor(ms / (EVERY_S * 1000)) odd: ls-refs to the shard; even: at the edge), the learners' `ls-refs` and `refs`
  reads (`ReadQuery` 25 and 23) in it, and the leader's confirm p50. Each odd window is paired with the even window
  before it, and the line gives the median of the paired differences (edge minus shard) and how many pairs favour each.
  The first and last partial windows of the load are dropped.
- **Isolates** (tsave ISOLATES=1): which members reported the same `/isolate` id.
"""
import bisect
import collections
import datetime
import json
import lzma
import statistics
import sys

READS_LOAD = 30  # learner /read per second that marks the load
PAIR_MS = 5  # confirmstmts.py's pairing of a confirm's start with its `Flushed` row
LS_CODES = (23, 25)  # READ_QUERIES: refs, lsRefs


def pct(xs, q):
    return sorted(xs)[min(len(xs) - 1, int(q * len(xs)))] if xs else None


def load(path):
    op = lzma.open if path.endswith(".xz") else open
    voters, learners, leader, isolates = collections.defaultdict(set), collections.defaultdict(set), {}, []
    with op(path, "rt") as f:
        lines = [json.loads(line) for line in f]
    for d in lines:
        if "isolate" in d:
            isolates.append(d)
        if "rows" not in d:
            continue
        rows = (tuple(x) for x in d["rows"])
        if d.get("learner") is not None:
            learners[d["learner"]].update(rows)
        elif d.get("site") is not None:
            voters[d["site"]].update(rows)
            leader[d["site"]] = d.get("leader")
    return voters, learners, leader, isolates


def kinds(rows):
    """Each confirm in `rows` as (at, ms, kind): "1" for a 1-statement deferred flush, "2+" for a larger one, and
    "unpaired" when no `Flushed` row lies within PAIR_MS before the confirm's start."""
    fl = sorted((x[0], x[4]) for x in rows if x[1] == 16)
    ft = [t for t, _ in fl]
    out = []
    for x in sorted(r for r in rows if r[1] == 10):
        i = bisect.bisect_right(ft, x[0] - x[5]) - 1
        n = fl[i][1] if i >= 0 and x[0] - x[5] - ft[i] <= PAIR_MS else None
        out.append((x[0], x[5], "unpaired" if n is None else ("1" if n <= 1 else "2+")))
    return out


def main(path, every_s=None):
    voters, learners, leader, isolates = load(path)
    lead = max(voters, key=lambda s: sum(1 for x in voters[s] if x[1] == 16 and leader.get(s) == s), default=None)
    if lead is None:
        sys.exit("no voter rows")
    typed = kinds(voters[lead])
    conf = [(t, ms) for t, ms, _ in typed]
    reads = collections.Counter(x[0] // 30000 for v in learners.values() for x in v if x[1] == 25 and x[4] % 64 == 2)
    hot = sorted(b for b, n in reads.items() if n > READS_LOAD * 30)
    t_first = conf[0][0] if conf else 0
    if not hot:
        print(f"leader site {lead}: no load window (learner /read never above {READS_LOAD}/s)")
        return
    a, b = hot[0] * 30000, (hot[-1] + 1) * 30000
    setup = [(ms, k) for t, ms, k in typed if t_first + 60000 <= t < a]
    under = [(ms, k) for t, ms, k in typed if a + 30000 <= t < b - 30000]
    of = lambda xs, k: [ms for ms, kk in xs if k in (kk, "all")]
    p50n = lambda xs: f"p50 {pct(xs, .5)} (n {len(xs)})"
    print(f"leader site {lead}: setup 1-statement confirm {p50n(of(setup, '1'))} (placement baseline); "
          f"unpaired {p50n(of(setup, 'unpaired'))}; pooled {p50n(of(setup, 'all'))}")
    print(f"  load: 1-statement {p50n(of(under, '1'))}; unpaired {p50n(of(under, 'unpaired'))}; "
          f"pooled p50 {pct(of(under, 'all'), .5)} p99 {pct(of(under, 'all'), .99)} (n {len(under)})")
    if every_s:
        w_ms = int(every_s * 1000)
        ls = collections.Counter(x[0] // w_ms for v in learners.values() for x in v if x[1] == 26 and x[4] in LS_CODES)
        by = collections.defaultdict(list)
        for t, ms in conf:
            if a <= t < b:
                by[t // w_ms].append(ms)
        ws = sorted(by)[1:-1]
        pairs = []
        for w in ws:
            arm = "shard" if w % 2 else "edge"
            at = datetime.datetime.fromtimestamp(w * w_ms / 1000, datetime.timezone.utc).strftime("%H:%M:%S")
            print(f"  window {at}: {arm:5} learner ls-refs+refs {ls[w] / every_s:5.1f}/s, "
                  f"leader confirm p50 {pct(by[w], .5)} p99 {pct(by[w], .99)} (n {len(by[w])})")
            if w % 2 and w - 1 in ws:
                pairs.append(statistics.median(by[w - 1]) - statistics.median(by[w]))
        if pairs:
            print(f"paired edge minus shard confirm p50: median {statistics.median(pairs):+.1f} ms over {len(pairs)} pairs; "
                  f"edge slower in {sum(p > 0 for p in pairs)}, shard slower in {sum(p < 0 for p in pairs)}")
    if isolates:
        last = {}
        for d in isolates:
            last[f"v{d['site']}" if "site" in d else f"l{d['learner']}"] = d["isolate"]
        shared = collections.defaultdict(list)
        for m, iso in last.items():
            shared[iso].append(m)
        multi = {i: ms for i, ms in shared.items() if len(ms) > 1}
        changes = len(isolates) - len(last)
        print(f"isolates: {len(last)} members, {len(shared)} distinct ids, {changes} changes; shared: "
              + (", ".join("+".join(sorted(ms)) for ms in multi.values()) or "none"))


if __name__ == "__main__":
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    main(sys.argv[1], float(sys.argv[2]) if len(sys.argv) > 2 else None)
