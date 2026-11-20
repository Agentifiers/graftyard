#!/usr/bin/env python3
"""outagecut.py SAMPLES OUTAGES [KIND...] -> each kind's line as measured and with the harness-outage windows removed
(tests/fanout/README.md, "Harness outages on gate rows").

SAMPLES is a1.mjs's `a1-samples-<row>-<run>.jsonl.gz` ([start ms on Cloudflare's clock, kind, ms, code] per sample).
OUTAGES is arrivals.py's summary line (`{"outages": [[from, to], ..], ...}`) or a file whose last line it is. A sample
is removed when it started inside a window or less than SLACK_MS before one (its retries ran into it). Served is the
share answered 200 of the samples that were sent (`skipped: cap` was never sent and is counted apart). Default kinds:
read:api, read:ref, merge:only, push:agent.
"""
import gzip
import json
import sys
from datetime import datetime, timezone

SLACK_MS = 3000  # a1's api() retries a failed fetch twice, 1 s apart: a sample that started this close before a window ran into it


def ms(s):
    return int(datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)


def pct(xs, q):
    return sorted(xs)[min(len(xs) - 1, int(q * len(xs)))] if xs else None


def line(samples):
    sent = [s for s in samples if s[3] != "skipped: cap"]
    ok = [s for s in sent if s[3] in ("200", "ok")]
    return {"n": len(sent), "served": round(len(ok) / len(sent), 4) if sent else None,
            "p50": pct([s[2] for s in sent], .5), "p99": pct([s[2] for s in sent], .99),
            "cap": len(samples) - len(sent)}


def first(path):
    """The start (ms, Cloudflare's clock) of the row's first request, its earliest sample. The load window opens here,
    not at the driver's announced start: the seconds before it are the client's own setup (DECISIONS,
    "`outage-window`")."""
    with gzip.open(path, "rt") as f:
        return min(json.loads(x)[0] for x in f if x.strip())


def cut(samples, wins):
    return [s for s in samples if not any(a - SLACK_MS <= s[0] < b for a, b in wins)]


def main(argv):
    with gzip.open(argv[0], "rt") as f:
        samples = [json.loads(x) for x in f if x.strip()]
    src = argv[1]
    text = src if src.lstrip().startswith("{") else open(src).read().strip().splitlines()[-1]
    outs = json.loads(text)["outages"]
    # A low-rate row (arrivals.py's "n/a (low rate)") is not judged by the rule: nothing is removed.
    wins = [(ms(a), ms(b)) for a, b in outs] if isinstance(outs, list) else []
    kinds = argv[2:] or ["read:api", "read:ref", "merge:only", "push:agent"]
    for k in kinds:
        mine = [s for s in samples if s[1] == k]
        print(json.dumps({"kind": k, "measured": line(mine), "outagesRemoved": line(cut(mine, wins))}))


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] == "--first":
        # `outagecut.py --first SAMPLES` -> the first request's second, ISO, where outages.sh opens the window.
        t = first(sys.argv[2])
        print(datetime.fromtimestamp((t - t % 1000) / 1000, timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"))
        sys.exit(0)
    if len(sys.argv) < 3:
        sys.exit(__doc__)
    main(sys.argv[1:])
