#!/usr/bin/env python3
"""dispatchlog.py REPO_ID FROM_ISO TO_ISO -> B0-35's site-side dispatch line from the sites' Workers Logs: every
`actions dispatch <repo> #N <action> at <index>: <n> runs in <ms>ms, <k> asks[, superseded][, conflict|, pending|, unmerged]`
line `ActionsTrigger` logs (`workers/site/src/actions_trigger.rs`, `decisions/DECISIONS.md`, "`pr-trigger`"), and
its `actions trigger <repo>: ...` round errors. Prints one JSON summary: events by outcome (run, conflict, unmerged,
no run) as last logged and on the first try (a failed round or a new leader runs a batch again), superseded, asks per outcome, the entry-to-run ms percentiles of events that ran, the same event logged twice
(a round rerun), and the error lines by kind.

The events view answers at most 100 lines a query, newest first, so the window is walked back in slices per site
script ($INSTALL-site-{a,b,c}, INSTALL default graft-m4a). Account and token from the environment, the token only as a
header. RAW=<path> also writes every parsed line, one JSON object a line (site, log time, PR, entry index, ms, asks).
"""
import collections, json, os, re, sys, time, urllib.error, urllib.request
from datetime import datetime

acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
repo, a, b = sys.argv[1], sys.argv[2], sys.argv[3]
P = os.environ.get("INSTALL", "graft-m4a")
ms = lambda s: int(datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
LINE = re.compile(r"actions dispatch (\d+) #(\d+) (\w+) at (\d+): (\d+) runs in (\d+)ms, (\d+) asks(.*)")


def query(script, needle, lo, hi):
    body = {"queryId": "q", "timeframe": {"from": lo, "to": hi}, "view": "events", "limit": 100, "parameters": {
        "filters": [{"key": "$metadata.service", "operation": "eq", "type": "string", "value": script},
                    {"key": "$metadata.message", "operation": "includes", "type": "string", "value": needle}]}}
    r = urllib.request.Request(
        f"https://api.cloudflare.com/client/v4/accounts/{acct}/workers/observability/telemetry/query",
        data=json.dumps(body).encode(), method="POST",
        headers={"authorization": f"Bearer {tok}", "content-type": "application/json"})
    for t in range(6):
        try:
            return json.load(urllib.request.urlopen(r, timeout=90))["result"]["events"]["events"]
        except (urllib.error.HTTPError, urllib.error.URLError, TimeoutError) as e:
            if t == 5 or (isinstance(e, urllib.error.HTTPError) and e.code < 500 and e.code != 429):
                raise
            time.sleep(5 * (t + 1))


def walk(script, needle):
    """Every line in [a, b): a slice that answers 100 is halved, since the view's order is not by time. A round logs its
    events in one millisecond, so a 1 ms slice of 100 is read again split by the PR number's leading digits (`#7 `,
    `#42`, which also holds 420 to 429). The walk before this stepped back from the oldest line returned and stopped
    early: B0-35 m54's first pass kept 17 of site b's 1,642 lines in one try and 281 of site a's 2,168 in another."""
    out, seen = [], set()

    def take(got):
        for e in got:
            if e["$metadata"]["id"] not in seen:
                seen.add(e["$metadata"]["id"])
                out.append(e)

    def slice_(lo, hi):
        got = query(script, needle, lo, hi)
        if len(got) < 100:
            return take(got)
        if hi - lo > 1:
            mid = (lo + hi) // 2
            return slice_(lo, mid) or slice_(mid, hi)
        stem = needle if needle.endswith("#") else needle + "#"
        for k in [f"{d} " for d in range(1, 10)] + [str(d) for d in range(10, 100)]:
            take(query(script, stem + k, lo, hi))

    slice_(ms(a), ms(b))
    return out


def pct(xs, q):
    s = sorted(xs)
    return s[min(len(s) - 1, int(q * len(s)))] if s else None


events, errors = [], collections.Counter()
for site in "abc":
    script = f"{P}-site-{site}"
    for e in walk(script, f"actions dispatch {repo} "):
        m = LINE.search(e["$metadata"].get("message") or "")
        if m:
            events.append({"site": site, "t": e["timestamp"], "n": int(m[2]), "action": m[3], "index": int(m[4]),
                           "runs": int(m[5]), "ms": int(m[6]), "asks": int(m[7]), "tail": m[8]})
    for e in walk(script, f"actions trigger {repo}"):
        msg = re.sub(r"\d{3,}", "N", e["$metadata"].get("message") or "")
        errors[msg[:120]] += 1
    for e in walk(script, f"actions dispatch {repo} #"):
        msg = e["$metadata"].get("message") or ""
        if " demand " in msg:
            errors[re.sub(r"\d{3,}", "N", msg)[:120]] += 1

if os.environ.get("RAW"):
    with open(os.environ["RAW"], "w") as f:
        for x in sorted(events, key=lambda y: y["t"]):
            f.write(json.dumps(x) + "\n")

# One event is (index, n, action); a rerun round logs it again.
by = collections.defaultdict(list)
for x in events:
    by[(x["index"], x["n"], x["action"])].append(x)
outcome, first = collections.Counter(), collections.Counter()


def outcome_of(x):
    # `pending`: shed this round and carried to the next (F84); `unmerged`: given up, or main's rule before F84.
    return "conflict" if "conflict" in x["tail"] else "unmerged" if "unmerged" in x["tail"] or "pending" in x["tail"] else \
        "run" if x["runs"] > 0 else "no run"


asks = collections.defaultdict(collections.Counter)
ran_ms, superseded = [], 0
for k, xs in by.items():
    x = max(xs, key=lambda y: y["t"])  # the last time it was logged is what stood
    f = min(xs, key=lambda y: y["t"])  # the first try, before any rerun of its round
    first[outcome_of(f)] += 1
    o = outcome_of(x)
    outcome[o] += 1
    asks[o][x["asks"]] += 1
    superseded += "superseded" in x["tail"]
    if o == "run":
        ran_ms.append(x["ms"])
n = len(by)
print(json.dumps({
    "repo": repo, "window": [a, b], "lines": len(events), "events": n, "loggedTwice": sum(sum("pending" not in y["tail"] for y in v) > 1 for v in by.values()),
    "byAction": dict(collections.Counter(k[2] for k in by)), "outcome": dict(outcome), "firstOutcome": dict(first),
    "unmergedFirstPct": round(100 * first["unmerged"] / n, 2) if n else None,
    "unmergedPct": round(100 * outcome["unmerged"] / n, 2) if n else None,
    "superseded": superseded, "asks": {o: dict(sorted(c.items())) for o, c in asks.items()},
    "entryToRunMs": {"n": len(ran_ms), "p50": pct(ran_ms, 0.5), "p99": pct(ran_ms, 0.99), "max": max(ran_ms) if ran_ms else None},
    "errors": dict(errors.most_common(20)),
}))
