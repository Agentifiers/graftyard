#!/usr/bin/env python3
"""Writes `../src/introspect.txt`: the field and enum-value names of the types gh 2.102.1 probes with `__type`
(`PullRequest_fields`, `PullRequest_fields2`, `Issue_fields`, `SearchType_enumValues`), from the pinned GHES 3.22 SDL.
gh picks the fields of its later queries from these answers, so they must equal GHES's. A trailing `!` marks a
deprecated name. Usage: introspect.py <schema.docs-enterprise.graphql>; needs graphql-core."""
import os, sys
from graphql import build_schema, GraphQLEnumType

PROBED = ['Issue', 'PullRequest', 'SearchType', 'StatusCheckRollupContextConnection', 'WorkflowRun']
S = build_schema(open(sys.argv[1]).read(), assume_valid=True)
out = []
for name in PROBED:
    t = S.type_map[name]
    if isinstance(t, GraphQLEnumType):
        items = [(v, d.deprecation_reason) for v, d in t.values.items()]
        kind = 'E'
    else:
        items = [(f, d.deprecation_reason) for f, d in t.fields.items()]
        kind = 'F'
    out.append(' '.join([name, kind] + [n + ('!' if dep else '') for n, dep in items]))
path = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'src', 'introspect.txt')
open(path, 'w').write('\n'.join(out) + '\n')
print(path, len(out), 'types')
