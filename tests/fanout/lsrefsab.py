#!/usr/bin/env python3
"""lsrefsab.py TSAVE [EVERY_S]: the leader's storage confirm (trace kind 10) for B0-29 and B0-33 reports, from a
row's tsave (tests/nr3/tsave.mjs, LEARNERS=1).

- **Setup baseline:** the leader's confirm p50 from 60 s after its first row until the first 30 s in which learners
  took more than 30 `/read`/s, so before load. Each row is a new network on new Durable Objects, and B0-29's A-B-A
  (tests/am1/results/b029-m2b.md) showed this baseline moves with placement (39 to 70 ms on one build): report it beside
  the load numbers so a load confirm is read against its own network's.
- **Load:** the confirm p50 and p99 over the 30 s blocks with that learner load.
- **With EVERY_S** (a1.mjs `--ls-refs-shard-every`): each load window's arm from Cloudflare's clock, as a1.mjs decides
  it (floor(ms / (EVERY_S * 1000)) odd: ls-refs to the shard; even: at the edge), the learners' `ls-refs` and `refs`
  reads (`ReadQuery` 25 and 23) in it, and the leader's confirm p50. Each odd window is paired with the even window
  before it, and the line gives the median of the paired differences (edge minus shard) and how many pairs favour each.
  The first and last partial windows of the load are dropped.
- **Isolates** (tsave ISOLATES=1): which members reported the same `/isolate` id.
"""
import collections
import datetime
import json
import lzma
import statistics
import sys

READS_LOAD = 30  # learner /read per second that marks the load
LS_CODES = (23, 25)  # READ_QUERIES: refs, lsRefs


def pct(xs, q):
    return sorted(xs)[min(len(xs) - 1, int(q * len(xs)))] if xs else None


def load(path):
    op = lzma.open if path.endswith(".xz") else open
    voters, learners, leader, isolates = collections.defaultdict(set), collections.defaultdict(set), {}, []
    for line in op(path, "rt"):
        d = json.loads(line)
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


def main(path, every_s=None):
    voters, learners, leader, isolates = load(path)
    lead = max(voters, key=lambda s: sum(1 for x in voters[s] if x[1] == 16 and leader.get(s) == s), default=None)
    if lead is None:
        sys.exit("no voter rows")
    conf = sorted((x[0], x[5]) for x in voters[lead] if x[1] == 10)
    reads = collections.Counter(x[0] // 30000 for v in learners.values() for x in v if x[1] == 25 and x[4] % 64 == 2)
    hot = sorted(b for b, n in reads.items() if n > READS_LOAD * 30)
    t_first = conf[0][0] if conf else 0
    if not hot:
        print(f"leader site {lead}: no load window (learner /read never above {READS_LOAD}/s)")
        return
    a, b = hot[0] * 30000, (hot[-1] + 1) * 30000
    setup = [ms for t, ms in conf if t_first + 60000 <= t < a]
    under = [ms for t, ms in conf if a + 30000 <= t < b - 30000]
    print(f"leader site {lead}: setup confirm p50 {pct(setup, .5)} (n {len(setup)}, before load) | "
          f"load confirm p50 {pct(under, .5)} p99 {pct(under, .99)} (n {len(under)})")
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
