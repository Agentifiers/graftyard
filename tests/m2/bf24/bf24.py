#!/usr/bin/env python3
"""BF-24 on the test install: head pushes to an open pull request, then `git ls-remote origin refs/pull/N/merge`.

bf24.py RUN TRIALS [--conflict-every K] [--transfer-at I,J,...]

Each trial makes a new head commit on the pull request's branch (every K-th one conflicting with the base), pushes it
with real git through the install's git edge, then polls `git ls-remote` for refs/pull/N/head and refs/pull/N/merge, and reads
the leader's `mergeable` (the value GraphQL renders, `graftyard_git::prsync::mergeable`), until both match the trial or
30 s pass. A clean head passes when refs/pull/N/merge is a merge commit whose parents are (base tip, head) and
`mergeable` is true; a conflicting one when the merge ref is gone and `mergeable` is false. The latency is from the
push's return to the first poll that shows the pass.

`--transfer-at` moves the group's leader to the next site (member `/transfer`) right after the push of each listed
trial, so the batch for that head is handled by a dispatcher that did not see it: the cursor has to carry it.
`--lose-at` does more for each listed trial: it pauses delivery on the leader's site queue before the push, purges the
queue 3 s after (the batch the leader sent is gone), resumes it, then transfers. Only a new leader that starts at the
committed cursor sends that head's batch again. `--drop-first-at` makes the leader drop the trial's first attempt
unrun (test-faults `/bench_seed {"prsyncDrop": 1}`), as if it was lost with its isolate: only the re-send through the
queue covers it. `--purge-resend-at` also pauses delivery on the leader's site queue before the push, purges it
38 s after (past the first re-send at `REDELIVER_AFTER_MS`), and resumes it, so only the second re-send covers the
batch. BF24_DEADLINE (default 30 s) is how long a trial polls. `--reset-at` resets the new leader's predecessor (member `/reset`)
after the transfer, so it restores from its storage, and the next transfer moves leadership back onto a restored
member.

Resumable: every trial is one JSON line in $BF24_RAW/RUN.jsonl; a restart picks up after the last line. The work tree
is $BF24_RAW/RUN.work.

The install comes from the environment, with the graft-m1 test install as the default (tests/m2/results/install-m2b.md
names graft-m2b's values): GRAFT_EDGE (the git edge), GRAFT_TOOLS_EDGE (the raw /member test edge), BF20_HOME
(tokens.txt and repos.json), BF24_OWNER (the owner's login in the repository's path), BF24_REPO (its name in
repos.json), BF24_PREFIX (the install's script prefix, for the site queues), and BF24_RAW (where runs go).
"""
import base64, json, os, subprocess, sys, threading, time, urllib.request

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "../../.."))
RAW = os.environ.get("BF24_RAW", f"{ROOT}/tests/m2/results/raw/wp5")
EDGE = os.environ.get("GRAFT_EDGE", "https://graft-m1-edge.<workers-subdomain>.workers.dev")
HOME = os.path.expanduser(os.environ.get("BF20_HOME", "~/.graft-integ"))
PREFIX = os.environ.get("BF24_PREFIX", "graft-m1")
REPO_ID = json.load(open(f"{HOME}/repos.json"))[os.environ.get("BF24_REPO", "bf24")]
WRITER = open(f"{HOME}/tokens.txt").read().split("\n")[1].split()[0]
URL = f"{EDGE}/{os.environ.get('BF24_OWNER', 'wp6')}/{os.environ.get('BF24_REPO', 'bf24')}.git/"
# The credential up front, so git skips the 401 round trip on every call.
AUTH = "Authorization: Basic " + base64.b64encode(f"x:{WRITER}".encode()).decode()
# In git's environment, never its command line, which shows in `ps` and in a `TimeoutExpired` traceback.
GIT_AUTH_ENV = {"GIT_CONFIG_COUNT": "1", "GIT_CONFIG_KEY_0": "http.extraHeader", "GIT_CONFIG_VALUE_0": AUTH}
M0 = open(f"{ROOT}/tests/m0/.m0-token").read().strip()
WP7 = os.environ.get("GRAFT_TOOLS_EDGE", "https://graft-m1-wp7-edge.<workers-subdomain>.workers.dev")
CHAIN = f"repo:{REPO_ID}"
N = 1
DEADLINE = float(os.environ.get("BF24_DEADLINE", "30"))
POLL_EVERY = 0.5

