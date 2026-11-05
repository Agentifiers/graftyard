#!/usr/bin/env python3
"""M2-WP3's hosted gates on the graft-m1 test install: real `gh pr merge`, REST merges, and the merge queue through the
edge, with the `MergeQueue` consumer on the sites.

hosted.py setup REPO           the repository's `main`, its rule, and its settings (`/bench_seed {"record": ..}`)
hosted.py direct RUN           `gh pr merge --merge|--squash|--rebase` and REST `PUT /pulls/n/merge`, with gh's
                               replies captured (GH_DEBUG=api) for `tests/m2/gh-replies.py`
hosted.py bf23 RUN TRIALS      a required `ci` status passes, is re-requested (a newer pending status), and
                               `gh pr merge` is refused until a newer success, then merges
hosted.py queue RUN TRIALS [--kill-every K] [--fail I] [--bf1]
                               5 pull requests enqueued with `gh pr merge` on a queued base; this script plays CI on
                               each `gh-readonly-queue/*` tip through `POST /statuses/{sha}`, failing the I-th
                               (default 3); the base must move only by whole landings. Every K-th trial (default 10)
                               kills the leader mid-landing (`mqKill`). `--bf1` holds the landing (`mqHold`) and
                               commits it in one entry with a `PrSync` batch through the leader's `/commit_many`.
hosted.py bm2 RUN TRIALS       `Build` and `Land` commands whose `parent_tip` is stale, straight to the leader

Every trial is one JSON line in $WP3_RAW/RUN.jsonl; a restart picks up after the last line. The install comes from
the same environment as tests/m2/bf24/bf24.py, which this script copies its member helpers from.
"""
import base64, json, os, random, subprocess, sys, tempfile, threading, time, urllib.error, urllib.request

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "../../.."))
MAIN_ROOT = os.environ.get("GRAFT_MAIN_CHECKOUT", "/home/user/workspace/repo")
RAW = os.environ.get("WP3_RAW", f"{ROOT}/tests/m2/results/raw/m2-wp3")
EDGE = os.environ.get("GRAFT_EDGE", "https://graft-m1-edge.<workers-subdomain>.workers.dev")
HOST = EDGE.split("//")[1]
HOME = os.path.expanduser(os.environ.get("BF20_HOME", "~/.graft-integ"))
REPOS = json.load(open(f"{HOME}/repos.json"))
TOKENS = [l.split()[0] for l in open(f"{HOME}/tokens.txt").read().split("\n") if l.strip()]
OWNER_TOK, WRITER = TOKENS[0], TOKENS[1]
OWNER_LOGIN, OWNER_ID, WRITER_ID = "wp6", 7000001, 7000002
AUTH = "Authorization: Basic " + base64.b64encode(f"x:{WRITER}".encode()).decode()
M0 = open(f"{MAIN_ROOT}/tests/m0/.m0-token").read().strip()
WP7 = os.environ.get("GRAFT_TOOLS_EDGE", "https://graft-m1-wp7-edge.<workers-subdomain>.workers.dev")
BED = open(os.path.expanduser("~/.graft-wp6/bed.token")).read().strip()
GH = "/usr/bin/gh"
os.makedirs(RAW, exist_ok=True)
REPO = None  # set by main: the repository's name in repos.json


def log(m):
    print(f"{time.strftime('%H:%M:%S')} {m}", flush=True)


def chain():
    return f"repo:{REPOS[REPO]}"


def url():
    return f"{EDGE}/{OWNER_LOGIN}/{REPO}.git/"


def http(method, u, body=None, headers=None, timeout=60):
    """(status, parsed body or text, headers)."""
    h = {"user-agent": "graftyard-wp3", **(headers or {})}
    data = None if body is None else json.dumps(body).encode()
    if data is not None:
        h.setdefault("content-type", "application/json")
    req = urllib.request.Request(u, data=data, headers=h, method=method)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            raw, status, hd = r.read(), r.status, dict(r.headers)
    except urllib.error.HTTPError as e:
        raw, status, hd = e.read(), e.code, dict(e.headers)
    try:
        return status, json.loads(raw), hd
    except ValueError:
        return status, raw.decode(errors="replace"), hd


def member(site, path, body, gen=1, deadline=20000):
    b = {"chain": chain(), "site": site, "path": path, "deadline": deadline, "gen": gen}
    if body is not None:
        b["body"] = body
    s, v, _ = http("POST", f"{WP7}/member", b, {"authorization": f"Bearer {M0}"}, timeout=deadline / 1000 + 20)
    if s != 200 or not isinstance(v, dict):
        raise RuntimeError(f"member {site} {path}: {s} {str(v)[:300]}")
    return v


def leader_site():
    for _ in range(20):
        best = None
        for s in range(3):
            try:
                v = member(s, "/status", None).get("v") or {}
            except Exception as e:
                log(f"status {s}: {e}")
                continue
            if v.get("role") == "leader" and (best is None or v.get("term", 0) > best[1]):
                best = (s, v.get("term", 0))
        if best:
            return best[0]
        # An idle group's objects sleep, and `/status` does not wake them: a client read through the edge does.
        http("GET", f"{EDGE}/{OWNER_LOGIN}/{REPO}.git/info/refs?service=git-upload-pack", None,
             {"authorization": AUTH.split(": ", 1)[1]}, timeout=60)
        time.sleep(1)
    raise RuntimeError("no leader")


