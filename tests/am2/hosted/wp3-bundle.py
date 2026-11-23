#!/usr/bin/env python3
"""AM2-WP3 hosted: one release folder for `graft-ops` (`crates/release/src/ops.rs`, `check_bundle`) holding only
`front/` and `mcp/` modules, so `next_roll` skips every site, edge, and container day and rolls `FrontRoll` and
`McpRoll` alone. `release.json` is `{label, files: [{name, sha256}]}`, as M5-WP3's bundles, and is signed separately
with the install's release key. `formats.txt` is copied from a bundle the group already took, so the release reads
every enabled format (65000.1 included) and no `UnreadableFormat` refuses it.

    python3 tests/am2/hosted/wp3-bundle.py <out dir> <label> --formats <formats.txt>
        [--front <workers/front/build>] [--mcp <workers/mcp/build>] [--marker]

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


for script in ('front', 'mcp'):
    build = flag(f'--{script}')
    if not build:
        continue
    for m in ('index.js', 'index_bg.wasm'):
        put(f'{script}/{m}', open(os.path.join(build, m), 'rb').read())
    if '--marker' in args:
        put(f'{script}/marker.js', f'export const label = {json.dumps(label)};\n'.encode())
put('formats.txt', open(flag('--formats'), 'rb').read())
rec = json.dumps({'label': label, 'files': files}, sort_keys=True).encode()
open(os.path.join(out, 'release.json'), 'wb').write(rec)
print(json.dumps({'release': hashlib.sha256(rec).hexdigest(), 'files': len(files)}))
