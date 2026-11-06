#!/usr/bin/env python3
"""BM-2's journeys 2 and 3 (`design/DESIGN.md`, "Commit and read paths") with real gh and git through graft-m1-edge.

journeys.py setup              wp6j's `main`, its settings, and its rule: `ci` required and one approving review
journeys.py j2 RUN TRIALS      journey 2 on a same-repository pull request, TRIALS times
journeys.py j3 RUN TRIALS      journey 3, TRIALS times

Journey 2, each trial: the writer pushes a branch and opens a pull request (`gh pr create`). `gh pr merge` is refused
while the review and the check are missing. Then three clients start at once: the owner's `gh pr review --approve`,
CI's `POST /statuses/{head}` with `ci` success, and the writer's `gh pr merge --merge`, retried until it exits 0 (a
merge decided before the review and the check committed is refused, as step 3 says; one decided after both merges).
Then `gh pr view` and `gh pr list --state merged` run at once and must both show it MERGED, and `main` must be the
merge commit (base, head). The fork in the journey's title is not built (`HostedDir::ref_of`: Milestone 2's
repositories are roots), so the head is a branch of the base repository.

Journey 3, each trial: `gh label create bug` once (then `--force`), `gh issue create -l bug`, then
`gh issue list --label bug` and `gh api repos/o/r/issues?sort=updated&labels=bug` at once; both must include it.

gh runs with GH_DEBUG=api and keeps its replies in $WP3_RAW/RUN/ for tests/m2/gh-replies.py. Every trial is one JSON
line in $WP3_RAW/RUN.jsonl.
"""
import json, os, sys, threading, time

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "../wp3"))
import hosted as h  # noqa: E402

OWNER_TOK = h.TOKENS[0]


def rule_event():
    p = {"checks": {"strict": False, "checks": [["ci", None]]},
         "reviews": {"count": 1, "dismiss_stale": False, "code_owners": False, "last_push": False, "dismissers": [],
                     "bypass": []},
         "push": None, "enforce_admins": False, "linear_history": False, "force_pushes": False, "deletions": False,
         "block_creations": False, "conversation_resolution": False, "lock_branch": False, "signatures": False,
         "merge_queue": None}
    return {"Rule": {"id": 1, "pattern": "main", "protection": p}}


def setup():
    h.setup(None)
    r = h.seed({"record": [rule_event()]})
    h.log(f"rule: {r}")


def at_once(fns):
    out = [None] * len(fns)

    def run(i):
        try:
            out[i] = fns[i]()
        except Exception as e:  # noqa: BLE001
            out[i] = e
    ts = [threading.Thread(target=run, args=(i,)) for i in range(len(fns))]
    for t in ts:
        t.start()
    for t in ts:
        t.join()
    return out


def j2(run, trials):
    out, done = h.resume(run)
    w = h.Work(run)
    owner = h.Work(f"{run}-owner")
    owner.env["GH_ENTERPRISE_TOKEN"] = OWNER_TOK
    d = f"{h.RAW}/{run}"
    R = f"{h.OWNER_LOGIN}/{h.REPO}"
    for i in range(done, trials):
        base = w.fetch_main()
        b = f"{run}-{i}-{int(time.time()) % 100000:05}"
        head = w.branch(b, {f"j2/{b}.txt": b})
        n = h.create_pr(w, d, f"{i:03}0-create", b)
        rc0, _, e0 = w.gh(d, f"{i:03}1-blocked", "pr", "merge", str(n), "-R", R, "--merge")
        t0 = time.time()

        def merge():
            tries = []
            while time.time() - t0 < 60:
                rc, _, e = w.gh(d, f"{i:03}3-merge-{len(tries)}", "pr", "merge", str(n), "-R", R, "--merge")
                tries.append({"rc": rc, "at": round(time.time() - t0, 2), "err": e.strip().splitlines()[-1:]})
                if rc == 0:
                    break
                time.sleep(0.3)
            return tries
        review, check, tries = at_once([
            lambda: owner.gh(d, f"{i:03}2-review", "pr", "review", str(n), "-R", R, "--approve", "-b", "lgtm")[0],
            lambda: h.status(head, "success")["state"],
            merge,
        ])
        view, lst = at_once([
            lambda: w.gh(d, f"{i:03}4-view", "pr", "view", str(n), "-R", R, "--json", "state,number,reviewDecision"),
            lambda: w.gh(d, f"{i:03}5-list", "pr", "list", "-R", R, "--state", "merged", "--json", "number,state",
                         "-L", "200"),
        ])
        tip = w.fetch_main()
        parents = w.show(tip, "%P").split()
        v_state = json.loads(view[1])["state"] if view[0] == 0 else None
        listed = view[0] == 0 and lst[0] == 0 and any(p["number"] == n and p["state"] == "MERGED"
                                                      for p in json.loads(lst[1]))
        merged_rc = tries[-1]["rc"] if isinstance(tries, list) and tries else None
        ok = (rc0 != 0 and review == 0 and check == "success" and merged_rc == 0 and v_state == "MERGED" and listed
              and parents[1:] == [head])
        rec = {"i": i, "n": n, "head": head, "base": base, "tip": tip, "parents": parents, "blocked_rc": rc0,
               "blocked": e0.strip().splitlines()[-1:], "review_rc": review, "check": check,
               "merge_tries": tries, "view": v_state, "listed": listed, "pass": ok, "at": time.time()}
        h.append(out, rec)
        h.log(f"j2 {i}: #{n} {'pass' if ok else 'FAIL'}; merge tries {len(tries) if isinstance(tries, list) else tries}"
              f" view {v_state} listed {listed}")
    rows = [json.loads(l) for l in open(out)]
    h.log(f"j2: {sum(r['pass'] for r in rows)} of {len(rows)} pass")