def leader_read(q):
    for _ in range(10):
        s = leader_site()
        try:
            v = member(s, "/read", {"q": q})
            if "value" in v.get("v", {}):
                return v["v"]["value"]
        except Exception as e:
            log(f"read: {e}")
        time.sleep(0.5)
    raise RuntimeError(f"read {q}")


def seed(body):
    s = leader_site()
    return member(s, "/bench_seed", body).get("v")


def bed(path, **q):
    qs = "&".join(f"{k}={v}" for k, v in q.items())
    return http("GET" if path in ("/_bed/log", "/_bed/refs") else "POST", f"{EDGE}{path}?repo={REPOS[REPO]}&{qs}",
                None, {"x-bed-token": BED}, timeout=120)


def member_log(frm):
    s, v, _ = bed("/_bed/log", **{"from": frm})
    if s != 200:
        raise RuntimeError(f"/_bed/log: {s} {str(v)[:200]}")
    return v


def api(method, path, body=None, tok=None):
    return http(method, f"{EDGE}/api/v3{path}", body, {"authorization": f"token {tok or WRITER}"})


def gql(query, variables=None, tok=None):
    s, v, _ = http("POST", f"{EDGE}/api/graphql", {"query": query, "variables": variables or {}},
                   {"authorization": f"bearer {tok or WRITER}"})
    return s, v


# ---- git and gh ----

class Work:
    """A work tree on the repository, with gh's environment."""

    def __init__(self, name):
        self.dir = f"{RAW}/{name}.work"
        self.home = f"{RAW}/{name}.home"
        os.makedirs(self.home, exist_ok=True)
        self.env = dict(os.environ, HOME=self.home, GH_CONFIG_DIR=f"{self.home}/gh", GH_HOST=HOST,
                        GH_ENTERPRISE_TOKEN=WRITER, GH_PROMPT_DISABLED="1", GH_NO_UPDATE_NOTIFIER="1",
                        GIT_TERMINAL_PROMPT="0", GIT_AUTHOR_NAME="wp3", GIT_AUTHOR_EMAIL="wp3@graftyard.invalid",
                        GIT_COMMITTER_NAME="wp3", GIT_COMMITTER_EMAIL="wp3@graftyard.invalid")
        if not os.path.isdir(self.dir):
            os.makedirs(self.dir)
            self.git("init", "-q")
            self.git("remote", "add", "origin", url())

    def git(self, *a, check=True, timeout=120):
        r = subprocess.run(["git", "-c", "credential.helper=", "-c", f"http.extraHeader={AUTH}", "-c",
                            "init.defaultBranch=main", *a], cwd=self.dir, env=self.env, capture_output=True,
                           text=True, timeout=timeout)
        if check and r.returncode != 0:
            raise RuntimeError(f"git {a[0]}: {r.returncode} {r.stderr.strip()[:400]}")
        return r.stdout.strip()

    def gh(self, out_dir, tag, *a, timeout=120):
        """gh with GH_DEBUG=api, its stderr in out_dir/tag.err for gh-replies.py. (rc, stdout, stderr)."""
        os.makedirs(out_dir, exist_ok=True)
        env = dict(self.env, GH_DEBUG="api")
        t0 = time.time()
        r = subprocess.run([GH, *a], cwd=self.dir, env=env, capture_output=True, text=True, timeout=timeout)
        open(f"{out_dir}/{tag}.cmd", "w").write("+ gh " + " ".join(a) + "\n")
        open(f"{out_dir}/{tag}.out", "w").write(r.stdout)
        open(f"{out_dir}/{tag}.err", "w").write(r.stderr)
        with open(f"{out_dir}/summary.txt", "a") as f:
            f.write(f"{tag} rc={r.returncode} {time.time() - t0:.1f}s gh {' '.join(a)}\n")
        return r.returncode, r.stdout, r.stderr

    def remote(self):
        o = self.git("ls-remote", url(), check=False)
        return {l.split("\t")[1]: l.split("\t")[0] for l in o.splitlines() if "\t" in l}

    def fetch_main(self):
        self.git("fetch", "-q", "-f", "origin", "refs/heads/main:refs/remotes/origin/main")
        return self.git("rev-parse", "origin/main")

    def branch(self, name, files, on="origin/main", commits=1):
        """A branch from `on` with `commits` commits writing `files`, pushed. Its head."""
        self.git("checkout", "-q", "-B", name, on)
        for c in range(commits):
            for p, text in files.items():
                os.makedirs(os.path.dirname(f"{self.dir}/{p}") or self.dir, exist_ok=True)
                open(f"{self.dir}/{p}", "w").write(f"{text} {c}\n")
            self.git("add", "-A")
            self.git("commit", "-qm", f"{name} {c}")
        self.git("push", "-q", "-f", "origin", f"HEAD:refs/heads/{name}")
        return self.git("rev-parse", "HEAD")

    def show(self, oid, fmt):
        return self.git("show", "-s", f"--format={fmt}", oid)


