#!/usr/bin/env python3
"""L0-11's hosted byte diff on graft-m1: the same read-only REST and GraphQL requests an unmodified client sends (gh's
own GraphQL documents from crates/api/gh, its REST calls, and the GHES routes the edge serves), with no agent header
and gh's default Accept, against graft-m1-edge. Run once before a deploy and once after; `diff` compares two runs.

bytediff.py run OUT [ROUNDS]     ROUNDS passes (default 10) over the request list into OUT/<n>.json, each reply as
                                 {status, headers (sorted, minus volatile), body bytes as text}
bytediff.py diff A B             every request whose status, kept headers, or body differs from A's reply with the
                                 same status, after masking x-github-request-id, date, the server-timing headers and
                                 the etag's value; exit 1 on any difference

The tokens come from ~/.graft-integ/tokens.txt (the wp6 owner and writer) and never reach argv or a log line.

Another install sets BYTEDIFF_OWNER, BYTEDIFF_REPOS (space separated) and BYTEDIFF_TOKENS, a file of `name token` lines
read by BYTEDIFF_TOKEN_NAMES (default `admin member`, the owner first, then the plain user every request is sent as).
graft-m2b's, the B0-33 row's (`tests/fanout/guardrow.sh`, BYTEDIFF_OUT), with `tests/m3/rows/env.sh` sourced:
  BYTEDIFF_OWNER=m3org BYTEDIFF_REPOS="j4 b034-mono" BYTEDIFF_TOKENS=$M3_STATE/m3-tokens.txt bytediff.py run OUT 3
"""
import base64, glob, json, os, re, sys, time, urllib.error, urllib.request

EDGE = os.environ.get("GRAFT_EDGE", f"https://graft-m1-edge.{os.environ['CF_WORKERS_SUBDOMAIN']}.workers.dev")
HOME = os.path.expanduser("~/.graft-integ")


def tokens():
    if "BYTEDIFF_TOKENS" not in os.environ:
        return [l.split()[0] for l in open(f"{HOME}/tokens.txt").read().split("\n") if l.strip()]
    named = dict(l.split()[:2] for l in open(os.environ["BYTEDIFF_TOKENS"]) if len(l.split()) >= 2)
    return [named[n] for n in os.environ.get("BYTEDIFF_TOKEN_NAMES", "admin member").split()]


TOKENS = tokens() if sys.argv[1:2] == ["run"] else []
ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "../../.."))
OWNER = os.environ.get("BYTEDIFF_OWNER", "wp6")
REPOS = os.environ.get("BYTEDIFF_REPOS", "wp6j wp3m main").split()
VOLATILE = {"x-github-request-id", "date", "cf-ray", "server-timing", "report-to", "nel", "server", "alt-svc",
            "cf-cache-status", "content-length", "x-graft-timing"}


def redact(s):
    for t in TOKENS:
        s = s.replace(t, "[redacted]")
    # The edge's host names the account's workers.dev subdomain, which a committed baseline must not hold: written as
    # EDGE, the same in every run, so a diff still compares every URL the edge answers.
    return s.replace(EDGE.split("//", 1)[-1], "EDGE")


def call(method, path, token, body=None):
    h = {"user-agent": "gh/2.102.1", "accept": "application/vnd.github.merge-info-preview+json, application/vnd.github.nebula-preview"}
    if token:
        h["authorization"] = f"token {token}"
    data = None
    if body is not None:
        data = json.dumps(body).encode()
        h["content-type"] = "application/json; charset=utf-8"
    for attempt in range(5):
        req = urllib.request.Request(EDGE + path, data=data, headers=h, method=method)
        try:
            r = urllib.request.urlopen(req, timeout=60)
            status, hdrs, b = r.status, r.headers, r.read()
            break
        except urllib.error.HTTPError as e:
            status, hdrs, b = e.code, e.headers, e.read()
            break
        except (urllib.error.URLError, TimeoutError, ConnectionError) as e:
            # A transport failure is no reply: retried, and never compared.
            if attempt == 4:
                raise
            time.sleep(2 * (attempt + 1))
    kept = sorted((k.lower(), v) for k, v in hdrs.items() if k.lower() not in VOLATILE)
    return {"status": status, "headers": kept, "body": b.decode("utf-8", "replace")}


def requests():
    """(name, method, path, token index or None, body)."""
    out = []
    for repo in REPOS:
        p = f"/api/v3/repos/{OWNER}/{repo}"
        for name, path in [("repo", p), ("issues", f"{p}/issues?state=all&per_page=30"), ("pulls", f"{p}/pulls?state=all"),
                           ("labels", f"{p}/labels"), ("releases", f"{p}/releases"), ("collab", f"{p}/collaborators"),
                           ("statuses", f"{p}/commits/main/statuses"), ("issue1", f"{p}/issues/1"),
                           ("missing", f"{p}/issues/99999"), ("tasks-anon", f"/api/graftyard/v1/repos/{OWNER}/{repo}/tasks")]:
            out.append((f"{repo}-{name}", "GET", path, 1, None))
            if name in ("repo", "issues", "issue1"):
                out.append((f"{repo}-{name}-anon", "GET", path, None, None))
        for f in sorted(glob.glob(f"{ROOT}/crates/api/gh/*.json")):
            d = json.load(open(f))
            q = d["query"]
            if q.lstrip().startswith("mutation"):
                continue
            v = json.loads(json.dumps(d.get("variables") or {}).replace('"o"', f'"{OWNER}"').replace('"r"', f'"{repo}"'))
            for k in ("pr_number", "number"):
                if k in v:
                    v[k] = 1
            out.append((f"{repo}-gql-{os.path.basename(f)[:-5]}", "POST", "/api/graphql", 1,
                        {"query": q, "variables": v, "operationName": d.get("operation")}))
    out += [("meta", "GET", "/api/v3/meta", None, None), ("user", "GET", "/api/v3/user", 1, None),
            ("rate", "GET", "/api/v3/rate_limit", 1, None), ("notfound", "GET", "/api/v3/nope", None, None),
            ("agents-v3", "GET", f"/api/v3/agents/repos/{OWNER}/{REPOS[0]}/tasks", 1, None)]
    return out