run = sys.argv[1]
trials = int(sys.argv[2])
args = sys.argv[3:]
conflict_every = int(args[args.index("--conflict-every") + 1]) if "--conflict-every" in args else 0
transfer_at = set(int(x) for x in args[args.index("--transfer-at") + 1].split(",")) if "--transfer-at" in args else set()
drop_at = set(int(x) for x in args[args.index("--drop-first-at") + 1].split(",")) if "--drop-first-at" in args else set()
reset_at = set(int(x) for x in args[args.index("--reset-at") + 1].split(",")) if "--reset-at" in args else set()
purge_resend_at = set(int(x) for x in args[args.index("--purge-resend-at") + 1].split(",")) if "--purge-resend-at" in args else set()
lose_at = set(int(x) for x in args[args.index("--lose-at") + 1].split(",")) if "--lose-at" in args else set()
out = f"{RAW}/{run}.jsonl"
work = f"{RAW}/{run}.work"


SECRETS = [WRITER, M0, base64.b64encode(f"x:{WRITER}".encode()).decode()]


def redact(m):
    """m with every token this driver holds replaced: every log line and uncaught traceback passes through here."""
    m = str(m)
    for t in SECRETS:
        m = m.replace(t, "[redacted]")
    return m


sys.excepthook = lambda t, v, tb: sys.stderr.write(redact("".join(__import__("traceback").format_exception(t, v, tb))))


def log(m):
    print(redact(f"{time.strftime('%H:%M:%S')} {m}"), flush=True)


def git(*a, cwd=None, check=True, timeout=120):
    env = dict(os.environ, GIT_AUTHOR_NAME="bf24", GIT_AUTHOR_EMAIL="bf24@graftyard.invalid",
               GIT_COMMITTER_NAME="bf24", GIT_COMMITTER_EMAIL="bf24@graftyard.invalid", **GIT_AUTH_ENV)
    r = subprocess.run(["git", "-c", "credential.helper=", "-c", "init.defaultBranch=main", *a], cwd=cwd or work,
                       env=env, capture_output=True, text=True, timeout=timeout)
    if check and r.returncode != 0:
        raise RuntimeError(f"git {a[0]}: {r.returncode} {r.stderr.strip()[:400]}")
    return r.stdout.strip()


def post(url, body, headers, timeout=40):
    # Cloudflare refuses urllib's default user agent (error 1010).
    headers = {"user-agent": "graftyard-bf24", **headers}
    req = urllib.request.Request(url, data=json.dumps(body).encode(), headers=headers, method="POST")
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read())


def member(site, path, body, gen=1):
    """A raw call to one member through the M1-WP7 test edge."""
    b = {"chain": CHAIN, "site": site, "path": path, "deadline": 20000, "gen": gen}
    if body is not None:
        b["body"] = body
    return post(f"{WP7}/member", b, {"authorization": f"Bearer {M0}"})


def leader_site():
    """The member that says it leads at the highest term."""
    for _ in range(10):
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
        time.sleep(1)
    return None


def leader_read(q):
    """A `fresh` read on whichever member leads."""
    for _ in range(6):
        s = leader_site()
        if s is None:
            continue
        try:
            v = member(s, "/read", {"q": q})
            if "value" in v.get("v", {}):
                return v["v"]["value"]
        except Exception as e:
            log(f"read: {e}")
        time.sleep(0.5)
    return None


def queue(op, site):
    """`wrangler queues pause-delivery|purge|resume-delivery` on one site's PrSync queue."""
    name = f"{PREFIX}-prsync-{'abc'[site]}"
    r = subprocess.run(["npx", "--prefix", f"{ROOT}/tests/m0", "wrangler", "queues", op, name, *(["--force"] if op == "purge" else [])],
                       capture_output=True, text=True, timeout=120, input="y\n")
    log(f"queue {op} {name}: {r.returncode} {(r.stdout + r.stderr).strip().splitlines()[-1:]}")
    return r.returncode == 0


def ls_remote(timeout=120):
    o = git("ls-remote", URL, f"refs/pull/{N}/*", "refs/heads/main", check=False, timeout=timeout)
    return {l.split("\t")[1]: l.split("\t")[0] for l in o.splitlines() if "\t" in l}


def setup():
    """The work tree, main, the pull request's branch, and the pull request, each only if missing."""
    if not os.path.isdir(work):
        os.makedirs(work)
        git("init", "-q", cwd=work)
        git("remote", "add", "origin", URL)
    remote = ls_remote()
    if "refs/heads/main" not in remote:
        open(f"{work}/base.txt", "w").write("base\n" * 3)
        open(f"{work}/shared.txt", "w").write("line 0\n")
        git("add", "-A")
        git("commit", "-qm", "base")
        git("push", "-q", "origin", "HEAD:refs/heads/main")
        git("push", "-q", "origin", "HEAD:refs/heads/pr")
    else:
        git("fetch", "-q", "origin", "refs/heads/main:refs/remotes/origin/main", "refs/heads/pr:refs/remotes/origin/pr")
        git("checkout", "-q", "-B", "pr", "origin/pr")
    view = leader_read({"prsync": [N]})
    if not view or not view["views"]:
        s = leader_site()
        r = member(s, "/bench_seed", {"openPr": {"n": N, "head": "refs/heads/pr", "base": "refs/heads/main"}})
        log(f"open #{N}: {r.get('v')}")