def create_pr(w, out_dir, tag, head):
    rc, out, err = w.gh(out_dir, tag, "pr", "create", "-R", f"{OWNER_LOGIN}/{REPO}", "-H", head, "-B", "main", "-t",
                        f"wp3 {head}", "-b", "hosted M2-WP3")
    if rc != 0:
        raise RuntimeError(f"gh pr create {head}: {rc} {err[-400:]}")
    return int(out.strip().rsplit("/", 1)[-1])


def status(sha, state, context="ci"):
    s, v, _ = api("POST", f"/repos/{OWNER_LOGIN}/{REPO}/statuses/{sha}", {"state": state, "context": context})
    if s != 201:
        raise RuntimeError(f"status {state} on {sha[:8]}: {s} {str(v)[:300]}")
    return v


def rest_case(out_dir, tag, method, path, status_, body):
    """A REST reply as a validate.py case."""
    os.makedirs(out_dir, exist_ok=True)
    json.dump({"method": method, "path": path, "status": status_, "body": body, "location": None},
              open(f"{out_dir}/{tag}.rest.json", "w"), indent=1)


# ---- setup ----

def rid():
    # A ULID: 48 bits of ms, 80 random.
    v = (int(time.time() * 1000) << 80) | random.getrandbits(80)
    a = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"
    return "".join(a[(v >> (5 * (25 - i))) & 31] for i in range(26))


def refname(s):
    return list(s.encode())


def rule_event(queue):
    p = {"checks": {"strict": False, "checks": [["ci", None]]}, "reviews": None, "push": None,
         "enforce_admins": False, "linear_history": False, "force_pushes": False, "deletions": False,
         "block_creations": False, "conversation_resolution": True, "lock_branch": False, "signatures": False,
         "merge_queue": None}
    if queue:
        p["merge_queue"] = {"check_timeout_minutes": 60, "grouping": "HeadGreen", "max_build": 5, "min_merge": 5,
                            "max_merge": 5, "min_wait_minutes": 1, "method": "Merge"}
    return {"Rule": {"id": 1, "pattern": "main", "protection": p}}


def settings_event():
    return {"Settings": {"owner": OWNER_ID, "enterprise": None, "visibility": "Private",
                         "default_branch": refname("refs/heads/main"), "archived": False, "description": None,
                         "homepage": None, "topics": [], "has_issues": True, "has_wiki": False, "has_projects": False,
                         "merge_methods": ["Merge", "Squash", "Rebase"], "delete_branch_on_merge": False,
                         "allow_auto_merge": False, "allow_update_branch": False, "check_retention_days": 90}}


def setup(rule):
    w = Work(f"setup-{REPO}")
    if "refs/heads/main" not in w.remote():
        open(f"{w.dir}/README", "w").write("wp3\n")
        w.git("add", "-A")
        w.git("commit", "-qm", "base")
        w.git("push", "-q", "origin", "HEAD:refs/heads/main")
    evs = [settings_event()] + ([rule_event(rule == "queue")] if rule else [])
    r = seed({"record": evs})
    log(f"record {[list(e)[0] for e in evs]}: {r}")
    if not r or "recorded" not in r:
        raise RuntimeError(f"record: {r}")


def resume(run):
    out = f"{RAW}/{run}.jsonl"
    done = sum(1 for _ in open(out)) if os.path.exists(out) else 0
    return out, done


def append(out, rec):
    with open(out, "a") as f:
        f.write(json.dumps(rec) + "\n")


# ---- direct merges ----

