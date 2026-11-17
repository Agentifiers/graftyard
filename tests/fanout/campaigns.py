#!/usr/bin/env python3
"""campaigns.py TSAVE.jsonl[.xz] -> why each campaign started (colo-unknown's check, LM-1 round 39).

`Trace::Reported` (28) rows are [at, 28, about, term, colo, verdict]; a `started` verdict is a campaign a `/suspect`
report began, which happens only once the member's promise has lapsed (election.rs `on_suspect`). A promise lapses
ELECTION_AFTER (10 s) after the last append that renewed it, and an append renews nothing while `suspect` withholds:
reports from two distinct colos within SUSPECT's 10 s, about the leader the member promised, in its own term, stop
renewals for 20 s. For each `started` row this prints, at that member:
  - gap: ms since the last AppendEntries it stepped (`AppendStepped`, 17) before the campaign: >= 10 s means its
    leader went quiet, and the promise lapsed by itself;
  - withheld: the first moment in the 30 s before the campaign when countable reports from two distinct known colos
    (UNKNOWN skipped, as colo-unknown counts) fell within 10 s, with those colos. A report counts when its term equals
    the member's term then and its `about` is the member's leader then, both read from the member's own `promised`
    and `leader` verdict rows and from its appends (the leader sending them);
  - neither: a lapse this trace does not explain.
  python3 tests/fanout/campaigns.py tests/fanout/raw/tsave-m44b33.jsonl"""
import bisect, collections, datetime, json, lzma, sys

V = ["leader", "resumed", "started", "bootstrapping", "promised", "busy", "won", "lost", "mayNotLead"]
hm = lambda t: datetime.datetime.fromtimestamp(t / 1000, datetime.UTC).strftime("%H:%M:%S.%f")[:-3]
colo = lambda c: "UNKNOWN" if c == 0 else c.to_bytes(4, "big").lstrip(b"\0").decode("ascii", "replace")
path = sys.argv[1]
rows = collections.defaultdict(set)
for line in (lzma.open if path.endswith(".xz") else open)(path, "rt"):
    r = json.loads(line)
    if "rows" in r and "learner" not in r:
        rows[r["site"]].update(tuple(x) for x in r["rows"] if x[1] in (17, 28))
out = collections.Counter()
for s in sorted(rows):
    appends = sorted((x[0], x[2]) for x in rows[s] if x[1] == 17)
    at = [a for a, _ in appends]
    reports = sorted(x for x in rows[s] if x[1] == 28)
    # The member's term over time: the highest term among reports it answered `promised` about the member sending it
    # appends then. A report's own term is its sender's view, so only matching ones say anything.
    for x in reports:
        if V[x[5]] != "started":
            continue
        t0 = x[0]
        i = bisect.bisect_left(at, t0)
        last = at[i - 1] if i else None
        gap = t0 - last if last is not None else None
        leader = appends[i - 1][1] if i else None
        # The member's term just before: the term of the latest `promised` report about its leader whose term the
        # member then held, approximated by the most common term among `promised` reports about it in the 30 s.
        recent = [r for r in reports if t0 - 30_000 <= r[0] < t0 and r[2] == leader and V[r[5]] == "promised"]
        term = collections.Counter(r[3] for r in recent if r[3]).most_common(1)
        term = term[0][0] if term else None
        seen, hit = {}, None
        for r in recent:
            if r[3] != term or r[4] == 0:
                continue
            seen = {k: v for k, v in seen.items() if v >= r[0] - 10_000}
            seen[r[4]] = r[0]
            if len(seen) >= 2:
                hit = (r[0], sorted(colo(c) for c in seen))
                break
        why = "gap" if gap is not None and gap >= 10_000 else "withheld" if hit else "neither"
        out[why] += 1
        print(f"site {s} campaign {hm(t0)} (report term {x[3]}, colo {colo(x[4])}): leader {leader} term {term}; "
              f"last append {'-' if gap is None else f'{gap} ms'} before; two colos {hm(hit[0]) + ' ' + str(hit[1]) if hit else '-'} -> {why}")
print("campaigns by cause:", dict(out))
