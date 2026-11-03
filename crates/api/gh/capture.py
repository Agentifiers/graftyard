#!/usr/bin/env python3
"""Turns a gh capture log into the committed fixtures: one `<Operation>-<n>.json` per distinct GraphQL document gh
sent, with the gh command that sent it, and `rest.txt` for its REST requests.

The log comes from a pinned gh run against a local TLS capture server that answered from the pinned GHES 3.22 schema
(see README.md). Usage: capture.py <log.jsonl> <out-dir>"""
import json, re, sys

log, out = sys.argv[1], sys.argv[2]
docs, rest, n = {}, [], {}
for line in open(log):
    r = json.loads(line)
    if r['path'] != '/api/graphql':
        rest.append(f"{r['method']} {re.sub(r'/repos/o/r/', '/repos/{owner}/{repo}/', r['path'])}\t{r['tag']}")
        continue
    b = json.loads(r['body'])
    q = b['query']
    if q in docs:
        docs[q]['commands'].append(r['tag'])
        continue
    op = re.search(r'(query|mutation)\s*(\w+)', q).group(2)
    n[op] = n.get(op, 0) + 1
    docs[q] = {'operation': op, 'file': f'{op}-{n[op]}.json', 'query': q, 'variables': b.get('variables'),
               'commands': [r['tag']], 'user_agent': r['headers'].get('User-Agent')}
for d in docs.values():
    f = d.pop('file')
    json.dump(d, open(f'{out}/{f}', 'w'), indent=1, sort_keys=True)
    open(f'{out}/{f}', 'a').write('\n')
open(f'{out}/rest.txt', 'w').write(''.join(sorted(set(x + '\n' for x in rest))))
print(len(docs), 'documents,', len(rest), 'REST requests')