def direct(run):
    """On a repository with no rule: gh pr merge with each method, then REST merges and their refusals."""
    out_dir = f"{RAW}/{run}"
    w = Work(run)
    res = []
    for i, method in enumerate(["merge", "squash", "rebase"]):
        base = w.fetch_main()
        head = w.branch(f"{run}-{method}", {f"{method}.txt": f"{run} {method}"}, commits=2)
        n = create_pr(w, out_dir, f"{i}0-create", f"{run}-{method}")
        rc, o, e = w.gh(out_dir, f"{i}1-merge-{method}", "pr", "merge", str(n), "-R", f"{OWNER_LOGIN}/{REPO}",
                        f"--{method}")
        tip = w.fetch_main()
        parents = w.show(tip, "%P").split()
        tree_ok = w.git("diff", "--quiet", f"{head}", tip, "--", f"{method}.txt", check=False) == ""
        rc2, view, _ = w.gh(out_dir, f"{i}2-view", "pr", "view", str(n), "-R", f"{OWNER_LOGIN}/{REPO}", "--json",
                            "state,number")
        state = json.loads(view)["state"] if rc2 == 0 else None
        if method == "merge":
            shape = parents == [base, head]
        elif method == "squash":
            shape = parents == [base] and w.show(tip, "%T") == w.git("rev-parse", f"{head}^{{tree}}")
        else:
            # Two replayed commits on the base, the last with the head's tree, none of them the head's own.
            shape = (w.show(tip, "%T") == w.git("rev-parse", f"{head}^{{tree}}") and tip != head
                     and w.show(f"{tip}~1", "%P").split() == [base])
        r = {"method": method, "n": n, "rc": rc, "base": base, "head": head, "tip": tip, "parents": parents,
             "shape": shape, "state": state, "stderr": e.strip().splitlines()[-1:] if rc else []}
        log(f"gh pr merge --{method}: {r}")
        res.append(r)
    # REST: a merge, then the same pull request again (405), a stale `sha` (409), and a reader (403/404).
    w.fetch_main()
    w.branch(f"{run}-rest", {"rest.txt": run})
    n = create_pr(w, out_dir, "30-create", f"{run}-rest")
    head = w.git("rev-parse", "HEAD")
    p = f"/repos/{OWNER_LOGIN}/{REPO}/pulls/{n}/merge"
    tpl = "/repos/{owner}/{repo}/pulls/{pull_number}/merge"
    rest = []
    s, v, _ = api("PUT", p, {"merge_method": "merge", "sha": "0" * 40})
    rest.append(("stale sha", s, v))
    s, v, _ = api("PUT", p, {"merge_method": "squash"}, tok=TOKENS[2])
    rest.append(("reader", s, v))
    s, v, _ = api("PUT", p, {"merge_method": "squash", "sha": head})
    rest.append(("merge", s, v))
    if s == 200:
        rest_case(out_dir + "-rest", "rest-merge", "PUT", tpl, s, v)
    s, v, _ = api("PUT", p, {"merge_method": "squash"})
    rest.append(("again", s, v))
    for name, s, v in rest:
        log(f"REST {name}: {s} {str(v)[:160]}")
    tip = w.fetch_main()
    rec = {"gh": res, "rest": [(n_, s, v) for n_, s, v in rest], "rest_tip": tip,
           "rest_tip_tree_has": w.git("ls-tree", "--name-only", tip, check=False).split()}
    json.dump(rec, open(f"{RAW}/{run}.json", "w"), indent=1)
    ok = all(r["rc"] == 0 and r["shape"] and r["state"] == "MERGED" for r in res)
    want = {"stale sha": 409, "reader": None, "merge": 200, "again": 405}
    ok_rest = all(want[n_] is None and s in (403, 404) or want[n_] == s for n_, s, _ in rest)
    log(f"direct: gh {'pass' if ok else 'FAIL'}, REST {'pass' if ok_rest else 'FAIL'}")


# ---- BF-23 ----

def bf23(run, trials, workers=4):
    """On a repository whose `main` requires `ci` with no queue. Each trial: a pull request, `ci` success, then a newer
    pending `ci` (the re-request); `gh pr merge` must fail and so must the edge's own `mergePullRequest`, with the pull
    request still open and none of its changes on `main`; then a newer success, and `gh pr merge` merges. `workers`
    trials run at once, each in its own work tree; a final merge that races another's and is refused "Base branch was
    modified" is tried again, as a client would."""
    out, done = resume(run)
    lock = threading.Lock()
    nxt = [done]

    def worker(k):
        w = Work(f"{run}-w{k}")
        while True:
            with lock:
                i = nxt[0]
                if i >= trials:
                    return
                nxt[0] += 1
            for attempt in range(3):
                try:
                    rec = bf23_trial(w, f"{RAW}/{run}", run, i)
                    break
                except Exception as e:
                    log(f"trial {i}: {e}; again")
                    time.sleep(5)
            else:
                rec = {"i": i, "pass": False, "error": "three attempts failed"}
            with lock:
                append(out, rec)
            log(f"{i} {'pass' if rec['pass'] else 'FAIL'} refused_rc={rec.get('refused_rc')} "
                f"merged_rc={rec.get('merged_rc')} retries={rec.get('retries')}")

    ts = [threading.Thread(target=worker, args=(k,)) for k in range(workers)]
    for t in ts:
        t.start()
    for t in ts:
        t.join()


def bf23_trial(w, out_dir, run, i):
    method = ["merge", "squash", "rebase"][i % 3]
    # Only the first trials keep gh's replies for the validators; later ones keep the summary line only.
    d = out_dir if i < 3 else f"{tempfile.gettempdir()}/wp3-bf23"
    w.fetch_main()
    b = f"{run}-{i}-{int(time.time()) % 100000:05}"
    path = f"b/{b}.txt"
    head = w.branch(b, {path: b})
    n = create_pr(w, d, f"{i:03}0-create", b)
    status(head, "success")
    status(head, "pending")

    def landed():
        tip = w.fetch_main()
        return subprocess.run(["git", "cat-file", "-e", f"{tip}:{path}"], cwd=w.dir, capture_output=True).returncode == 0

    rc1, _, e1 = w.gh(d, f"{i:03}1-refused", "pr", "merge", str(n), "-R", f"{OWNER_LOGIN}/{REPO}", f"--{method}")
    after_refusal = landed()
    # The edge's mutation, past gh's local mergeStateStatus check: `decide` itself must refuse.
    s, v = gql("mutation($id: ID!) { mergePullRequest(input: {pullRequestId: $id}) { clientMutationId } }",
               {"id": pr_node(n)})
    direct_refused = s == 200 and (v.get("data") or {}).get("mergePullRequest") is None and bool(v.get("errors"))
    after_direct = landed()
    open_after = pr_state(n) == "OPEN"
    status(head, "success")
    retries = 0
    while True:
        rc2, _, e2 = w.gh(d, f"{i:03}2-merged-{retries}", "pr", "merge", str(n), "-R", f"{OWNER_LOGIN}/{REPO}",
                          f"--{method}")
        if rc2 == 0 or "Base branch was modified" not in e2 or retries >= 5:
            break
        retries += 1
    ok = (rc1 != 0 and not after_refusal and direct_refused and not after_direct and open_after and rc2 == 0
          and landed() and pr_state(n) == "MERGED")
    return {"i": i, "n": n, "method": method, "head": head, "refused_rc": rc1,
            "refused": e1.strip().splitlines()[:1], "direct": v.get("errors") if direct_refused else v,
            "merged_rc": rc2, "merged_err": e2.strip().splitlines()[-1:] if rc2 else [], "retries": retries,
            "pass": ok, "at": time.time()}


