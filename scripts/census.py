#!/usr/bin/env python3
"""Count GitHub wire surfaces for the Graftyard design (stdlib only).

Sources:
  REST + webhooks: github/rest-api-description, pinned to one commit.
  GraphQL:         docs.github.com/public/<plan>/schema.docs*.graphql.
  Git protocol v2: GIT_TRACE_PACKET=1 git ls-remote https://github.com/git/git.

Downloads go to /tmp/graftyard-research/census/. The first run records the
resolved commit and file hashes in pins.json there; later runs reuse the pins
and cached files, so reruns are deterministic and offline. Pass --refresh to
re-resolve the newest commit, re-download everything, and re-capture git.

Writes research/surfaces.json next to this script's parent directory.
Set GITHUB_TOKEN or GH_TOKEN to avoid the 60/hour unauthenticated API limit.
"""

import collections
import hashlib
import json
import os
import re
import ssl
import subprocess
import sys
import urllib.request

CACHE = "/tmp/graftyard-research/census"
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, "research", "surfaces.json")
PINS = os.path.join(CACHE, "pins.json")
REPO = "github/rest-api-description"
HTTP_METHODS = ("get", "put", "post", "delete", "patch", "head", "options", "trace")
UA = "graftyard-census/1"


# ---------------------------------------------------------------- fetching

def _ssl_context():
    # python.org builds on macOS ship without a CA bundle; add the system one.
    ctx = ssl.create_default_context()
    for path in (os.environ.get("SSL_CERT_FILE"), "/etc/ssl/cert.pem",
                 "/etc/ssl/certs/ca-certificates.crt"):
        if path and os.path.exists(path):
            ctx.load_verify_locations(cafile=path)
    return ctx


SSL_CTX = _ssl_context()


def _token():
    for k in ("GITHUB_TOKEN", "GH_TOKEN"):
        if os.environ.get(k):
            return os.environ[k]
    return None


def http_get(url, api=False):
    headers = {"User-Agent": UA}
    if api:
        headers["Accept"] = "application/vnd.github+json"
        headers["X-GitHub-Api-Version"] = "2022-11-28"
        tok = _token()
        if tok:
            headers["Authorization"] = "Bearer " + tok
    req = urllib.request.Request(url, headers=headers)
    with urllib.request.urlopen(req, timeout=120, context=SSL_CTX) as resp:
        return resp.read()


def cached(name, url, refresh):
    path = os.path.join(CACHE, name)
    if refresh or not os.path.exists(path):
        data = http_get(url)
        tmp = path + ".part"
        with open(tmp, "wb") as f:
            f.write(data)
        os.replace(tmp, path)
    with open(path, "rb") as f:
        return f.read()


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def ghes_key(name):
    m = re.fullmatch(r"ghes-(\d+)\.(\d+)", name)
    return (int(m.group(1)), int(m.group(2))) if m else None


def resolve_pins():
    commit = json.loads(http_get(f"https://api.github.com/repos/{REPO}/commits/main", api=True))
    sha = commit["sha"]
    listing = json.loads(http_get(
        f"https://api.github.com/repos/{REPO}/contents/descriptions?ref={sha}", api=True))
    ghes = sorted((ghes_key(e["name"]), e["name"]) for e in listing
                  if e["type"] == "dir" and ghes_key(e["name"]) and e["name"].startswith("ghes-3."))
    newest = ghes[-1][1]
    versions = {}
    for plan in ("api.github.com", newest):
        files = json.loads(http_get(
            f"https://api.github.com/repos/{REPO}/contents/descriptions/{plan}?ref={sha}", api=True))
        versions[plan] = sorted(
            m.group(1) for e in files
            for m in [re.fullmatch(re.escape(plan) + r"\.(\d{4}-\d{2}-\d{2})\.json", e["name"])] if m)
    return {
        "rest_commit": sha,
        "rest_commit_date": commit["commit"]["committer"]["date"],
        "ghes": newest,
        "api_versions": versions,
        "files": {},
    }


# ---------------------------------------------------------------- REST + webhooks

def rest_ops(doc):
    for path, item in doc["paths"].items():
        for method, op in item.items():
            if method in HTTP_METHODS:
                yield path, method, op