def done_trials():
    if not os.path.exists(out):
        return 0
    return sum(1 for _ in open(out))


def trial(i):
    conflict = conflict_every > 0 and i % conflict_every == conflict_every - 1
    base = git("rev-parse", "origin/main")
    git("checkout", "-q", "-B", "pr", "origin/pr")
    if conflict:
        # Change the line main changed in the conflict setup: main's shared.txt says "main N", this says "pr N".
        git("checkout", "-q", "origin/main", "--", "shared.txt")
        open(f"{work}/shared.txt", "w").write(f"pr {run} {i}\n")
    else:
        # A clean head: restore shared.txt to the base's, so only own.txt differs from main.
        git("checkout", "-q", "origin/main", "--", "shared.txt")
        open(f"{work}/own.txt", "w").write(f"{run} {i} {time.time()}\n")
    git("add", "-A")
    git("commit", "-qm", f"{run} {i}")
    head = git("rev-parse", "HEAD")
    prev_merge = ls_remote().get(f"refs/pull/{N}/merge")
    lost = None
    dropped = None
    if i in drop_at:
        s0 = leader_site()
        dropped = {"site": s0, "r": member(s0, "/bench_seed", {"prsyncDrop": 1}).get("v")}
    if i in lose_at or i in purge_resend_at:
        s0 = leader_site()
        lost = {"site": s0, "paused": queue("pause-delivery", s0)}
    t_push = time.time()
    git("push", "-q", "--force", "origin", "HEAD:refs/heads/pr")
    t0 = time.time()
    git("update-ref", "refs/remotes/origin/pr", head)
    transferred = None
    if lost and i in purge_resend_at:
        # The poll runs meanwhile in its own thread, so the latency still counts from the push.
        def purge_later(lost=lost, t0=t0):
            time.sleep(max(0.0, t0 + 38 - time.time()))
            lost["purged"] = queue("purge", lost["site"])
            lost["resumed"] = queue("resume-delivery", lost["site"])
            lost["purged_at_s"] = round(time.time() - t0, 3)
        purger = threading.Thread(target=purge_later)
        purger.start()
    elif lost:
        time.sleep(3)
        lost["purged"] = queue("purge", lost["site"])
        lost["resumed"] = queue("resume-delivery", lost["site"])
        lost["purged_at_s"] = round(time.time() - t0, 3)
    if i in transfer_at or (lost and i not in purge_resend_at):
        s = leader_site()
        to = (s + 1) % 3
        try:
            transferred = {"from": s, "to": to, "r": member(s, "/transfer", {"to": to}).get("v")}
        except Exception as e:
            transferred = {"from": s, "to": to, "error": str(e)}
        if i in reset_at:
            try:
                member(s, "/reset", {})
            except Exception as e:
                transferred["reset"] = str(e)[:80]
            transferred["reset_site"] = s
        log(f"transfer {transferred}")
    # Pollers start every POLL_EVERY s, each one `git ls-remote`; the trial's latency is the end of the earliest one
    # that shows refs/pull/N/head at the head and refs/pull/N/merge moved off the previous merge (clean) or gone
    # (conflict). Correctness is checked after: the merge commit's parents and the leader's `mergeable`.
    polls, lock, stop = [], threading.Lock(), threading.Event()

    def poll():
        a = time.time()
        try:
            refs = ls_remote(timeout=DEADLINE)
        except Exception:
            refs = {}
        b = time.time()
        with lock:
            polls.append((a - t0, b - t0, refs))

    threads = []
    while time.time() - t0 < DEADLINE and not stop.is_set():
        th = threading.Thread(target=poll)
        th.start()
        threads.append(th)
        time.sleep(POLL_EVERY)
        with lock:
            if any(shows(r, head, conflict, prev_merge) for _, _, r in polls):
                stop.set()
    for th in threads:
        th.join()
    if lost and i in purge_resend_at:
        purger.join()
    hits = sorted(e for _, e, r in polls if shows(r, head, conflict, prev_merge))
    seen = hits[0] if hits else None
    shown = next((r for _, e, r in sorted(polls, key=lambda p: p[1]) if shows(r, head, conflict, prev_merge)), {})
    merge = shown.get(f"refs/pull/{N}/merge")
    parents = merge_parents(merge) if merge else None
    if not conflict and parents != [base, head]:
        seen = None
    final = ls_remote()
    v = leader_read({"prsync": [N]})
    server = server_ms(head, conflict)
    m = v["mergeable"][0] if v and v["mergeable"] else None
    if m is not (not conflict):
        seen = None
    rec = {
        "i": i, "conflict": conflict, "base": base, "head": head, "push_s": round(t0 - t_push, 3),
        "latency_s": None if seen is None else round(seen, 3), "polls": len(polls), "transfer": transferred, "lost": lost, "dropped": dropped,
        "poll_s": round(sum(e - a for a, e, _ in polls) / max(1, len(polls)), 3), "parents": parents,
        "merge_ref": final.get(f"refs/pull/{N}/merge"), "head_ref": final.get(f"refs/pull/{N}/head"),
        "mergeable": v["mergeable"][0] if v and v["mergeable"] else None,
        "cursor": v["cursor"] if v else None, "at": time.time(), "server": server,
    }
    return rec