def pr_state(n):
    s, v = gql("query($o: String!, $r: String!, $n: Int!) { repository(owner: $o, name: $r) { pullRequest(number: $n)"
               " { state } } }", {"o": OWNER_LOGIN, "r": REPO, "n": n})
    return v["data"]["repository"]["pullRequest"]["state"]


def pr_node(n):
    s, v = gql("query($o: String!, $r: String!, $n: Int!) { repository(owner: $o, name: $r) { pullRequest(number: $n)"
               " { id } } }", {"o": OWNER_LOGIN, "r": REPO, "n": n})
    return v["data"]["repository"]["pullRequest"]["id"]


# ---- BF-21 and BF-1 ----

def queue(run, trials, kill_every, fail, bf1, offset=0):
    """Trials `offset..offset+trials` of a run split over repositories: trial i kills when its global index is the
    last of each `kill_every`."""
    out, done = resume(run)
    w = Work(run)
    for i in range(done, trials):
        kill = kill_every > 0 and (offset + i) % kill_every == kill_every - 1
        try:
            rec = queue_trial(w, run, i, kill, fail, bf1)
        except Exception as e:
            log(f"trial {i}: {e!r}; recovering")
            rec = {"i": i, "error": repr(e)[:400], "pass": False}
            # A failed trial leaves queued pull requests: they go before the next one starts.
            drain(w, run, i)
        append(out, rec)
        log(f"{i} {'pass' if rec.get('pass') else 'FAIL'} kill={kill} {rec.get('summary', rec.get('error'))}")


KILLS = []


def view():
    return leader_read({"mergequeue": refname("refs/heads/main")})


def queue_trial(w, run, i, kill, fail, bf1):
    out_dir = f"{RAW}/{run}" if i < 2 else f"{tempfile.gettempdir()}/wp3-q"
    t0 = time.time()
    base = w.fetch_main()
    start = int(member(leader_site(), "/status", None)["v"].get("commit", 0))
    tag = format(int(time.time()) % 100000, "05")
    names = [f"{run}-{i}-{tag}-{k}" for k in range(1, 6)]
    for k, b in enumerate(names, 1):
        w.branch(b, {f"q/{i}/{k}.txt": f"{run} {i} {k} {time.time()}"})
    prs = parallel([lambda k=k, b=b: create_pr(w, out_dir, f"{i:03}{k}-create", b) for k, b in enumerate(names, 1)])
    failing = prs[fail - 1] if fail else None
    killed = None
    if kill:
        killed = {"site": leader_site(), "mode": ["after_quorum", "before_commit"][len(KILLS) % 2]}
        KILLS.append(i)
        # Down past ELECTION_AFTER (10 s), so a follower is elected and finishes, or redoes, the landing.
        killed["r"] = seed({"mqKill": {"kill": killed["mode"], "down_ms": 25000}})
    if bf1:
        bys = bystanders(w, run)
        seed({"mqHold": 1})
    enq = parallel([lambda k=k, n=n: w.gh(out_dir, f"{i:03}{k}-enqueue", "pr", "merge", str(n), "-R",
                                          f"{OWNER_LOGIN}/{REPO}", "--merge") for k, n in enumerate(prs, 1)])
    for n, (rc, o, e) in zip(prs, enq):
        if rc != 0:
            raise RuntimeError(f"enqueue #{n}: {rc} {e[-300:]}")
    # CI: a status on every built entry's tip, once each; the failing PR's tip fails, and the later tips (which hold
    # its changes) do too, as real CI on a queue ref would.
    reported, landed, held, rebuilt = {}, set(), None, False
    deadline = t0 + 600
    while time.time() < deadline:
        v = view()["view"]
        entries = [e for e in v["entries"] if e["n"] in prs]
        bad, todo = False, []
        for e in entries:
            if not e["built"]:
                break
            tip = bytes(e["built"]["entry"]["group_tip"]).hex()
            bad |= e["n"] == failing
            if tip not in reported:
                reported[tip] = "failure" if bad else "success"
                todo.append(tip)
        # The newest tip first, so under HeadGreen a passing group can land in one QueueMerge.
        for tip in reversed(todo):
            status(tip, reported[tip])
        if bf1 and held is None:
            h = seed({"mqHeld": True}).get("held") or []
            if h:
                held = h[0]
                bf1_rec = bf1_commit(w, held, bys)
        if not entries:
            break
        time.sleep(2)
    tip = w.fetch_main()
    if killed:
        killed["fired"] = fired(killed["site"])
        killed["after"] = statuses()
    log_ = member_log(start + 1)
    audit = audit_base(log_, base, prs)
    states = {}
    for n in prs:
        s, v = gql("query($o: String!, $r: String!, $n: Int!) { repository(owner: $o, name: $r) { pullRequest(number:"
                   " $n) { state isInMergeQueue } } }", {"o": OWNER_LOGIN, "r": REPO, "n": n})
        states[n] = v["data"]["repository"]["pullRequest"]
    merged = sorted(n for n, s in states.items() if s["state"] == "MERGED")
    want = sorted(n for n in prs if n != failing)
    ok = (audit["ok"] and merged == want and audit["landed"] == want
          and audit["ejected"] == ([failing] if failing else []))
    if failing:
        ok = ok and not states[failing]["isInMergeQueue"] and states[failing]["state"] == "OPEN"
    rec = {"i": i, "prs": prs, "failing": failing, "base": base, "tip": tip, "kill": killed, "merged": merged,
           "audit": audit, "secs": round(time.time() - t0, 1), "pass": ok,
           "summary": f"landed {audit['groups']} in {audit['landings']} landings, ejected {audit['ejected']}, "
                      f"{round(time.time() - t0)} s"}
    if bf1:
        rec["bf1"] = bf1_rec if held else None
        rec["pass"] = ok and bool(held) and rec["bf1"]["ok"]
    return rec