def rest_stats(doc):
    cats = collections.Counter()
    subs = collections.Counter()
    cloud_only = deprecated = 0
    for _, _, op in rest_ops(doc):
        xg = op.get("x-github", {})
        cat = xg.get("category", "(none)")
        cats[cat] += 1
        subs[(cat, xg.get("subcategory", "(none)"))] += 1
        cloud_only += bool(xg.get("githubCloudOnly"))
        deprecated += bool(xg.get("deprecationDate") or op.get("deprecated"))
    return {
        "paths": len(doc["paths"]),
        "operations": sum(cats.values()),
        "github_cloud_only_flagged": cloud_only,
        "deprecated": deprecated,
        "categories": dict(sorted(cats.items())),
        "subcategories": {f"{c}/{s}": n for (c, s), n in sorted(subs.items())},
    }


def webhook_stats(doc):
    # Event name = operationId prefix with "-" -> "_" (e.g. projects-v2-item/edited
    # -> projects_v2_item). The X-Github-Event examples in the spec are not reliable
    # (they say "project-v2" for projects_v2).
    hooks = doc.get("webhooks") or doc.get("x-webhooks") or {}
    events = collections.defaultdict(set)
    headers = set()
    for key, item in hooks.items():
        for op in item.values():
            headers.update(p["name"] for p in op.get("parameters", []) if p.get("in") == "header")
            op_id = op.get("operationId", key)
            event = op_id.split("/")[0].replace("-", "_")
            action = op_id.split("/", 1)[1] if "/" in op_id else None
            events[event].add(action)
    pairs = sum(len(a) for a in events.values())
    return {
        "events": len(events),
        "event_action_pairs": pairs,
        "events_without_actions": sorted(e for e, a in events.items() if a == {None}),
        "by_event": {e: sorted(a for a in acts if a) for e, acts in sorted(events.items())},
        "delivery_headers": sorted(headers),
    }


# ---------------------------------------------------------------- GraphQL

def strip_sdl(text):
    text = re.sub(r'"""(?:.|\n)*?"""', " ", text)
    text = re.sub(r'"(?:\\.|[^"\\\n])*"', " ", text)
    text = re.sub(r"#[^\n]*", " ", text)
    return text


def graphql_stats(text):
    src = strip_sdl(text)
    kinds = collections.Counter()
    root_fields = {}
    pos = 0
    head = re.compile(r"\b(extend\s+)?(type|interface|union|enum|input|scalar|directive|schema)\b\s*@?(\w*)")
    while True:
        m = head.search(src, pos)
        if not m:
            break
        kind, name = m.group(2), m.group(3)
        if not m.group(1):
            kinds[kind] += 1
        # find body end: either a brace block or end of line for union/scalar/directive
        i = m.end()
        brace = src.find("{", i)
        nxt = head.search(src, i)
        if kind in ("type", "interface", "enum", "input", "schema") and brace != -1 and (not nxt or brace < nxt.start()):
            depth, j = 0, brace
            fields, paren, tok_start = [], 0, None
            while j < len(src):
                ch = src[j]
                if ch == "{":
                    depth += 1
                elif ch == "}":
                    depth -= 1
                    if depth == 0:
                        break
                elif ch == "(":
                    paren += 1
                elif ch == ")":
                    paren -= 1
                j += 1
            body = src[brace + 1:j]
            if kind == "type" and name in ("Query", "Mutation"):
                flat = re.sub(r"\((?:[^()]|\([^()]*\))*\)", " ", body)
                flat = re.sub(r"@\w+(\s*\([^)]*\))?", " ", flat)
                fields = re.findall(r"(?m)^\s*(\w+)\s*:", flat)
                root_fields[name] = len(fields)
            pos = j + 1
        else:
            pos = i
    return {
        "object_types": kinds["type"],
        "interfaces": kinds["interface"],
        "unions": kinds["union"],
        "enums": kinds["enum"],
        "input_types": kinds["input"],
        "scalars": kinds["scalar"],
        "directives": kinds["directive"],
        "query_fields": root_fields.get("Query"),
        "mutation_fields": root_fields.get("Mutation"),
    }


# ---------------------------------------------------------------- git v2 capture

