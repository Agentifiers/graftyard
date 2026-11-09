#!/usr/bin/env python3
"""Validates every reply the API crate's tests wrote against the pinned GHES 3.22 descriptions (see README.md).

GraphQL: each `<case>.json` under the replies directory holds a captured gh document, its variables, and the reply.
The document is validated against the GHES 3.22 SDL (OverlappingFieldsCanBeMerged excepted, which gh's own documents
break and GitHub does not enforce). Then the reply's `data` is checked by re-executing the document on the schema with
a resolver that returns the reply's own values, so graphql-core checks every value's type, nullability, enum
membership, and list shape, and every selected field's presence; any extra key in the reply fails too.

REST: each `<case>.rest.json` holds a method, an OpenAPI path, a status, and a body, checked with jsonschema against
that operation's response schema in `ghes-3.22.json`, with OpenAPI 3.0's `nullable` honored. The status must be one
the operation documents; a response the description gives no JSON body (a 204, a 302) is checked for an empty body
and, for a 302, a `location` header. A response whose JSON schema is a string (raw content) is skipped.

Webhooks: each `<case>.webhook.json` holds an `x-webhooks` key and a payload (below).

LFS: each `<case>.lfs.json` names one of git-lfs 3.8.0's JSON schemas in `crates/api/lfs/schemas/` and holds a body,
checked with jsonschema (draft 4).

Usage: validate.py <ghes-3.22.json> <schema.docs-enterprise.graphql> <replies-dir>; needs graphql-core, jsonschema."""
import copy, glob, json, os, sys
from graphql import (build_schema, parse, validate, specified_rules, OverlappingFieldsCanBeMergedRule, execute,
                     get_named_type, is_abstract_type)
import jsonschema

openapi_path, sdl_path, replies = sys.argv[1:4]
S = build_schema(open(sdl_path).read(), assume_valid=True)
RULES = [r for r in specified_rules if r is not OverlappingFieldsCanBeMergedRule]
fails, n_gql, n_rest = [], 0, 0


def resolver(source, info, **args):
    key = info.path.key
    return source.get(key) if isinstance(source, dict) else None


def keys_for(info, t, sets):
    """The response keys a selection produces on concrete type `t`, as CollectFields gathers them."""
    out = set()
    for sel_set in sets:
        for sel in sel_set.selections:
            kind = sel.kind
            if kind == 'field':
                out.add(sel.alias.value if sel.alias else sel.name.value)
            elif kind == 'inline_fragment':
                cond = sel.type_condition.name.value if sel.type_condition else t.name
                if applies(info.schema, t, cond):
                    out |= keys_for(info, t, [sel.selection_set])
            elif kind == 'fragment_spread':
                frag = info.fragments[sel.name.value]
                if applies(info.schema, t, frag.type_condition.name.value):
                    out |= keys_for(info, t, [frag.selection_set])
    return out


def applies(schema, t, cond):
    if cond == t.name:
        return True
    c = schema.type_map[cond]
    return is_abstract_type(c) and schema.is_sub_type(c, t)


def type_resolver(value, info, abstract):
    """The concrete type: `__typename` when the reply has it, else the one possible type whose selected response keys
    are exactly the reply's keys. A reply that fits none fails, as it should."""
    if '__typename' in value:
        return value['__typename']
    sets = [n.selection_set for n in info.field_nodes]
    fits = [t.name for t in info.schema.get_possible_types(abstract) if keys_for(info, t, sets) == set(value)]
    return fits[0] if fits else None


for f in sorted(glob.glob(os.path.join(replies, '*.json'))):
    if f.endswith('.rest.json') or f.endswith('.lfs.json') or f.endswith('.webhook.json'):
        continue
    case = json.load(open(f))
    doc = parse(case['query'])
    errs = validate(S, doc, RULES)
    if errs:
        fails.append((f, 'document', [e.message for e in errs]))
        continue
    reply = case['reply']
    if 'errors' in reply and reply.get('data') is None:
        fails.append((f, 'reply', reply['errors']))
        continue
    data = reply['data']
    seen_keys = []
    r = execute(S, doc, root_value=data, variable_values=case.get('variables'), field_resolver=resolver,
                type_resolver=type_resolver)
    if r.errors:
        fails.append((f, 'shape', [f'{e.message} at {e.path}' for e in r.errors]))
    elif r.data != data:
        fails.append((f, 'extra or changed keys', json.dumps(r.data)[:300] + ' != ' + json.dumps(data)[:300]))
    n_gql += 1