def parallel(fns):
    """Each function on its own thread; their results in order, or the first exception."""
    out = [None] * len(fns)

    def one(k):
        try:
            out[k] = (True, fns[k]())
        except Exception as e:
            out[k] = (False, e)

    ts = [threading.Thread(target=one, args=(k,)) for k in range(len(fns))]
    for t in ts:
        t.start()
    for t in ts:
        t.join()
    for ok, v in out:
        if not ok:
            raise v
    return [v for _, v in out]


def fired(site):
    """The kills this member's fault table recorded, newest last."""
    try:
        v = member(site, "/faults", None).get("v") or []
        return [{k: f.get(k) for k in ("mode", "index", "term", "at")} for f in v][-1:]
    except Exception as e:
        return str(e)


def statuses():
    out = {}
    for s in range(3):
        try:
            v = member(s, "/status", None).get("v") or {}
            out[s] = [v.get("role"), v.get("term"), v.get("commit")]
        except Exception as e:
            out[s] = str(e)[:80]
    return out


def drain(w, run, i):
    """Dequeues every pull request left on the base, so the next trial starts with an empty queue."""
    try:
        for e in view()["view"]["entries"]:
            s, v = gql("mutation($id: ID!) { dequeuePullRequest(input: {id: $id}) { clientMutationId } }",
                       {"id": pr_node(e["n"])})
            log(f"dequeue #{e['n']}: {s} {str(v)[:120]}")
    except Exception as e:
        log(f"drain: {e}")


def audit_base(entries, base, prs):
    """BF-21 over the leader's log from the trial's start: `main` moves only in entries whose events are `Merged`s
    then `Landed`, each move from the previous tip, and the landed set is the group minus the ejected one."""
    tip, landed, ejected, landings, bad, groups = base, [], [], 0, [], []
    for e in entries:
        if not e.get("committed"):
            continue
        kinds = e.get("kinds") or []
        moves = [m for m in e.get("branchMoves") or [] if m[1] == "refs/heads/main"]
        for k in kinds:
            if k[0] == "Queue" and k[1].startswith("Ejected"):
                n = int(k[1].split()[1])
                if n in prs:
                    ejected.append(n)
        if not moves:
            continue
        ev_kinds = [kinds[m[0]][0] for m in moves]
        if any(k != "Merged" for k in ev_kinds):
            bad.append(f"entry {e['index']}: main moved by {ev_kinds}")
        landed_here = [kinds[m[0]][1] for m in moves]
        if not any(k[0] == "Queue" and k[1].startswith("Landed") for k in kinds):
            bad.append(f"entry {e['index']}: Merged without Landed")
        for m in moves:
            if m[2] != tip:
                bad.append(f"entry {e['index']}: main moved from {m[2]} while at {tip}")
            tip = m[3]
        landings += 1
        groups.append(landed_here)
        landed.extend(n for n in landed_here if n in prs)
    return {"ok": not bad, "bad": bad, "landed": sorted(landed), "ejected": sorted(set(ejected)), "landings": landings,
            "groups": groups, "tip": tip}


def bystanders(w, run):
    """Five open pull requests outside the queue, made once per run, whose heads BF-1's `PrSync` batch moves."""
    path = f"{RAW}/{run}.bystanders.json"
    if os.path.exists(path):
        return json.load(open(path))
    out = []
    for k in range(1, 6):
        b = f"{run}-by-{k}"
        w.fetch_main()
        w.branch(b, {f"by/{k}.txt": f"{run} {k} {time.time()}"})
        out.append([b, create_pr(w, f"{RAW}/{run}", f"by{k}-create", b)])
    json.dump(out, open(path, "w"))
    return out