def capture_v2(refresh):
    path = os.path.join(CACHE, "git-v2-advert.txt")
    if refresh or not os.path.exists(path):
        env = dict(os.environ, GIT_TRACE_PACKET="1", GIT_TERMINAL_PROMPT="0")
        try:
            proc = subprocess.run(["git", "ls-remote", "https://github.com/git/git", "HEAD"],
                                  env=env, capture_output=True, text=True, timeout=120)
        except (OSError, subprocess.TimeoutExpired):
            return None
        lines, seen_v2 = [], False
        for line in proc.stderr.splitlines():
            m = re.search(r"packet:\s+git< (.*)$", line)
            if not m:
                continue
            pkt = m.group(1)
            if pkt == "version 2":
                seen_v2 = True
            if seen_v2:
                if pkt == "0000":
                    break
                lines.append(pkt)
        if not lines:
            return None
        with open(path, "w") as f:
            f.write("\n".join(lines) + "\n")
    with open(path) as f:
        lines = [l for l in f.read().splitlines() if l]
    caps = [l for l in lines if l != "version 2"]
    commands = [c.split("=")[0] for c in caps
                if c.split("=")[0] in ("ls-refs", "fetch", "object-info", "bundle-uri")]
    return {"advertisement": lines, "commands": commands}


# ---------------------------------------------------------------- surfaces

STATIC = [
    # id, kind, count, count_basis, source
    ("git-http", "git", 3,
     "smart HTTP endpoints: GET info/refs?service=, POST git-upload-pack, POST git-receive-pack",
     "https://git-scm.com/docs/gitprotocol-http"),
    ("git-ssh", "git", 3,
     "remote commands run as user git: git-upload-pack, git-receive-pack, git-upload-archive",
     "https://git-scm.com/docs/gitprotocol-pack"),
    ("git-protocol-v2", "git", None, "commands advertised by github.com (captured)",
     "https://git-scm.com/docs/gitprotocol-v2"),
    ("lfs", "git", 5,
     "Batch API (POST objects/batch) plus locking API (POST locks, GET locks, POST locks/verify, POST locks/:id/unlock)",
     "https://github.com/git-lfs/git-lfs/tree/main/docs/api"),
    ("graphql", "graphql", None, "Query + Mutation root fields in newest GHES schema", None),
    ("webhooks", "webhooks", None, "event-action pairs in newest GHES description", None),
    ("oauth-web-flow", "auth", 3,
     "GET /login/oauth/authorize, POST /login/oauth/access_token, GET user API with token",
     "https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/authorizing-oauth-apps"),
    ("oauth-device-flow", "auth", 2,
     "POST /login/device/code, POST /login/oauth/access_token (device_code grant); user visits /login/device",
     "https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/authorizing-oauth-apps#device-flow"),
    ("github-apps", "auth", None,
     "JWT (RS256, iss=client or app id) -> POST /app/installations/{id}/access_tokens; REST ops counted under rest:apps",
     "https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/generating-a-json-web-token-jwt-for-a-github-app"),
    ("tokens", "auth", 6,
     "token prefixes: ghp_, github_pat_, gho_, ghu_, ghs_, ghr_",
     "https://github.blog/engineering/platform-security/behind-githubs-new-authentication-token-formats/"),
    ("actions-runner-protocol", "actions", None, "see research/actions-protocol.md",
     "https://github.com/actions/runner"),
    ("actions-workflow-syntax", "actions", None, "see research/actions-protocol.md",
     "https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax"),
    ("actions-cache", "actions", None, "see research/actions-protocol.md",
     "https://github.com/actions/toolkit/tree/main/packages/cache"),
    ("actions-artifacts", "actions", None, "see research/actions-protocol.md",
     "https://github.com/actions/toolkit/tree/main/packages/artifact"),
    ("actions-oidc", "actions", None, "see research/actions-protocol.md",
     "https://docs.github.com/en/actions/concepts/security/openid-connect"),
    ("actions-logs-results", "actions", None, "see research/actions-protocol.md",
     "https://github.com/actions/runner"),
    ("packages-container", "packages", None, "OCI Distribution Spec endpoints on ghcr.io / containers.HOSTNAME",
     "https://docs.github.com/en/packages/working-with-a-github-packages-registry/working-with-the-container-registry"),
    ("packages-npm", "packages", None, "npm registry protocol on npm.pkg.github.com / npm.HOSTNAME",
     "https://docs.github.com/en/packages/working-with-a-github-packages-registry/working-with-the-npm-registry"),
    ("packages-maven", "packages", None, "Maven repository layout on maven.pkg.github.com / maven.HOSTNAME",
     "https://docs.github.com/en/packages/working-with-a-github-packages-registry/working-with-the-apache-maven-registry"),
    ("packages-nuget", "packages", None, "NuGet v3 service index on nuget.pkg.github.com / nuget.HOSTNAME",
     "https://docs.github.com/en/packages/working-with-a-github-packages-registry/working-with-the-nuget-registry"),
    ("packages-rubygems", "packages", None, "RubyGems protocol on rubygems.pkg.github.com / rubygems.HOSTNAME",
     "https://docs.github.com/en/packages/working-with-a-github-packages-registry/working-with-the-rubygems-registry"),
    ("pages", "content", None, "static site serving at OWNER.github.io / pages.HOSTNAME; REST ops counted under rest:pages",
     "https://docs.github.com/en/pages/getting-started-with-github-pages/about-github-pages"),
    ("codeload-archives", "content", 2,
     "tarball and zipball archives (codeload.github.com / HOSTNAME/_codeload or codeload.HOSTNAME); REST redirects from /repos/{o}/{r}/tarball|zipball/{ref}",
     "https://docs.github.com/en/rest/repos/contents#download-a-repository-archive-tar"),
    ("raw-content", "content", 1,
     "raw file bytes (raw.githubusercontent.com / HOSTNAME/raw or raw.HOSTNAME)",
     "https://docs.github.com/en/enterprise-server@latest/admin/configuring-settings/hardening-security-for-your-enterprise/enabling-subdomain-isolation"),
    ("uploads-release-assets", "content", 1,
     "POST upload_url (uploads.github.com / HOSTNAME/api/uploads)",
     "https://docs.github.com/en/rest/releases/assets#upload-a-release-asset"),
    ("meta-endpoint", "meta", None, "GET /meta fields; REST op counted under rest:meta",
     "https://docs.github.com/en/rest/meta/meta#get-github-meta-information"),
]


