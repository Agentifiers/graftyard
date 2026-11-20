#!/usr/bin/env python3
"""load34: planted bugs in the hosted B0-34 client (tests/am1/load34/), its shared pack builder (tests/am1/rest/wire.mjs)
and the clone lines (tests/fanout/clonelines.py). Plants one mutant at a time, runs the test that gates it, and restores
the file byte for byte. Each line prints CAUGHT, MISSED, or ANCHOR NOT FOUND. Run from the worktree root, with
WRANGLER_CLI pointing at tests/m0's wrangler if this worktree has no npm install there:

    python3 tests/am1/load34/plant.py             # every mutant
    python3 tests/am1/load34/plant.py 0 3         # by number
    python3 tests/am1/load34/plant.py --residue   # every anchor found once in the worktree
"""
import os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..', '..'))
L = 'tests/am1/load34/load34.mjs'
P = 'tests/am1/load34/plan.mjs'
W = 'tests/am1/rest/wire.mjs'
C = 'tests/fanout/clonelines.py'
NODE = lambda pat: ['node', '--test', '--test-name-pattern', pat, 'tests/am1/load34/load34.test.mjs']
WORKERD = NODE('workerd')
PY = ['python3', '-m', 'unittest', 'tests/fanout/test_clonelines.py']

# (name, file, old, new, test)
MUTANTS = [
    # workerd's own deflate in place of pako: a1's bytes in node, other bytes in workerd.
    ('wire: node:zlib deflate, which workerd writes differently', W,
     'import pako from "./vendor/pako_deflate.min.cjs";\n\nconst deflateSync = (b, { level }) => Buffer.from(pako.deflate(b, { level }));',
     'import { deflateSync } from "node:zlib";', WORKERD),
    ('wire: deflate at the default level, not a1\'s level 1', W,
     'deflateSync(b, { level: 1 })', 'deflateSync(b, { level: 6 })', NODE('node writes')),
    ('plan: a capped kind\'s whole cap on each of its clients', P,
     'cap: Math.floor(k.cap / parts) + (part < k.cap % parts ? 1 : 0), part, parts }));', 'cap: k.cap, part, parts }));', NODE('split')),
    ('plan: a kind\'s whole rate on each of its clients', P,
     'return Array.from({ length: parts }, (_, part) => ({ ...k, perS: k.perS / parts,', 'return Array.from({ length: parts }, (_, part) => ({ ...k, perS: k.perS,', NODE('split')),
    ('plan: agents dealt over all clients, roles included', P,
     'const every = (count) => Array.from({ length: Math.max(0, Math.ceil((count - j) / w)) }, (_, k) => j + k * w);',
     'const every = (count) => Array.from({ length: Math.max(0, Math.ceil((count - j) / n)) }, (_, k) => j + k * n);', NODE('split')),
    ('plan: a mark is exclusive (the sample at the kill\'s first ms is j9)', P,
     'now >= t0 + Number(at) * 1000;', 'now > t0 + Number(at) * 1000;', NODE('phases')),
    # The slot wait counted into the sample: the in-runtime queue that the slots exist to keep out.
    ('client: a sample keeps its slot wait', L,
     "this.track(f(c).then((code) => { if (code !== SAMPLED) this.note(kind, this.now() - s - c.waited, code); },",
     "this.track(f(c).then((code) => { if (code !== SAMPLED) this.note(kind, this.now() - s, code); },", WORKERD),
    ('client: no slots, every fetch at once', L,
     '    c.waited += await this.slots.take();\n    let held = true;', '    let held = false;', WORKERD),
    ('client: tip:move sends the next pack from the history\'s start, not after the advertised main', L,
     'const next = at >= 0 ? at + 1 : (this.nextMove ?? 0);', 'const next = (this.nextMove ?? 0) + 1;', WORKERD),
    ('client: tip:move without the advertisement (old tip null)', L,
     'receiveBody("refs/heads/main", cur, want, pack)', 'receiveBody("refs/heads/main", null, want, pack)', WORKERD),
    ('client: hits and misses on the edge clock not split', L,
     '        if (edge) this.note(`${kind}:${cache}:edge`, Number(edge[1]), compose ?? String(r.status));\n', '', WORKERD),
    ('client: a hot network\'s load before its mark', L,
     'const n = hot[role.net], tok = spec.tokens[n.who], from = this.t0 + n.at * 1000;',
     'const n = hot[role.net], tok = spec.tokens[n.who], from = this.t0;', WORKERD),
    ('client: the clone probe\'s miss key from the wrong network', L,
     'key: cloneKeyHex(Number(this.spec.chain.split(":")[1]), want)', 'key: cloneKeyHex(Number(this.spec.chain.split(":")[1]) + 1, want)', WORKERD),
    ('client: forget leaves the spec (and its tokens) in storage', L,
     'async forget() { this.run = null; await this.ctx.storage.deleteAlarm(); await this.ctx.storage.deleteAll(); }',
     'async forget() { this.run = null; await this.ctx.storage.deleteAlarm(); }', WORKERD),
    ('lines: the hits line over every phase\'s hits', C,
     'STEADY, COLD = ["j9"], ["kill", "second"]', 'STEADY, COLD = ["j9", "kill", "second", "hostile"], ["kill", "second"]', PY),
    ('lines: every clone counts hits only', C,
     'cold = {p: by.get(p, {}).get("hit", []) + by.get(p, {}).get("miss", []) for p in COLD}',
     'cold = {p: by.get(p, {}).get("hit", []) for p in COLD}', PY),
    ('lines: the two cold phases pooled into one share', C,
     '"pass": all(s is not None and s >= SHARE for s in share.values()),',
     '"pass": sum(sum(1 for v in xs if v <= EVERY_MS) for xs in cold.values()) >= SHARE * max(1, sum(len(xs) for xs in cold.values())),', PY),
    ('lines: an empty phase passes', C,
     '"pass": all(s is not None and s >= SHARE for s in share.values()),', '"pass": all(s is None or s >= SHARE for s in share.values()),', PY),
    ('lines: 5 s itself counted over', C,
     'share = {p: (round(sum(1 for v in xs if v <= EVERY_MS)', 'share = {p: (round(sum(1 for v in xs if v < EVERY_MS)', PY),
]

ENV = dict(os.environ)


def plant(i, name, path, old, new, test):
    full = os.path.join(ROOT, path)
    with open(full, 'rb') as f:
        orig = f.read()
    src = orig.decode()
    if src.count(old) != 1:
        print(f'{i:2} ANCHOR NOT FOUND ({src.count(old)}): {name}')
        return False
    try:
        with open(full, 'w') as f:
            f.write(src.replace(old, new))
        r = subprocess.run(test, cwd=ROOT, env=ENV, capture_output=True, text=True, timeout=600)
        ok = r.returncode != 0
        print(f'{i:2} {"CAUGHT" if ok else "MISSED"}: {name}')
        return ok
    finally:
        with open(full, 'wb') as f:
            f.write(orig)


def main(args):
    if args == ['--residue']:
        bad = [m[0] for m in MUTANTS if open(os.path.join(ROOT, m[1])).read().count(m[2]) != 1]
        print('residue ok' if not bad else f'anchors missing: {bad}')
        return 0 if not bad else 1
    pick = [int(a) for a in args] or range(len(MUTANTS))
    caught = sum(plant(i, *MUTANTS[i]) for i in pick)
    print(f'{caught} of {len(pick)} caught')
    return 0 if caught == len(pick) else 1


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