def bf1_commit(w, held, bys):
    """BF-1: with the group's landing held, the leader drops its next `PrSync` first attempt, five bystander heads go
    up in one push, and the landing and the `PrSync` batch for those heads (what the dropped attempt would have sent,
    without its cursor move) go to the leader's `/commit_many` as two commands, so they share one entry. The entry
    must hold both request ids, five `Merged`, `Landed`, and five `refs/pull/N/head` moves, and be the same committed
    entry on all three members."""
    s = leader_site()
    member(s, "/bench_seed", {"prsyncDrop": 1})
    heads = []
    for b, n in bys:
        w.git("checkout", "-q", b)
        open(f"{w.dir}/by.txt", "w").write(f"{b} {time.time()}\n")
        w.git("add", "-A")
        w.git("commit", "-qm", f"{b} again")
        heads.append([n, list(bytes.fromhex(w.git("rev-parse", "HEAD")))])
    w.git("push", "-q", "-f", "origin", *[f"{b}:refs/heads/{b}" for b, _ in bys])
    st = member(s, "/status", None)["v"]
    sync_rid = rid()
    sync = {"prSync": {"from": 0, "through": None, "heads": heads, "merges": [], "unmerged": [], "request": sync_rid}}
    body = {"cmds": [{"rid": held["rid"], "cmd": held["cmd"]}, {"rid": sync_rid, "cmd": sync}], "hold_ms": 20000}
    r = member(s, "/commit_many", body, deadline=60000)
    results = (r.get("v") or {}).get("results")
    seed({"mqHold": 0})
    time.sleep(3)
    per = {}
    for site in range(3):
        try:
            lg = member(site, f"/log?from={max(1, st.get('commit', 1) - 5)}&n=200", None).get("v") or []
        except Exception as e:
            per[site] = str(e)
            continue
        both = [e for e in lg if held["rid"] in e.get("requests", []) and sync_rid in e.get("requests", [])]
        per[site] = [{"index": e["index"], "term": e["term"], "digest": e["digest"], "committed": e["committed"],
                      "kinds": [k[0] if k[0] != "Queue" else k[1].split()[0] for k in e.get("kinds", [])],
                      "ref_updates": e.get("ref_updates")} for e in both]
    lists = [v for v in per.values() if isinstance(v, list)]
    one = lists[0][0] if lists and lists[0] else None
    plans = len(held["cmd"]["merge"]["cmd"]["land"]["plans"])
    ok = (len(lists) == 3 and all(len(v) == 1 for v in lists)
          and len({(v[0]["index"], v[0]["digest"]) for v in lists}) == 1 and all(v[0]["committed"] for v in lists)
          and one["kinds"].count("Merged") == plans == 5 and "Landed" in one["kinds"] and one["kinds"].count("Refs") == 1)
    return {"leader": s, "results": results, "per_member": per, "plans": plans, "ok": ok}


# ---- the per-base timer ----

def timer(run, trials):
    """Two pull requests enqueued under `min_merge` 5 and `min_wait_minutes` 1: nothing builds until the timer from
    `Step::Idle { at }` fires, about 60 s after the first `Enqueued`, with no entry in between. Then CI passes both
    tips and they land. Each trial records when the group built against the first `Enqueued`, on the leader's clock."""
    out, done = resume(run)
    w = Work(run)
    for i in range(done, trials):
        start = int(member(leader_site(), "/status", None)["v"].get("commit", 0))
        tag = format(int(time.time()) % 100000, "05")
        names = [f"{run}-{i}-{tag}-{k}" for k in (1, 2)]
        for k, b in enumerate(names, 1):
            w.branch(b, {f"t/{i}/{k}.txt": f"{run} {i} {k} {time.time()}"})
        prs = parallel([lambda b=b: create_pr(w, f"{tempfile.gettempdir()}/wp3-t", b, b) for b in names])
        for n in prs:
            rc, _, e = w.gh(f"{tempfile.gettempdir()}/wp3-t", f"enq-{n}", "pr", "merge", str(n), "-R",
                            f"{OWNER_LOGIN}/{REPO}", "--merge")
            if rc != 0:
                raise RuntimeError(f"enqueue #{n}: {e[-300:]}")
        reported = set()
        t0 = time.time()
        while time.time() - t0 < 300:
            v = view()["view"]
            es = [e for e in v["entries"] if e["n"] in prs]
            for e in es:
                if e["built"]:
                    tip = bytes(e["built"]["entry"]["group_tip"]).hex()
                    if tip not in reported:
                        reported.add(tip)
                        status(tip, "success")
            if not es:
                break
            time.sleep(2)
        lg = member_log(start + 1)
        enq = [e["at"] for e in lg if any(k[0] == "Queue" and k[1].startswith("Enqueued") for k in e.get("kinds", []))]
        built = [(e["at"], e["index"]) for e in lg
                 if any(k[0] == "Queue" and k[1].startswith("GroupBuilt") for k in e.get("kinds", []))]
        between = [e["index"] for e in lg if enq and built and enq[-1] < e["at"] < built[0][0]
                   and any(k[0] in ("Queue", "Refs", "RequiredCheck") for k in e.get("kinds", []))]
        wait_s = round((built[0][0] - enq[0]) / 1000, 1) if enq and built else None
        merged = all(pr_state(n) == "MERGED" for n in prs)
        ok = wait_s is not None and 60 <= wait_s <= 90 and not between and merged
        rec = {"i": i, "prs": prs, "wait_s": wait_s, "entries_between": between, "merged": merged, "pass": ok}
        append(out, rec)
        log(f"{i} {'pass' if ok else 'FAIL'} built {wait_s} s after the first Enqueued, merged={merged}")


