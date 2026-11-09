#!/usr/bin/env python3
"""L0-11's hosted byte diff on graft-m1: the same read-only REST and GraphQL requests an unmodified client sends (gh's
own GraphQL documents from crates/api/gh, its REST calls, and the GHES routes the edge serves), with no agent header
and gh's default Accept, against graft-m1-edge. Run once before a deploy and once after; `diff` compares two runs.

bytediff.py run OUT [ROUNDS]     ROUNDS passes (default 10) over the request list into OUT/<n>.json, each reply as
                                 {status, headers (sorted, minus volatile), body bytes as text}
bytediff.py diff A B             every request whose status, kept headers, or body differs, after masking
                                 x-github-request-id, date, and the server-timing headers; exit 1 on any difference

The tokens come from ~/.graft-integ/tokens.txt (the wp6 owner and writer) and never reach argv or a log line.
"""
import base64, glob, json, os, re, sys, time, urllib.error, urllib.request

EDGE = os.environ.get("GRAFT_EDGE", "https://graft-m1-edge.<workers-subdomain>.workers.dev")
HOME = os.path.expanduser("~/.graft-integ")
TOKENS = [l.split()[0] for l in open(f"{HOME}/tokens.txt").read().split("\n") if l.strip()]
ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "../../.."))
OWNER, REPOS = "wp6", ["wp6j", "wp3m", "main"]
VOLATILE = {"x-github-request-id", "date", "cf-ray", "server-timing", "report-to", "nel", "server", "alt-svc",
            "cf-cache-status", "content-length", "x-graft-timing"}


def redact(s):
    for t in TOKENS:
        s = s.replace(t, "[redacted]")
    return s


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
            ("agents-v3", "GET", "/api/v3/agents/repos/wp6/wp6j/tasks", 1, None)]
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
    h = [(k, v) for k, v in c["headers"] if not k.startswith("x-ratelimit")]
    return c["status"], h, mask(c["body"])


def diff(a, b):
    bad = 0
    total = 0
    fa, fb = sorted(glob.glob(f"{a}/*.json")), sorted(glob.glob(f"{b}/*.json"))
    base = json.load(open(fa[0]))
    for f in fa + fb:
        cur = json.load(open(f))
        for k, v in cur.items():
            total += 1
            if norm(v) != norm(base[k]):
                bad += 1
                if bad <= 20:
                    print(f"DIFF {os.path.basename(os.path.dirname(f))}/{os.path.basename(f)} {k}: "
                          f"{norm(base[k])[0]} -> {norm(v)[0]}\n  before {norm(base[k])[2][:300]}\n  after  {norm(v)[2][:300]}")
    print(f"{total} replies compared against {os.path.basename(fa[0])} of {a}, {bad} differ")
    return bad


if __name__ == "__main__":
    if sys.argv[1] == "run":
        run(sys.argv[2], int(sys.argv[3]) if len(sys.argv) > 3 else 10)
    else:
        sys.exit(1 if diff(sys.argv[2], sys.argv[3]) else 0)
