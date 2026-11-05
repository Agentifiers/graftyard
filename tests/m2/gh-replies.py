#!/usr/bin/env python3
"""Turns the replies a hosted edge served gh into `crates/api/gh/validate.py` cases.

Reads every `*.err` that `tests/m2/edge-drive.sh` wrote with `GH_DEBUG=api` and writes, for each 2xx reply with a
JSON body:
- a GraphQL request and reply as `<cmd>-<n>.json` (`query`, `variables`, `reply`);
- a REST reply as `<cmd>-<n>.rest.json` (`method`, the OpenAPI path, `status`, `body`, `location`).
A 302 is written with its `location`. Requests to paths the pinned description does not list (the upload host, the
codeload host) are counted and skipped, as is a reply whose body is not JSON (an asset's bytes).

gh sends some requests at once (`release upload` asks REST and GraphQL for the release, and cancels the loser), so
two requests can log before either reply, and replies can come back in either order. A body with `data` or `errors`
answers a GraphQL request: the pending one whose document names each of `data`'s keys, else the oldest. Any other
body answers the oldest pending REST request.

Usage: gh-replies.py <drive outdir> <replies dir> <ghes-3.22.json>"""
import glob, json, os, re, sys

out_dir, replies, spec_path = sys.argv[1:4]
os.makedirs(replies, exist_ok=True)
paths = json.load(open(spec_path))['paths']
templates = [(p, re.compile('^' + re.sub(r'\{[^/]+\}', '[^/]+', p) + '$')) for p in paths]


def openapi_path(method, url_path):
    """The description's path for a request: under `/api/v3`, or under `/api/uploads` (the asset upload's server)."""
    p = url_path.split('?')[0]
    base = next((b for b in ('/api/v3', '/api/uploads') if p.startswith(b)), None)
    if base is None:
        return None
    p = p[len(base):] or '/'
    hits = [t for t, rx in templates if rx.match(p) and method.lower() in paths[t]]
    # The most literal template wins: `/releases/latest` over `/releases/{release_id}`.
    return min(hits, key=lambda t: t.count('{')) if hits else None


def requests(text):
    """Each request in log order: method, path, and for GraphQL its document and variables. gh logs a document either
    as `GraphQL query:` lines or, with no operation name, as the request's JSON body, and may log two requests'
    headers before either body."""
    out = []
    for part in re.split(r'^(?=\* Request to )', text, flags=re.M)[1:]:
        m = re.search(r'^> (GET|POST|PUT|PATCH|DELETE) (\S+) ', part, re.M)
        if not m:
            continue
        out.append([m.group(1), m.group(2), None])
    gql = [r for r in out if r[1].startswith('/api/graphql')]
    docs = [(m.start(), m.group(1), json.loads(m.group(2)))
            for m in re.finditer(r'^GraphQL query:\n(.*?)\nGraphQL variables: (.*?)\n', text, re.M | re.S)]
    for at in re.finditer(r'^\{\n  "query": ', text, re.M):
        b = json.JSONDecoder().raw_decode(text, at.start())[0]
        docs.append((at.start(), b['query'], b.get('variables')))
    docs = [(q, v) for _, q, v in sorted(docs, key=lambda d: d[0])]
    for r in gql:
        r[2] = docs.pop(0) if docs else None
    return out


def exchanges(text):
    """Each reply with the request it answers: method, path, GraphQL query and variables, status, location, body. A
    GraphQL reply answers the pending document whose aliases or fields name each of `data`'s keys, else the oldest; a
    REST reply answers the oldest pending REST request."""
    pending = {'graphql': [], 'rest': []}
    for method, path, q in requests(text):
        pending['graphql' if path.startswith('/api/graphql') else 'rest'].append((method, path, q))
    names = lambda d: set(re.findall(r'(\w+)\s*[:({]', d))
    # Two replies' headers can interleave, so one header block has no body and the next is followed by both bodies
    # (one after its `* Request took` line). A header block's status waits until a body takes it.
    statuses = []
    for part in re.split(r'^(?=< HTTP/)', text, flags=re.M)[1:]:
        statuses.append((int(part.split()[2]), re.search(r'^< Location: (\S+)', part, re.M | re.I)))
        bodies = []
        for at in re.finditer(r'^[{\[]', part, re.M):
            if part[:at.start()].rstrip('\n').split('\n')[-1].startswith('> '):
                continue
            try:
                bodies.append(json.JSONDecoder().raw_decode(part, at.start())[0])
            except ValueError:
                continue
        if not bodies and '\n\n' in part and part.split('\n\n', 1)[1].strip()[:1] not in ('', '*', '<'):
            bodies.append(ValueError)
        if not bodies and re.search(r'\n\n\* Request took', part):
            # gh logs no body that is not JSON (an asset's bytes).
            json_type = re.search(r'^< Content-Type: application/json', part, re.M | re.I)
            bodies.append(None if not json_type and statuses[-1][0] == 302 else ValueError if not json_type else None)
        for value in bodies:
            if not statuses:
                break
            status, loc = statuses.pop(0)
            kind = 'graphql' if isinstance(value, dict) and ('data' in value or 'errors' in value) else 'rest'
            if not pending[kind]:
                continue
            keys = set((value.get('data') or {}).keys()) if kind == 'graphql' else set()
            i = next((i for i, (_, _, q) in enumerate(pending[kind]) if q and keys <= names(q[0])), 0)
            method, path, q = pending[kind].pop(i)
            yield method, path, q, status, loc.group(1) if loc else None, value


