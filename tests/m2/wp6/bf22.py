#!/usr/bin/env python3
"""BF-22's hosted part at a reduced count, on graft-m1 through graft-m1-edge.

bf22.py RUN N

Opens a pull request on $WP3_REPO (default wp6j), then posts N non-required statuses on its head through
`POST /repos/o/r/statuses/{sha}` (contexts `nr-0` .. `nr-(N-1)`, 8 at a time). Then:

- `gh pr checks` lists every one of them, and none is marked required;
- `GET /repos/o/r/commits/{sha}/statuses`, paged by its `link` header, returns all N;
- the core tier's rows on the leader are the same before and after the statuses (they live in Metadata);
- every member reports the same core and metadata `fold_digest` at one index (`/fold {"at": i}`).

The prune clause (entry time advanced past `check_retention_days`) needs a clock the install does not let a test move;
it stays with the pure test `bf22_non_required_checks_live_in_metadata_and_prune_on_every_member`.
"""
import json, os, sys, threading, time

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "../wp3"))
import hosted as h  # noqa: E402


def core_rows(site):
    """The leader's core-tier rows (`{"q": {"rows": true}}`): what "core tier bytes unchanged" compares. The tier's
    `fold_digest` also hashes `applied`, the term, and the last entry's digest, so it moves on every entry."""
    v = h.member(site, "/read", {"q": {"rows": True}})["v"]["value"]
    assert v["complete"], "rows truncated"
    return json.dumps(v["core"], sort_keys=True)


def tiers(site):
    """The leader's tiers: each one's `applied` and `fold_digest` (`{"q": {"tiers": true}}`)."""
    return h.member(site, "/read", {"q": {"tiers": True}})["v"]["value"]


def main():
    run, n = sys.argv[1], int(sys.argv[2])
    h.REPO = os.environ.get("WP3_REPO", "wp6j")
    R = f"{h.OWNER_LOGIN}/{h.REPO}"
    w = h.Work(run)
    d = f"{h.RAW}/{run}"
    w.fetch_main()
    b = f"{run}-{int(time.time()) % 100000:05}"
    head = w.branch(b, {f"bf22/{b}.txt": b})
    pr = h.create_pr(w, d, "00-create", b)
    leader = h.leader_site()
    time.sleep(3)  # the pull request's own `TestMerged` (a core row) lands first
    core0 = core_rows(leader)
    errs, lock = [], threading.Lock()

    def post(lo):
        for k in range(lo, n, 8):
            for _ in range(5):
                try:
                    h.status(head, "success", context=f"nr-{k}")
                    break
                except Exception as e:  # noqa: BLE001
                    with lock:
                        errs.append(str(e)[:120])
                    time.sleep(1)
    t0 = time.time()
    ts = [threading.Thread(target=post, args=(i,)) for i in range(8)]
    for t in ts:
        t.start()
    for t in ts:
        t.join()
    post_s = round(time.time() - t0, 1)
    rc, out, _ = w.gh(d, "01-checks", "pr", "checks", str(pr), "-R", R, "--json", "name,state,bucket", timeout=300)
    checks = json.loads(out) if rc in (0, 8) and out.strip() else []
    names = {c["name"] for c in checks}
    rc2, out2, _ = w.gh(d, "02-required", "pr", "checks", str(pr), "-R", R, "--required", "--json", "name",
                        timeout=300)
    required = json.loads(out2) if out2.strip().startswith("[") else []
    got, url = [], f"/repos/{R}/commits/{head}/statuses?per_page=100"
    while url:
        s, v, hd = h.api("GET", url)
        if s != 200:
            raise RuntimeError(f"statuses: {s} {str(v)[:200]}")
        got.extend(v)
        h.rest_case(d, f"03-statuses-{len(got):04}", "get", "/repos/{owner}/{repo}/commits/{ref}/statuses", s, v)
        link = hd.get("link") or hd.get("Link") or ""
        nxt = [p.split(";")[0].strip("<> ") for p in link.split(",") if 'rel="next"' in p]
        url = nxt[0].split("/api/v3", 1)[1] if nxt else None
    leader = h.leader_site()
    core1, at = core_rows(leader), tiers(leader)["metadata"]["applied"]
    time.sleep(3)
    by = {s: h.member(s, "/fold", {"at": at})["v"] for s in range(3)}
    meta_at = {s: by[s]["metadata"]["at"] for s in range(3)}
    core_at = {s: by[s]["core"]["at"] for s in range(3)}
    want = {f"nr-{k}" for k in range(n)}
    rest_names = {x["context"] for x in got}
    ok = {
        "gh pr checks lists all": want <= names,
        "none required": not (want & {c["name"] for c in required}),
        "REST lists all": want <= rest_names and len([x for x in got if x["context"] in want]) == n,
        "core unchanged": core0 == core1,
        "members agree (metadata)": len(set(meta_at.values())) == 1 and None not in meta_at.values(),
        "members agree (core)": len(set(core_at.values())) == 1 and None not in core_at.values(),
    }
    rec = {"run": run, "n": n, "pr": pr, "head": head, "post_s": post_s, "post_errors": errs[:5],
           "gh_checks": len(names & want), "gh_rc": rc, "rest": len(rest_names & want),
           "core_bytes": len(core0), "at": at, "meta_at": meta_at, "core_at": core_at, "ok": ok, "pass": all(ok.values())}
    h.append(f"{h.RAW}/{run}.jsonl", rec)
    h.log(json.dumps(rec)[:1500])


if __name__ == "__main__":
    main()
