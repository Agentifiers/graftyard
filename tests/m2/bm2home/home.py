#!/usr/bin/env python3
"""BM-2's latency clause as reworded: merge p99 on a freshly elected leader on the group's home site (enam) with the
client in that region, at most 2 s, and on a freshly elected leader off the home site, at most 3 s.

home.py RUN ROUNDS [--gh-every K]
home.py summary RUN.jsonl

Each round takes two samples, each on a new term:

- off: leadership moves from enam to weur or apac (alternating), then the orb (LAX) sends gh's own
  `PullRequestMerge` mutation to graft-m1-edge, timed at the orb, as `tests/m2/wp6/p99.py` does. Every K-th off
  sample (default 25) is a whole `gh pr merge --merge` instead, its replies kept for `tests/m2/gh-replies.py`.
- home: leadership moves back to enam, then the same mutation is sent by `graft-m1-bm2client`'s Durable Object,
  created with `locationHint: "enam"` (`client.ts`). It times the request in its own region and returns its colo
  (`cdn-cgi/trace`) and the colo the edge ran in (`x-graft-colo`).

Before each merge the pull request is MERGEABLE (its `PrSync` test merge ran), as in p99.py. The repository is
$WP3_REPO (default wp6p). Every sample is one JSON line in $WP3_RAW/RUN.jsonl; a restart picks up after the last line.
The client's bearer token is read from ~/.graft-bm2home/client.token; the edge token rides in the POST body.
"""
import json, math, os, sys, time

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "../wp3"))
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "../wp6"))
import hosted as h  # noqa: E402
import p99  # noqa: E402

CLIENT = os.environ.get("BM2_CLIENT", f"https://graft-m1-bm2client.{os.environ['CF_WORKERS_SUBDOMAIN']}.workers.dev")
CLIENT_TOK = open(os.path.expanduser("~/.graft-bm2home/client.token")).read().strip()
h.SECRETS.append(CLIENT_TOK)
HOME_SITE = 0
BOUND = {"home": 2000, "off": 3000}


def via_client(path, body=None):
    s, v, _ = h.http("POST" if body is not None else "GET", f"{CLIENT}{path}", body,
                     {"authorization": f"Bearer {CLIENT_TOK}"}, timeout=120)
    if s != 200 or not isinstance(v, dict):
        raise RuntimeError(f"client {path}: {s} {str(v)[:200]}")
    return v


def sample(w, run, i, kind, to, use_gh, out):
    w.fetch_main()
    b = f"{run}-{i}-{int(time.time()) % 100000:05}"
    w.branch(b, {f"p/{b}.txt": b})
    d = f"{h.RAW}/{run}-gh" if use_gh else f"/tmp/bm2home-{run}"
    pr = h.create_pr(w, d, f"{i:04}0-create", b)
    node = h.pr_node(pr)
    waited = p99.wait_mergeable(pr)
    # A new term on `to` for every sample: after a restart the leader may already sit there, so move it away first.
    if h.leader_site() == to:
        p99.elect(lambda s: (to + 1) % 3)
    e = p99.elect(lambda s: to)
    if e["to"] != to:
        raise RuntimeError(f"elected {e['to']}, wanted {to}")
    t0 = time.time()
    rec = {"i": i, "kind": kind, "pr": pr, "gh": bool(use_gh), "waited_s": waited}
    if kind == "home":
        v = via_client("/merge", {"token": h.WRITER, "node": node})
        rec.update(ms=v["ms"], orb_ms=round((time.time() - t0) * 1000), status=v["status"], reply=v["body"],
                   timing=v["timing"], trace=[v["trace"], v["shardTiming"]], client_colo=v["clientColo"],
                   edge_colo=v["edgeColo"])
        body = v["body"]
        ok = v["status"] == 200 and isinstance(body, dict) and not body.get("errors") and \
            (body.get("data") or {}).get("mergePullRequest") is not None
    elif use_gh:
        rc, _, err = w.gh(d, f"{i:04}1-merge", "pr", "merge", str(pr), "-R", f"{h.OWNER_LOGIN}/{h.REPO}", "--merge")
        rec.update(ms=round((time.time() - t0) * 1000), reply=err.strip().splitlines()[-1:] if rc else [],
                   timing=None, trace=None, client_colo="LAX", edge_colo=None)
        ok = rc == 0
    else:
        s, v, hd = h.http("POST", f"{h.EDGE}/api/graphql",
                          {"query": p99.MUTATION, "variables": {"input": {"pullRequestId": node, "mergeMethod": "MERGE"}}},
                          {"authorization": f"bearer {h.WRITER}"})
        g = lambda k: hd.get(k) or hd.get(k.title())  # noqa: E731
        rec.update(ms=round((time.time() - t0) * 1000), status=s, reply=v, timing=g("server-timing"),
                   trace=[g("x-graft-trace"), g("x-graft-shard-timing")], client_colo="LAX", edge_colo=g("x-graft-colo"))
        ok = s == 200 and isinstance(v, dict) and not v.get("errors") and \
            (v.get("data") or {}).get("mergePullRequest") is not None
    state = h.pr_state(pr)
    rec.update(state=state, at=time.time(), **e)
    rec["pass"] = ok and state == "MERGED"
    h.append(out, rec)
    h.log(f"{i} {kind}: {rec['ms']} ms {'pass' if rec['pass'] else 'FAIL'} leader {e['from']}->{e['to']} "
          f"term {e['term']} client {rec['client_colo']} edge {rec['edge_colo']} {rec['timing'] or ''}")