# ---- BM-2 ----

def bm2(run, trials):
    """A queue with two built entries; each trial sends the leader a `Build` chained to a stale `parent_tip`, a `Build`
    whose chain breaks inside, and a `Land` whose second plan's `base_old` is stale. Each must be refused
    `StaleChain`, and the queue must not change."""
    out, done = resume(run)
    for i in range(done, trials):
        rec = bm2_trial(i)
        append(out, rec)
        log(f"{i} {'pass' if rec['pass'] else 'FAIL'} {rec['errs']}")


BM2_STATE = {}


def bm2_prepare():
    """Three queued pull requests, the first two built and held there: CI never reports, the timeout is an hour."""
    w = Work("bm2")
    v = view()["view"]
    if len([e for e in v["entries"] if e["built"]]) >= 2:
        return v
    for k in range(1, 4):
        b = f"bm2-{k}"
        w.fetch_main()
        w.branch(b, {f"bm2/{k}.txt": f"bm2 {k}"})
        n = create_pr(w, f"{RAW}/bm2-setup", f"{k}-create", b)
        rc, _, e = w.gh(f"{RAW}/bm2-setup", f"{k}-enqueue", "pr", "merge", str(n), "-R", f"{OWNER_LOGIN}/{REPO}",
                        "--merge")
        if rc != 0:
            raise RuntimeError(f"enqueue: {e[-300:]}")
    for _ in range(60):
        v = view()["view"]
        if len([e for e in v["entries"] if e["built"]]) >= 2:
            return v
        time.sleep(2)
    raise RuntimeError(f"not built: {v}")


def bm2_trial(i):
    v = BM2_STATE.get("v") or bm2_prepare()
    BM2_STATE["v"] = v
    built = [e for e in v["entries"] if e["built"]]
    es = [e["built"]["entry"] for e in built]
    stale = list(random.randbytes(20))
    # Under max_build 5, all 3 are built: chain a copy of the last onto a stale tip, and break the chain inside.
    third = dict(es[-1], parent_tip=stale if i % 2 == 0 else es[0]["group_tip"])
    if third["parent_tip"] == es[-1]["parent_tip"]:
        third["parent_tip"] = stale
    inner = [dict(e) for e in es]
    inner[1]["parent_tip"] = stale
    pack = {"inline": base64.b64encode(b"").decode()}
    cmds = [
        {"build": {"base": refname("refs/heads/main"), "entries": [third], "pack": pack, "actor": None}},
        {"build": {"base": refname("refs/heads/main"), "entries": inner, "pack": pack, "actor": None}},
    ]
    plans = [[e["pr"], {"base_old": e["parent_tip"], "head": list(bytes.fromhex(heads_of(e["pr"]))),
                        "commit": e["group_tip"], "pack": pack, "method": "Merge", "up_to_date": True,
                        "verified": [0] * 32, "engine": "bm2"}] for e in es[:2]]
    plans[1][1]["base_old"] = stale
    cmds.append({"land": {"plans": plans, "entry": es[1]}})
    errs = []
    s = leader_site()
    for c in cmds:
        body = {"merge": {"format": [32, 4], "cmd": c, "render": None}}
        r = member(s, "/commit", {"rid": rid(), "cmd": body}, deadline=30000).get("v") or {}
        ref = ((r.get("done") or {}).get("refused") or {}).get("merge", {}).get("refusal")
        errs.append(ref["err"] if ref else r)
    after = view()["view"]
    same = [e["built"]["entry"] for e in after["entries"] if e["built"]] == es
    ok = all(e == "StaleChain" for e in errs) and same
    return {"i": i, "errs": errs, "queue_unchanged": same, "pass": ok, "at": time.time()}


_HEADS = {}


def heads_of(n):
    if n not in _HEADS:
        s, v = gql("query($o: String!, $r: String!, $n: Int!) { repository(owner: $o, name: $r) { pullRequest(number:"
                   " $n) { headRefOid } } }", {"o": OWNER_LOGIN, "r": REPO, "n": n})
        _HEADS[n] = v["data"]["repository"]["pullRequest"]["headRefOid"]
    return _HEADS[n]


def main():
    global REPO
    cmd, rest = sys.argv[1], sys.argv[2:]
    REPO = os.environ.get("WP3_REPO", {"direct": "wp3m", "bf23": "wp3c", "queue": "wp3q", "bm2": "wp3b"}.get(cmd, ""))
    if cmd == "setup":
        REPO = rest[0]
        setup(rest[1] if len(rest) > 1 else None)
    elif cmd == "direct":
        direct(rest[0])
    elif cmd == "bf23":
        bf23(rest[0], int(rest[1]))
    elif cmd == "queue":
        a = rest[2:]
        k = int(a[a.index("--kill-every") + 1]) if "--kill-every" in a else 10
        f = int(a[a.index("--fail") + 1]) if "--fail" in a else 3
        o = int(a[a.index("--offset") + 1]) if "--offset" in a else 0
        queue(rest[0], int(rest[1]), k, f, "--bf1" in a, o)
    elif cmd == "timer":
        timer(rest[0], int(rest[1]))
    elif cmd == "bm2":
        bm2(rest[0], int(rest[1]))
    else:
        sys.exit(__doc__)


main()