spec = json.load(open(openapi_path))


def deref(node, seen=()):
    if isinstance(node, dict):
        if '$ref' in node:
            name = node['$ref']
            parts = name.lstrip('#/').split('/')
            t = spec
            for p in parts:
                t = t[p]
            return deref(t)
        out = {k: deref(v) for k, v in node.items()}
        if out.pop('nullable', False):
            # A nullable enum allows null too (GitHub's open issue has `state_reason: null`).
            if 'enum' in out and None not in out['enum']:
                out['enum'] = out['enum'] + [None]
            if 'type' in out:
                out['type'] = [out['type'], 'null']
            else:
                out = {'anyOf': [out, {'type': 'null'}]}
        return out
    if isinstance(node, list):
        return [deref(x) for x in node]
    return node


for f in sorted(glob.glob(os.path.join(replies, '*.rest.json'))):
    case = json.load(open(f))
    n_rest += 1
    op = spec['paths'][case['path']][case['method'].lower()]
    resp = op['responses'].get(str(case['status']))
    if resp is None:
        fails.append((f, 'rest', f"status {case['status']} is not documented: {sorted(op['responses'])}"))
        continue
    if '$ref' in resp:
        resp = deref(resp)
    content = resp.get('content', {}).get('application/json')
    if content is None:
        if case.get('body') is not None:
            fails.append((f, 'rest', 'a body where the description has none'))
        if case['status'] == 302 and not case.get('location'):
            fails.append((f, 'rest', 'a 302 without a location'))
        continue
    schema = deref(content['schema'])
    try:
        jsonschema.Draft4Validator(schema).validate(case['body'])
    except jsonschema.ValidationError as e:
        fails.append((f, 'rest', f'{e.message} at {list(e.absolute_path)}'))

n_lfs = 0
lfs_dir = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'lfs', 'schemas')
for f in sorted(glob.glob(os.path.join(replies, '*.lfs.json'))):
    case = json.load(open(f))
    n_lfs += 1
    schema = json.load(open(os.path.join(lfs_dir, case['schema'])))
    try:
        jsonschema.Draft4Validator(schema).validate(case['body'])
    except jsonschema.ValidationError as e:
        fails.append((f, 'lfs', f'{e.message} at {list(e.absolute_path)}'))

# Webhooks: each `<case>.webhook.json` holds a `key` of the `x-webhooks` section (`push`, `pull-request-opened`) and a
# payload, checked against that key's request body schema. A key GHES 3.22 lacks (`merge-group-*`) is checked against
# dotcom's description at the same pinned commit, when `$DOTCOM_OPENAPI` names it; otherwise it fails.
n_hooks = 0
dotcom = json.load(open(os.environ['DOTCOM_OPENAPI'])) if os.environ.get('DOTCOM_OPENAPI') else None
for f in sorted(glob.glob(os.path.join(replies, '*.webhook.json'))):
    case = json.load(open(f))
    n_hooks += 1
    src, key = spec, case['key']
    if key not in spec['x-webhooks']:
        if dotcom is None or key not in dotcom['x-webhooks']:
            fails.append((f, 'webhook', f'{key} is in no description'))
            continue
        src = dotcom
    saved, spec = spec, src
    schema = deref(src['x-webhooks'][key]['post']['requestBody']['content']['application/json']['schema'])
    spec = saved
    try:
        jsonschema.Draft4Validator(schema).validate(case['body'])
    except jsonschema.ValidationError as e:
        fails.append((f, 'webhook', f'{e.message[:200]} at {list(e.absolute_path)}'))

for f, kind, why in fails:
    print('FAIL', os.path.basename(f), kind, why)
print(f'{n_gql} GraphQL replies, {n_rest} REST replies, {n_lfs} LFS replies, and {n_hooks} webhook payloads checked, '
      f'{len(fails)} failed')
sys.exit(1 if fails else 0)
