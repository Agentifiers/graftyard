#!/usr/bin/env python3
"""AM2-WP3 hosted: one release folder for `graft-ops` (`crates/release/src/ops.rs`, `check_bundle`) holding only
`front/` and `mcp/` modules, so `next_roll` skips every site, edge, and container day and rolls `FrontRoll` and
`McpRoll` alone. `release.json` is `{label, files: [{name, sha256}]}`, as M5-WP3's bundles, and is signed separately
with the install's release key. `formats.txt` is copied from a bundle the group already took, so the release reads
every enabled format (65000.1 included) and no `UnreadableFormat` refuses it.

    python3 tests/am2/hosted/wp3-bundle.py <out dir> <label> --formats <formats.txt>
        [--front <workers/front/build>] [--mcp <workers/mcp/build>] [--marker]
        [--agents-front <workers/agents-front/build>] [--controller <workers/controller/build>]
        [--controller-entry <workers/controller/entry.mjs>]

AM3-WP1's LF-7 takes `--controller` alone: the agents account's bundle, which `graft-ops` under `Account::Agents`
uploads and rolls as a `ControllerRoll`.

`--marker` adds `<script>/marker.js` (an unused module naming the label) to each script, so the upload is a new
version of the same build. Prints `{"release": <sha256 of release.json>, "files": n}`.
"""
import hashlib
import json
import os
import sys

args = sys.argv[1:]
out, label = args[0], args[1]
flag = lambda f: args[args.index(f) + 1] if f in args else None
os.makedirs(out, exist_ok=False)
files = []


def put(name, data):
    p = os.path.join(out, name)
    os.makedirs(os.path.dirname(p), exist_ok=True)
    open(p, 'wb').write(data)
    files.append({'name': name, 'sha256': hashlib.sha256(data).hexdigest()})


# `--controller-entry <entry.mjs>` (LM-3's kill row): the controller with its `TaskBox`, whose main module is
# `entry.mjs` (the Rust build re-exported beside `Outbound`): `controller/entry.js` and the build under
# `controller/build/`, so `entry.js`'s `./build/index.js` import resolves as it does under wrangler.
entry = flag('--controller-entry')
for script in ('front', 'mcp', 'agents-front', 'controller'):
    build = flag(f'--{script}')
    if not build:
        continue
    if script == 'controller' and entry:
        put('controller/entry.js', open(entry, 'rb').read())
        for m in ('index.js', 'index_bg.wasm'):
            put(f'controller/build/{m}', open(os.path.join(build, m), 'rb').read())
        if '--marker' in args:
            put('controller/marker.js', f'export const label = {json.dumps(label)};\n'.encode())
        continue
    # A JavaScript-only build (R47's bad graft-mcp) has no index_bg.wasm.
    for m in ('index.js', 'index_bg.wasm'):
        if m == 'index.js' or os.path.exists(os.path.join(build, m)):
            put(f'{script}/{m}', open(os.path.join(build, m), 'rb').read())
    if '--marker' in args:
        put(f'{script}/marker.js', f'export const label = {json.dumps(label)};\n'.encode())
put('formats.txt', open(flag('--formats'), 'rb').read())
rec = json.dumps({'label': label, 'files': files}, sort_keys=True).encode()
open(os.path.join(out, 'release.json'), 'wb').write(rec)
print(json.dumps({'release': hashlib.sha256(rec).hexdigest(), 'files': len(files)}))