counts = {'graphql': 0, 'rest': 0, 'skipped': 0}
for f in sorted(glob.glob(os.path.join(out_dir, '*.err'))):
    tag = os.path.basename(f)[:-4]
    for n, (method, path, q, status, loc, value) in enumerate(exchanges(open(f, errors='replace').read()), 1):
        if status >= 400:
            continue
        if value is ValueError:
            counts['skipped'] += 1
            continue
        if path.startswith('/api/graphql') and q:
            case = {'query': q[0], 'variables': q[1], 'reply': value}
            json.dump(case, open(os.path.join(replies, f'{tag}-{n}.json'), 'w'), indent=1)
            counts['graphql'] += 1
            continue
        op = openapi_path(method, path)
        if op is None:
            counts['skipped'] += 1
            continue
        case = {'method': method, 'path': op, 'status': status, 'body': value, 'location': loc}
        json.dump(case, open(os.path.join(replies, f'{tag}-{n}.rest.json'), 'w'), indent=1)
        counts['rest'] += 1
print(f"{counts['graphql']} GraphQL and {counts['rest']} REST cases written, {counts['skipped']} replies skipped "
      "(no path in the description, or not JSON)")

# git-lfs: each `GIT_TRACE=1 GIT_CURL_VERBOSE=1` trace (`*.trace`) logs a reply body as `trace git-lfs: HTTP: <chunk>`
# lines after the request's `HTTP: <status>` line. Each 2xx JSON body of a batch, lock, locks, or lock-verify request
# becomes `<trace>-<n>.lfs.json`, naming git-lfs 3.8.0's schema for it.
LFS_SCHEMAS = [('POST', '/info/lfs/objects/batch', 'http-batch-response-schema.json'),
               ('POST', '/info/lfs/locks/verify', 'http-lock-verify-response-schema.json'),
               ('POST', '/info/lfs/locks', 'http-lock-create-response-schema.json'),
               ('GET', '/info/lfs/locks', 'http-lock-list-response-schema.json')]
n_lfs = 0
for f in sorted(glob.glob(os.path.join(out_dir, '*.trace'))):
    tag = os.path.basename(f)[:-6]
    req, status, chunks, n = None, None, [], 0

    def flush():
        global n_lfs, n
        if req and status and 200 <= status < 300 and chunks:
            schema = next((s for m, p, s in LFS_SCHEMAS if req[0] == m and req[1].endswith(p)), None)
            try:
                body = json.loads(''.join(chunks))
            except ValueError:
                return
            if schema:
                n += 1
                json.dump({'schema': schema, 'body': body}, open(os.path.join(replies, f'{tag}-{n}.lfs.json'), 'w'))
                n_lfs += 1

    for line in open(f, errors='replace'):
        m = re.search(r'trace git-lfs: HTTP: (.*)$', line.rstrip('\n'))
        if not m:
            continue
        rest = m.group(1)
        r = re.match(r'(GET|POST|PUT|DELETE) https?://[^/]+(\S*)$', rest)
        if r:
            flush()
            req, status, chunks = (r.group(1), r.group(2)), None, []
        elif re.fullmatch(r'\d{3}', rest) and status is None:
            status = int(rest)
        elif status is not None:
            chunks.append(rest)
    flush()
print(f'{n_lfs} LFS cases written')