def mask(s):
    # Times and ids that differ per request, not per release.
    s = re.sub(r'"(x-github-request-id|requestId)":\s*"[^"]*"', r'"\1":"*"', s)
    s = re.sub(r"\b[0-9A-F]{4,8}:[0-9A-F]{4,8}:[0-9A-F]{4,10}:[0-9A-F]{4,10}:[0-9A-F]{6,10}\b", "RID", s)
    s = re.sub(r'"reset":\s*\d+', '"reset":0', s)
    s = re.sub(r'"(used|remaining)":\s*\d+', r'"\1":0', s)
    return s


def run(out, rounds):
    os.makedirs(out, exist_ok=True)
    reqs = requests()
    n = 0
    for r in range(rounds):
        res = {}
        for name, method, path, tok, body in reqs:
            res[name] = call(method, path, TOKENS[tok] if tok is not None else None, body)
            n += 1
        with open(f"{out}/{r}.json", "w") as f:
            f.write(redact(json.dumps(res, indent=1, sort_keys=True)))
        print(f"{time.strftime('%H:%M:%S')} round {r}: {len(reqs)} requests", flush=True)
    print(f"total {n} requests, {len(reqs)} distinct", flush=True)


def norm(c):
    # An `etag` is kept but not its value, which follows the repository's state, not the release: graft-m2b's m3org/j4
    # answered byte-identical bodies under three etags in three rounds (2026-11-23).
    h = [(k, "*" if k == "etag" else v) for k, v in c["headers"] if not k.startswith("x-ratelimit")]
    return c["status"], h, mask(c["body"])


def excusable(method, path, body):
    """Whether a 5xx on this request may be load: a REST or GraphQL read only. A write, a GraphQL mutation, and git's
    protocol (info/refs, upload-pack, receive-pack) never are (the lead's ruling of 2026-11-23)."""
    p = path.split("?")[0]
    if not p.startswith("/api/") or p.endswith(("/info/refs", "/git-upload-pack", "/git-receive-pack")):
        return False
    if p.endswith("/graphql"):
        return method == "POST" and not str((body or {}).get("query", "")).lstrip().startswith("mutation")
    return method in ("GET", "HEAD")


def diff(a, b):
    """Each reply against A's first reply to the same request with the same status. A 5xx whose status A never
    answered that request is load, not a release, when the request is a REST or GraphQL read (`excusable`): counted
    as unavailable, printed, and not compared (a tierOff shed on graft-m2b's idle m3org/j4 answered one round of
    three, 2026-11-23), unless more than a tenth of B's replies are, which is a release answering 5xx, not load. On a
    write, a mutation, git's protocol, or a request this install's list does not name, it differs. Any other status A
    never answered differs."""
    reqs = {name: (method, path, body) for name, method, path, _, body in requests()}
    bad = total = unavailable = 0
    fa, fb = sorted(glob.glob(f"{a}/*.json")), sorted(glob.glob(f"{b}/*.json"))
    base = {}
    for f in fa:
        for k, v in json.load(open(f)).items():
            base.setdefault(k, {}).setdefault(v["status"], v)
    for f in fa + fb:
        cur = json.load(open(f))
        for k, v in cur.items():
            total += 1
            was = base.get(k, {}).get(v["status"])
            if was is None and v["status"] >= 500:
                method, path, body = reqs.get(k, ("?", "?", None))
                where = f"{os.path.basename(os.path.dirname(f))}/{os.path.basename(f)} {k}: {method} {path} {v['status']}"
                if excusable(method, path, body):
                    unavailable += 1
                    print(f"UNAVAILABLE {where} (a read, excused as load)")
                    continue
                print(f"UNAVAILABLE {where} (not a REST or GraphQL read: counted as a difference)")
            if was is None or norm(v) != norm(was):
                bad += 1
                old = was or next(iter(base.get(k, {}).values()), {"status": None, "headers": [], "body": ""})
                if bad <= 20:
                    print(f"DIFF {os.path.basename(os.path.dirname(f))}/{os.path.basename(f)} {k}: "
                          f"{norm(old)[0]} -> {norm(v)[0]}\n  before {norm(old)[1]} {norm(old)[2][:300]}"
                          f"\n  after  {norm(v)[1]} {norm(v)[2][:300]}")
    print(f"{total} replies compared against {a}, {bad} differ, {unavailable} unavailable (a 5xx {a} never answered)")
    return bad + (unavailable if unavailable * 10 > total else 0)


if __name__ == "__main__":
    if sys.argv[1] == "run":
        run(sys.argv[2], int(sys.argv[3]) if len(sys.argv) > 3 else 10)
    else:
        sys.exit(1 if diff(sys.argv[2], sys.argv[3]) else 0)
