#!/usr/bin/env python3
"""Turns bf32.mjs's REST replies (<raw>/bf32/<case>.json) and identity.sh's (<raw>/identity/<case>.json) into
`crates/api/gh/validate.py` cases (`<case>.rest.json`): method, OpenAPI path, status, body. Only replies to
`/api/v3` operations the pinned description lists are written; the OAuth endpoints are not in it.

Usage: rest-cases.py <raw dir> <replies dir>"""
import glob, json, os, sys

raw, replies = sys.argv[1:3]
os.makedirs(replies, exist_ok=True)
# bf32.mjs's cases by name: the operation each one called.
BF32 = {
    'access-token': ('POST', '/app/installations/{installation_id}/access_tokens'),
    'narrowed': ('POST', '/app/installations/{installation_id}/access_tokens'),
    'lacks-issues': ('POST', '/app/installations/{installation_id}/access_tokens'),
    'above-level': ('POST', '/app/installations/{installation_id}/access_tokens'),
    'other-key': ('POST', '/app/installations/{installation_id}/access_tokens'),
    'installation-read': ('GET', '/repos/{owner}/{repo}/issues'),
    'revoke': ('DELETE', '/installation/token'),
}
n = 0
for f in sorted(glob.glob(os.path.join(raw, 'bf32', '*.json'))) + sorted(glob.glob(os.path.join(raw, 'identity', '*.json'))):
    name = os.path.basename(f)[:-5]
    case = json.load(open(f))
    if 'status' not in case:
        continue
    op = BF32.get(name) or (case.get('method'), case.get('path'))
    if not op[0]:
        continue
    body = case.get('body')
    # The token's text was replaced by its length before the file was written; the schema only needs a string.
    out = {'method': op[0], 'path': op[1], 'status': case['status'], 'body': body, 'location': None}
    json.dump(out, open(os.path.join(replies, f'm3-{name}.rest.json'), 'w'), indent=1)
    n += 1
print(f'{n} REST cases written')
