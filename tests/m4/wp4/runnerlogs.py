#!/usr/bin/env python3
"""runnerlogs.py FROM_ISO TO_ISO -> every `graft-runners` Workers Logs event in the window, one line each, then a
`chunks` line naming any hour the API never answered.

The canary's log reads (`sandboxstarts.py` and this search) split the window into chunks of at most `CHUNK_MS` (1 h).
Each chunk has its own deadline (`CHUNK_S`, 900 s), and each query in it has its own timeout (`QUERY_S`, 30 s) and up
to `TRIES` (3) tries with backoff, so one failed call costs a retry, not the chunk's work so far. A chunk whose query
fails every try, or that passes its deadline, is reported by its bounds, and the rest of the window is still read.
A busy hour is many queries: m57's hour on the runner account (2026-11-19 18:35Z) held 11,156 events, 181 queries and
140 s, so a chunk-wide retry of 300 s each never finished it. One 26-hour read failed both canary wakes
on 2026-11-20 (09:11Z and 13:14Z) on a single TLS handshake timeout to the observability API. A check that names a
failed chunk is incomplete, and the canary must not promote on it.

Within a chunk, a span that answers fewer than 100 events (the events view's limit, newest first) is complete. A span
that answers 100 is split into 12, down to `SLICE_MS` (30 s), and a 30 s slice is walked back by timestamp. That is
`sandboxstarts.py`'s F80 rule (one walk back through a busy window can end early), applied only where a span is busy,
so a quiet hour costs one query instead of 120.

Account and token from the environment: the runner account's `CLOUDFLARE_RUNNER_ACCOUNT_ID` and
`CLOUDFLARE_RUNNER_API_TOKEN` when set, else `CLOUDFLARE_ACCOUNT_ID` and `CLOUDFLARE_API_TOKEN`. The token is sent only as a header. Exit 0 when every
chunk answered, 3 when any failed.
"""
import json, os, sys, time, urllib.request
from datetime import datetime, timezone

CHUNK_MS = 3_600_000
SLICE_MS = 30_000
LIMIT = 100
TRIES = 3
CHUNK_S = 900
QUERY_S = 30
BACKOFF_S = (5, 20)