def main():
    run, rounds = sys.argv[1], int(sys.argv[2])
    a = sys.argv[3:]
    gh_every = int(a[a.index("--gh-every") + 1]) if "--gh-every" in a else 25
    h.REPO = os.environ.get("WP3_REPO", "wp6p")
    where = via_client("/where")
    h.log(f"client {where}")
    if where["clientColo"] in ("", None) or where["edgeColo"] in ("", None):
        raise RuntimeError(f"client placement unproven: {where}")
    out, done = h.resume(run)
    w = h.Work(run)
    offs = 0
    for i in range(done, 2 * rounds):
        if i % 2 == 0:
            # Off home: from enam to weur, then apac, alternating; a leader already off home moves home first.
            if h.leader_site() != HOME_SITE:
                p99.elect(lambda s: HOME_SITE)
            to = 1 + (i // 2) % 2
            offs = i // 2
            sample(w, run, i, "off", to, gh_every and offs % gh_every == gh_every - 1, out)
        else:
            sample(w, run, i, "home", HOME_SITE, False, out)
    summary(out)


def q(xs, p):
    xs = sorted(xs)
    return xs[min(len(xs) - 1, math.ceil(p * len(xs)) - 1)]


def summary(out):
    """Per clause: n, pass, p50/p90/p99/max, the count over its bound, and whether a p99 pass can be called at that n:
    with n samples, p99 at most the bound means at most floor(0.01 n) samples over it."""
    rows = [json.loads(l) for l in open(out)]
    ghs = [r for r in rows if r["gh"]]
    if ghs:
        h.log(f"gh pr merge whole runs (off, LAX): n={len(ghs)} pass={sum(r['pass'] for r in ghs)} "
              f"p50={q([r['ms'] for r in ghs], .5)} max={max(r['ms'] for r in ghs)} ms")
    for kind in ("home", "off"):
        rs = [r for r in rows if r["kind"] == kind and not r["gh"]]
        if not rs:
            continue
        ms = [r["ms"] for r in rs]
        over = sum(m > BOUND[kind] for m in ms)
        allow = math.floor(0.01 * len(ms))
        sites = {}
        for r in rs:
            sites.setdefault(r["to"], []).append(r["ms"])
        h.log(f"{kind}: n={len(ms)} pass={sum(r['pass'] for r in rs)} p50={q(ms, .5)} p90={q(ms, .9)} "
              f"p99={q(ms, .99)} max={max(ms)} ms; over {BOUND[kind]} ms: {over} (a pass allows {allow}); "
              f"terms {len({r['term'] for r in rs})}; client colos {sorted({r['client_colo'] for r in rs})}; "
              f"edge colos {sorted({str(r['edge_colo']) for r in rs})}; "
              + "; ".join(f"leader {s}: n={len(v)} p99={q(v, .99)} over={sum(m > BOUND[kind] for m in v)}"
                          for s, v in sorted(sites.items())))


if __name__ == "__main__":
    if sys.argv[1] == "summary":
        summary(sys.argv[2])
    else:
        main()