def shows(refs, head, conflict, prev_merge):
    merge = refs.get(f"refs/pull/{N}/merge")
    if refs.get(f"refs/pull/{N}/head") != head:
        return False
    return merge is None if conflict else (merge is not None and merge != prev_merge)


def server_ms(head, conflict):
    """On the leader's clock: the entry that moved refs/heads/pr to `head`, the `TestMerged` of `head`, and the entry
    that moved refs/pull/N/head to it, as (index, at) each, and the push-to-`TestMerged` time in ms."""
    s = leader_site()
    if s is None:
        return None
    try:
        st = member(s, "/status", None).get("v") or {}
        last = st.get("last") or st.get("commit") or 0
        log_ = member(s, f"/log?from={max(1, int(last) - 40)}&n=60", None).get("v") or []
    except Exception as e:
        return {"error": str(e)}
    push = merged = pull = None
    for e in log_:
        for name, new in e.get("refs") or []:
            if name == "refs/heads/pr" and new == head and push is None:
                push = (e["index"], e["at"])
            if name == f"refs/pull/{N}/head" and new == head and pull is None:
                pull = (e["index"], e["at"])
        for t in e.get("testMerged") or []:
            if t["head"] == head and merged is None:
                merged = (e["index"], e["at"], t["clean"])
    ms = merged[1] - push[1] if push and merged else None
    return {"push": push, "testMerged": merged, "pullHead": pull, "ms": ms, "leader": s}


def merge_parents(oid):
    """The merge commit's parents, fetched from the remote: refs/pull/N/merge must serve as a real commit."""
    r = subprocess.run(["git", "-c", "credential.helper=", "fetch", "-q", URL, f"refs/pull/{N}/merge"], cwd=work,
                       env=dict(os.environ, **GIT_AUTH_ENV), capture_output=True, text=True, timeout=60)
    if r.returncode != 0:
        return None
    p = subprocess.run(["git", "show", "-s", "--format=%P", oid], cwd=work, capture_output=True, text=True)
    return p.stdout.split() if p.returncode == 0 else None


def main():
    setup()
    # The conflicting line: main holds "main <run>" in shared.txt, so a head that writes it conflicts and one that
    # keeps the base's does not.
    if conflict_every:
        git("fetch", "-q", "origin", "refs/heads/main:refs/remotes/origin/main")
        git("checkout", "-q", "-B", "m", "origin/main")
        if open(f"{work}/shared.txt").read() != f"main {run}\n":
            open(f"{work}/shared.txt", "w").write(f"main {run}\n")
            git("commit", "-qam", f"main {run}")
            git("push", "-q", "origin", "HEAD:refs/heads/main")
            git("update-ref", "refs/remotes/origin/main", git("rev-parse", "HEAD"))
    git("fetch", "-q", "origin", "refs/heads/pr:refs/remotes/origin/pr")
    i = done_trials()
    while i < trials:
        try:
            rec = trial(i)
        except Exception as e:
            log(f"trial {i}: {e}; again")
            time.sleep(5)
            git("fetch", "-q", "-f", "origin", "refs/heads/main:refs/remotes/origin/main",
                "refs/heads/pr:refs/remotes/origin/pr", check=False)
            continue
        with open(out, "a") as f:
            f.write(redact(json.dumps(rec)) + "\n")
        log(f"{i} {'conflict' if rec['conflict'] else 'clean'} {rec['latency_s']} s mergeable={rec['mergeable']}")
        i += 1


main()