ms = lambda s: int(datetime.fromisoformat(s.replace("Z", "+00:00")).timestamp() * 1000)
iso = lambda t: datetime.fromtimestamp(t / 1000, timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


class ChunkTimeout(Exception):
    pass


def api_query(acct, tok):
    """One events-view query for `graft-runners` over [lo, hi): newest first, at most LIMIT."""
    def query(lo, hi, timeout):
        body = {"queryId": "q", "timeframe": {"from": lo, "to": hi}, "view": "events", "limit": LIMIT, "parameters": {
            "filters": [{"key": "$metadata.service", "operation": "eq", "type": "string", "value": "graft-runners"}]}}
        r = urllib.request.Request(
            f"https://api.cloudflare.com/client/v4/accounts/{acct}/workers/observability/telemetry/query",
            data=json.dumps(body).encode(), method="POST",
            headers={"authorization": f"Bearer {tok}", "content-type": "application/json"})
        return json.load(urllib.request.urlopen(r, timeout=timeout))["result"]["events"]["events"]
    return query


def _tried(query, lo, hi, deadline, clock, sleep):
    """One query, with its own timeout, tried up to TRIES times with backoff, within the chunk's deadline."""
    for t in range(TRIES):
        left = deadline - clock()
        if left <= 0:
            raise ChunkTimeout(f"chunk deadline passed at {iso(lo)}")
        try:
            return query(lo, hi, min(QUERY_S, left))
        except Exception:  # noqa: BLE001 - retried, then raised to fail the chunk
            if t + 1 == TRIES:
                raise
            sleep(BACKOFF_S[min(t, len(BACKOFF_S) - 1)])


def _span(query, lo, hi, deadline, clock, sleep):
    """Every event in [lo, hi), splitting a span that hit the limit."""
    got = _tried(query, lo, hi, deadline, clock, sleep)
    if len(got) < LIMIT:
        return got
    if hi - lo > SLICE_MS:
        step = max(SLICE_MS, -(-(hi - lo) // 12))
        return [e for s in range(lo, hi, step) for e in _span(query, s, min(s + step, hi), deadline, clock, sleep)]
    # A busy 30 s slice: walk back from its newest event.
    out, seen = list(got), {e["$metadata"]["id"] for e in got}
    while True:
        top = min(e["timestamp"] for e in got) + 1
        if top <= lo:
            return out
        got = _tried(query, lo, top, deadline, clock, sleep)
        new = [e for e in got if e["$metadata"]["id"] not in seen]
        seen.update(e["$metadata"]["id"] for e in new)
        out += new
        if len(got) < LIMIT or not new:
            return out


def fetch(a, b, query, clock=time.monotonic, sleep=time.sleep, chunk_ms=CHUNK_MS, progress=None):
    """Events in [a, b) by chunks of at most `chunk_ms`, deduplicated and oldest first, and the chunks that did not
    answer, as {"from", "to", "error"}. `progress(line)` gets one line per chunk."""
    events, seen, failed = [], set(), []
    for lo in range(a, b, chunk_ms):
        hi, start = min(lo + chunk_ms, b), clock()
        try:
            got = _span(query, lo, hi, start + CHUNK_S, clock, sleep)
        except Exception as e:  # noqa: BLE001 - any failure is this chunk's, reported below
            failed.append({"from": iso(lo), "to": iso(hi), "error": f"{type(e).__name__}: {e}"[:200]})
            if progress:
                progress(f"{iso(lo)} FAILED after {clock() - start:.0f} s: {failed[-1]['error']}")
            continue
        if progress:
            progress(f"{iso(lo)} {len(got)} events in {clock() - start:.0f} s")
        for e in got:
            if e["$metadata"]["id"] not in seen:
                seen.add(e["$metadata"]["id"])
                events.append(e)
    events.sort(key=lambda e: e["timestamp"])
    return events, failed


def chunks_line(a, b, failed, chunk_ms=CHUNK_MS):
    return {"chunks": -(-(b - a) // chunk_ms), "failed": failed, "complete": not failed}


def credentials():
    """The runner account's vars first: `tests/am1/lf13/env-m4a.sh` also sets `CLOUDFLARE_ACCOUNT_ID`, to graft-m4a,
    where no `graft-runners` runs, and a search there answers every chunk with no events."""
    if os.environ.get("CLOUDFLARE_RUNNER_ACCOUNT_ID"):
        return os.environ["CLOUDFLARE_RUNNER_ACCOUNT_ID"], os.environ["CLOUDFLARE_RUNNER_API_TOKEN"]
    return os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]


def main():
    acct, tok = credentials()
    a = ms(sys.argv[1])
    b = ms(sys.argv[2]) if len(sys.argv) > 2 else int(time.time() * 1000)
    events, failed = fetch(a, b, api_query(acct, tok), progress=lambda l: print(l, file=sys.stderr, flush=True))
    for e in events:
        m, w = e["$metadata"], e.get("$workers") or {}
        t = datetime.fromtimestamp(e["timestamp"] / 1000, timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.%f")[:-3]
        rpc = ",".join((w.get("event") or {}).get("rpcMethods") or [])
        ver = ((w.get("scriptVersion") or {}).get("id") or "")[:8]
        msg = (m.get("message") or "").replace(acct, "<acct>")
        print(t, ver, w.get("eventType") or "", (w.get("durableObjectId") or "")[:8], rpc, m.get("type", ""), msg[:400])
    print(json.dumps(chunks_line(a, b, failed)))
    sys.exit(0 if not failed else 3)


if __name__ == "__main__":
    main()
