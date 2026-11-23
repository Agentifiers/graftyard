#!/usr/bin/env python3
"""hostguard.py: judge a B0-33 leader's host from a1's setup, before load (`tests/am1/results/b033-pga.md`, "The host
guard").

  hostguard.py SERVICE NAME FROM_ISO [TO_ISO]   SERVICE is the leader's site script (graft-m2b-site-a), NAME its
                                       object (`repo:131393#1#f0`), FROM a time before a1's setup began (a1's log start),
                                       TO the window's end for a replay of a past setup (default now).

a1's setup creates 100 tasks, one `/commit` each, at the same pace on every run. The guard finds the first `/commit`
of that run (not the repository's own creation commits before it), sums the leader object's Workers Logs CPU (every
route) over the GUARD_S seconds from there, over the `/commit` calls in that window, and compares the result with
LINE_MS. Three outcomes, printed as one line:
  exit 0  keep: CPU per setup commit at or under LINE_MS;
  exit 3  re-place: over LINE_MS;
  exit 2  unknown: no run of setup commits yet, the window not ended INGEST_MS ago, or fewer than MIN_COMMITS
          commits in it. A row must not load on unknown (`guardrow.sh` fails closed).
Workers Logs only; no request path and no member is touched. Needs CLOUDFLARE_ACCOUNT_ID and CLOUDFLARE_API_TOKEN.
"""
import os, sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import wlobj  # noqa: E402

GUARD_S = 60
# 1.2 times the larger of the two baseline hosts' readings by this script: blB (EWR) 23.7 and qlC (EWR) 21.8 ms a
# setup commit. On the twelve B0-33 runs of 2026-11-21 to 23 it keeps sabA, crA, blB, qlA, qlB and qlC (21.8 to 26.2)
# and re-places chA, blA, blC2, ncA, pfA and pgA (31.0 to 46.1): the slow-host runs by A41, and of those with a
# heavy-turn join, the ones whose loaded leader paid 2.4 to 3.5 ms an event applied against 1.2 to 1.6
# (`tests/am1/results/b033-pga.md`, "The host guard"). It reads object f0, the leader a1's setup names on these rows.
LINE_MS = 28.4
MIN_COMMITS = 30
# The task phase's pace: about 1.2 to 1.6 s between commits on every run, so RUN commits within RUN_GAP_MS each.
RUN, RUN_GAP_MS = 5, 5_000
# Workers Logs' ingestion lag: a window is judged only this long after it ends.
INGEST_MS = 30_000


def calc(svc, do, a, b, extra, calcs):
    body = {"queryId": "hostguard", "timeframe": {"from": a, "to": b}, "view": "calculations", "limit": 10,
            "parameters": {"filters": wlobj.filters(svc, do) + extra, "calculations": calcs}}
    out = wlobj.post(wlobj.URL, body)["result"]["calculations"]
    return [c["aggregates"][0]["value"] if c["aggregates"] else 0 for c in out]


def main(svc, name, frm, to=None):
    a = wlobj.ms(frm)
    # TO (optional): replay a past setup, as the line's derivation did; live, the window ends now.
    now = wlobj.ms(to) if to else int(__import__("time").time() * 1000)
    do = wlobj.object_id(name, frm, wlobj.datetime.utcfromtimestamp(now / 1000).strftime("%Y-%m-%dT%H:%M:%SZ"))
    commit = [{"key": "$workers.event.path", "operation": "eq", "type": "string", "value": "/commit"}]
    # The task creates' first `/commit`: the first of RUN commits each within RUN_GAP_MS of the next. The repository's
    # own setup commits come earlier and apart (pgA: two at 02:13:28, then the tasks from 02:14:22), and a window
    # anchored on them held 6 commits, not 46. One events query a minute from FROM, timestamps only.
    ts, first = [], None
    for t in range(a, now, 60_000):
        body = {"queryId": "hostguard", "timeframe": {"from": t, "to": min(now, t + 60_000)}, "view": "events",
                "limit": 2000, "parameters": {"filters": wlobj.filters(svc, do) + commit, "limit": 2000}}
        ts += [e["timestamp"] for e in wlobj.post(wlobj.URL, body)["result"].get("events", {}).get("events", [])]
        ts.sort()
        first = next((ts[i] for i in range(len(ts) - RUN + 1)
                      if all(ts[j + 1] - ts[j] <= RUN_GAP_MS for j in range(i, i + RUN - 1))), None)
        if first is not None:
            break
    if first is None:
        print(f"hostguard {name}: unknown, no run of {RUN} setup commits yet")
        return 2
    b = first + GUARD_S * 1000
    if now < b + INGEST_MS:
        end = wlobj.datetime.utcfromtimestamp(b / 1000).strftime("%H:%M:%SZ")
        print(f"hostguard {name}: unknown, the window ends at {end} and its logs may still be arriving")
        return 2
    (n,) = calc(svc, do, first, b, commit, [{"operator": "count"}])
    (cpu,) = calc(svc, do, first, b, [], [{"operator": "sum", "key": "$workers.cpuTimeMs", "keyType": "number"}])
    if n < MIN_COMMITS:
        print(f"hostguard {name}: unknown, {n} setup commits in the first {GUARD_S} s")
        return 2
    per = cpu / n
    verdict = "keep" if per <= LINE_MS else "re-place"
    print(f"hostguard {name}: {verdict}, {per:.1f} ms of leader CPU a setup commit ({n} commits, {cpu:.0f} ms, "
          f"line {LINE_MS})")
    return 0 if verdict == "keep" else 3


if __name__ == "__main__":
    sys.exit(main(*sys.argv[1:5]))