def main():
    refresh = "--refresh" in sys.argv[1:]
    os.makedirs(CACHE, exist_ok=True)
    os.makedirs(os.path.dirname(OUT), exist_ok=True)

    pins = None
    if not refresh and os.path.exists(PINS):
        with open(PINS) as f:
            pins = json.load(f)
    if pins is None:
        pins = resolve_pins()
    sha, ghes = pins["rest_commit"], pins["ghes"]
    raw = f"https://raw.githubusercontent.com/{REPO}/{sha}/descriptions"

    def load(plan, version=None):
        fname = f"{plan}.{version}.json" if version else f"{plan}.json"
        data = cached(f"{sha[:12]}-{fname}", f"{raw}/{plan}/{fname}", refresh)
        pins["files"][f"{plan}/{fname}"] = sha256(data)
        return json.loads(data)

    dot_doc, ghes_doc = load("api.github.com"), load(ghes)
    dot, gh = rest_stats(dot_doc), rest_stats(ghes_doc)
    by_version = {plan: {v: rest_stats(load(plan, v))["operations"] for v in vs}
                  for plan, vs in pins["api_versions"].items()}
    wh_dot, wh_ghes = webhook_stats(dot_doc), webhook_stats(ghes_doc)
    del dot_doc, ghes_doc

    gql = {}
    gql_src = {
        "dotcom": "https://docs.github.com/public/fpt/schema.docs.graphql",
        ghes: f"https://docs.github.com/public/{ghes}/schema.docs-enterprise.graphql",
    }
    for plan, url in gql_src.items():
        data = cached(f"graphql-{plan}.graphql", url, refresh)
        pins["files"][url] = sha256(data)
        gql[plan] = graphql_stats(data.decode("utf-8"))

    v2 = capture_v2(refresh)

    with open(PINS, "w") as f:
        json.dump(pins, f, indent=2, sort_keys=True)
        f.write("\n")

    rest_src = f"https://github.com/{REPO}/tree/{sha}/descriptions"
    surfaces = []
    for sid, kind, count, basis, src in STATIC:
        entry = {"id": sid, "kind": kind, "count": count, "source": src, "count_basis": basis}
        if sid == "git-protocol-v2" and v2:
            entry["count"] = len(v2["commands"])
            entry["advertisement"] = v2["advertisement"]
            entry["source"] = "GIT_TRACE_PACKET=1 git ls-remote https://github.com/git/git; https://git-scm.com/docs/gitprotocol-v2"
        if sid == "graphql":
            g = gql[ghes]
            entry["count"] = g["query_fields"] + g["mutation_fields"]
            entry["source"] = gql_src[ghes]
            entry["detail"] = gql
        if sid == "webhooks":
            entry["count"] = wh_ghes["event_action_pairs"]
            entry["source"] = f"{rest_src}/{ghes}/{ghes}.json#x-webhooks"
            entry["detail"] = {
                "dotcom": {k: wh_dot[k] for k in ("events", "event_action_pairs")},
                ghes: {k: wh_ghes[k] for k in ("events", "event_action_pairs")},
                "dotcom_only_events": sorted(set(wh_dot["by_event"]) - set(wh_ghes["by_event"])),
                "ghes_only_events": sorted(set(wh_ghes["by_event"]) - set(wh_dot["by_event"])),
                "delivery_headers": wh_ghes["delivery_headers"],
            }
        surfaces.append(entry)
        if sid == "lfs":
            # REST categories follow the git surfaces
            for cat in sorted(set(gh["categories"]) | set(dot["categories"])):
                e = {"id": f"rest:{cat}", "kind": "rest",
                     "count": gh["categories"].get(cat, dot["categories"].get(cat)),
                     "source": f"{rest_src}/{ghes}/{ghes}.json"}
                e["count_dotcom"] = dot["categories"].get(cat, 0)
                base = gh if cat in gh["categories"] else dot
                e["subcategories"] = {k.split("/", 1)[1]: n for k, n in base["subcategories"].items()
                                      if k.split("/", 1)[0] == cat}
                if cat not in gh["categories"]:
                    e["dotcom_only"] = True
                    e["source"] = f"{rest_src}/api.github.com/api.github.com.json"
                if cat not in dot["categories"]:
                    e["ghes_only"] = True
                surfaces.append(e)

    with open(OUT, "w") as f:
        json.dump(surfaces, f, indent=2)
        f.write("\n")

    # ------------------------------------------------------------ summary
    p = print
    p(f"rest-api-description commit {sha} ({pins['rest_commit_date']}); newest GHES: {ghes}")
    p(f"REST dotcom: {dot['operations']} ops, {dot['paths']} paths, {len(dot['categories'])} categories, "
      f"{len(dot['subcategories'])} subcategories, {dot['github_cloud_only_flagged']} githubCloudOnly, {dot['deprecated']} deprecated")
    p(f"REST {ghes}: {gh['operations']} ops, {gh['paths']} paths, {len(gh['categories'])} categories, "
      f"{len(gh['subcategories'])} subcategories, {gh['deprecated']} deprecated")
    p(f"REST ops per API version: {json.dumps(by_version, sort_keys=True)}")
    p("dotcom-only categories: " + ", ".join(
        f"{c}({dot['categories'][c]})" for c in sorted(set(dot["categories"]) - set(gh["categories"]))))
    p("GHES-only categories: " + ", ".join(
        f"{c}({gh['categories'][c]})" for c in sorted(set(gh["categories"]) - set(dot["categories"]))))
    p(f"\n{'category':28} {'ghes':>5} {'dotcom':>6}")
    for cat in sorted(set(gh["categories"]) | set(dot["categories"])):
        p(f"{cat:28} {gh['categories'].get(cat, '-'):>5} {dot['categories'].get(cat, '-'):>6}")
    p("\nGraphQL:")
    for plan, s in gql.items():
        p(f"  {plan}: " + ", ".join(f"{k}={v}" for k, v in s.items()))
    p(f"Webhooks dotcom: {wh_dot['events']} events, {wh_dot['event_action_pairs']} event-action pairs")
    p(f"Webhooks {ghes}: {wh_ghes['events']} events, {wh_ghes['event_action_pairs']} event-action pairs")
    p("Webhook events only on dotcom: " + ", ".join(sorted(set(wh_dot["by_event"]) - set(wh_ghes["by_event"]))))
    p(f"Webhook events only on {ghes}: " + ", ".join(sorted(set(wh_ghes["by_event"]) - set(wh_dot["by_event"]))))
    p("Webhook delivery headers (spec): " + ", ".join(wh_ghes["delivery_headers"]))
    if v2:
        p("git v2 advertisement (github.com): " + " | ".join(v2["advertisement"]))
    else:
        p("git v2 advertisement: capture failed (git missing or offline)")
    p(f"\nwrote {OUT} ({len(surfaces)} surfaces); pins in {PINS}")


if __name__ == "__main__":
    main()