def j3(run, trials):
    out, done = h.resume(run)
    w = h.Work(run)
    d = f"{h.RAW}/{run}"
    R = f"{h.OWNER_LOGIN}/{h.REPO}"
    if done == 0:
        rc, _, e = w.gh(d, "0000-label", "label", "create", "bug", "-R", R, "-c", "d73a4a", "-d", "Something is wrong")
        rc2, _, e2 = w.gh(d, "0001-label-again", "label", "create", "bug", "-R", R, "-c", "d73a4a")
        rc3, _, e3 = w.gh(d, "0002-label-force", "label", "create", "bug", "-R", R, "-c", "d73a4a", "--force")
        h.log(f"label create: {rc} {e.strip()[-120:]}; again {rc2} {e2.strip()[-120:]}; --force {rc3} {e3.strip()[-200:]}")
    for i in range(done, trials):
        title = f"{run} {i} {int(time.time())}"
        rc, o, e = w.gh(d, f"{i:03}0-create", "issue", "create", "-R", R, "-t", title, "-b", "journey 3", "-l", "bug")
        n = int(o.strip().rsplit("/", 1)[-1]) if rc == 0 else None
        lst, api = at_once([
            lambda: w.gh(d, f"{i:03}1-list", "issue", "list", "-R", R, "--label", "bug", "--json", "number,labels",
                         "-L", "100"),
            lambda: w.gh(d, f"{i:03}2-api", "api", f"repos/{R}/issues?sort=updated&labels=bug&per_page=100"),
        ])
        in_list = lst[0] == 0 and any(x["number"] == n and any(l["name"] == "bug" for l in x["labels"])
                                      for x in json.loads(lst[1]))
        api_rows = json.loads(api[1]) if api[0] == 0 else []
        in_api = any(x["number"] == n for x in api_rows)
        first = bool(api_rows) and api_rows[0]["number"] == n
        ok = rc == 0 and in_list and in_api and first
        rec = {"i": i, "n": n, "create_rc": rc, "create_err": e.strip().splitlines()[-1:] if rc else [],
               "list_rc": lst[0], "in_list": in_list, "api_rc": api[0], "in_api": in_api, "api_first": first,
               "pass": ok, "at": time.time()}
        h.append(out, rec)
        h.log(f"j3 {i}: #{n} {'pass' if ok else 'FAIL'} list {in_list} api {in_api} first {first}")
    rows = [json.loads(l) for l in open(out)]
    h.log(f"j3: {sum(r['pass'] for r in rows)} of {len(rows)} pass")


if __name__ == "__main__":
    h.REPO = os.environ.get("WP3_REPO", "wp6j")
    cmd, rest = sys.argv[1], sys.argv[2:]
    if cmd == "setup":
        setup()
    elif cmd == "j2":
        j2(rest[0], int(rest[1]))
    elif cmd == "j3":
        j3(rest[0], int(rest[1]))
    else:
        sys.exit(__doc__)
