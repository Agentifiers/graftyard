#!/usr/bin/env python3
"""BM-2's merge p99 on a freshly elected leader, on the graft-m1 test install through graft-m1-edge.

p99.py RUN SAMPLES [--gh-every K]

Each sample opens a pull request (a new branch pushed with real git, `gh pr create`), then moves the repository
group's leadership to the next site with the leader's member `/transfer`, waits until that site's member reports
`leader` at a higher term, and at once sends gh's own `PullRequestMerge` mutation (`mergePullRequest` with
`clientMutationId`, as `gh pr merge` sends it) through the edge. The sample is that request's time at the client.
Every K-th sample (default 25) merges with real `gh pr merge --merge` instead, its replies kept for
`tests/m2/gh-replies.py`, and its `mergePullRequest` time read from gh's debug log. A sample passes when the reply is
the payload with no errors and the pull request then reads `MERGED`.

The repository is $WP3_REPO (default wp6p) in repos.json; it needs `hosted.py setup` first. Every sample is one JSON
line in $WP3_RAW/RUN.jsonl; a restart picks up after the last line.
"""
import json, math, os, sys, time

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "../wp3"))
import hosted as h  # noqa: E402

MUTATION = "mutation PullRequestMerge($input:MergePullRequestInput!){mergePullRequest(input: $input){clientMutationId}}"


def status(site):
    return h.member(site, "/status", None).get("v") or {}


def elect(nxt):
    """Moves leadership to the next site; (from, to, old term, new term, ms until the new leader reported)."""
    s = h.leader_site()
    old = status(s).get("term", 0)
    to = nxt(s)
    t0 = time.time()
    r = h.member(s, "/transfer", {"to": to}).get("v")
    for _ in range(200):
        v = status(to)
        if v.get("role") == "leader" and v.get("term", 0) > old:
            return {"from": s, "to": to, "term0": old, "term": v["term"], "elect_ms": round((time.time() - t0) * 1000),
                    "transfer": r}
        time.sleep(0.05)
    raise RuntimeError(f"site {to} did not lead after transfer from {s}: {r}")


def main():
    run, n = sys.argv[1], int(sys.argv[2])
    a = sys.argv[3:]
    gh_every = int(a[a.index("--gh-every") + 1]) if "--gh-every" in a else 25
    h.REPO = os.environ.get("WP3_REPO", "wp6p")
    out, done = h.resume(run)
    w = h.Work(run)
    sites = 3
    for i in range(done, n):
        w.fetch_main()
        b = f"{run}-{i}-{int(time.time()) % 100000:05}"
        w.branch(b, {f"p/{b}.txt": b})
        use_gh = gh_every and i % gh_every == gh_every - 1
        d = f"{h.RAW}/{run}-gh" if use_gh else f"/tmp/wp6-{run}"
        pr = h.create_pr(w, d, f"{i:04}0-create", b)
        node = h.pr_node(pr)
        e = elect(lambda s: (s + 1) % sites)
        t0 = time.time()
        if use_gh:
            rc, _, err = w.gh(d, f"{i:04}1-merge", "pr", "merge", str(pr), "-R", f"{h.OWNER_LOGIN}/{h.REPO}", "--merge")
            ms = round((time.time() - t0) * 1000)
            ok_reply, timing, reply = rc == 0, None, err.strip().splitlines()[-1:] if rc else []
            trace = None
        else:
            s, v, hd = h.http("POST", f"{h.EDGE}/api/graphql",
                              {"query": MUTATION, "variables": {"input": {"pullRequestId": node, "mergeMethod": "MERGE"}}},
                              {"authorization": f"bearer {h.WRITER}"})
            ms = round((time.time() - t0) * 1000)
            ok_reply = s == 200 and isinstance(v, dict) and not v.get("errors") and \
                (v.get("data") or {}).get("mergePullRequest") is not None
            timing, reply = hd.get("server-timing") or hd.get("Server-Timing"), v
            trace = [hd.get("x-graft-trace") or hd.get("X-Graft-Trace"), hd.get("x-graft-shard-timing") or hd.get("X-Graft-Shard-Timing")]
        state = h.pr_state(pr)
        rec = {"i": i, "pr": pr, "gh": bool(use_gh), "ms": ms, "pass": ok_reply and state == "MERGED", "state": state,
               "timing": timing, "trace": trace, "reply": reply, "at": time.time(), **e}
        h.append(out, rec)
        h.log(f"{i}: {ms} ms {'pass' if rec['pass'] else 'FAIL'} leader {e['from']}->{e['to']} term {e['term']} "
              f"elect {e['elect_ms']} ms {timing or ''}")
    summary(out)


def summary(out):
    """The p99 is over the mutation samples only; gh's whole runs (a read and the mutation) are reported apart."""
    allrows = [json.loads(l) for l in open(out)]
    ghs = sorted(r["ms"] for r in allrows if r["gh"])
    if ghs:
        h.log(f"gh pr merge whole runs: n={len(ghs)} pass={sum(r['pass'] for r in allrows if r['gh'])} "
              f"p50={ghs[len(ghs) // 2]} max={ghs[-1]} ms")
    rows = [r for r in allrows if not r["gh"]]
    ms = sorted(r["ms"] for r in rows)
    k = len(ms)
    p = lambda q: ms[min(k - 1, math.ceil(q * k) - 1)]
    by = {}
    for r in rows:
        by.setdefault(r["to"], []).append(r["ms"])
    h.log(f"n={k} pass={sum(r['pass'] for r in rows)} p50={p(0.5)} p90={p(0.9)} p99={p(0.99)} max={ms[-1]} ms; "
          + "; ".join(f"leader {s}: n={len(v)} p99={sorted(v)[min(len(v) - 1, math.ceil(0.99 * len(v)) - 1)]}"
                      for s, v in sorted(by.items())))


if __name__ == "__main__":
    if sys.argv[1] == "summary":
        summary(sys.argv[2])
    else:
        main()
