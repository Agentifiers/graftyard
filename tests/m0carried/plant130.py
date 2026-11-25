#!/usr/bin/env python3
"""M6-WP1's hosted glue: a `ReplaceMember` ask that moves a group which never held the lost site, a restore planned
as a dead site's replace, a day-long health read summed from the 10 s buckets, `deleteAll()` before 35 days, a probe of
the prober's own site, a single-copy refill pass that copies everything, and the Workflows coordinator taking a chain's
lowest generation, a group asking a member on the lost site, or a group following a leader on the lost site
instead of asking a live voter to campaign.

Plants one mutant at a time, runs the tests that guard it, and restores the file byte for byte. Each line prints
CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant130.py          # every mutant
    python3 tests/m0carried/plant130.py 0 1      # by number
"""
import os
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
MOVE = 'crates/identity/src/group_move.rs'
GATE = 'workers/site/src/gatecount.rs'
REPL = 'workers/site/src/replace.rs'
SWEEP = 'crates/git/src/sweep.rs'
WF = 'workers/workflows/move.mjs'
MOVE_T = (['cargo', 'test', '-q', '-p', 'graftyard-identity', '--test', 'group_move'], '.')
SITE_T = (['cargo', 'test', '-q', '-p', 'graftyard-site', '--lib', '--', 'gatecount', 'replace'], '.')
SWEEP_T = (['cargo', 'test', '-q', '-p', 'graftyard-git', '--lib', '--', 'refill_runs'], '.')
WF_T = (['node', '--test', 'workers/workflows/tests/move.test.mjs'], '.')

MUTANTS = [
    ("a group that never held the lost site is moved anyway", MOVE,
     "    if !started && !holds(cfg, from) {\n", "    if !started && !holds(cfg, from) && false {\n", MOVE_T),
    ("a restore is planned as a dead site's replace", MOVE,
     "        _ if dead => replace_step(cfg, leader, from, to),\n",
     "        _ if dead || true => replace_step(cfg, leader, from, to),\n", MOVE_T),
    ("a day-long health read sums the 10 s buckets", GATE,
     "    if now.saturating_sub(since.max(0) as u64) > graftyard_release::gate::UNDRAIN_AFTER {\n",
     "    if false {\n", SITE_T),
    ("a removed member is wiped a moment before 35 days", REPL,
     "    now.saturating_sub(removed_at) >= DELETE_AFTER\n", "    now.saturating_sub(removed_at) + 1 >= DELETE_AFTER\n",
     SITE_T),
    ("a site probes itself", REPL,
     ".filter(|s| *s != me.0).collect();", ".collect();", SITE_T),
    ("the single-copy refill pass copies every pack", SWEEP,
     "        if !single {\n            fills.extend(rest);\n", "        if true {\n            fills.extend(rest);\n",
     SWEEP_T),
    ("the coordinator keeps a chain's lowest generation", WF,
     "    if (!e || r.generation > e.generation) by.set(r.chain, r);\n",
     "    if (!e || r.generation < e.generation) by.set(r.chain, r);\n", WF_T),
    ("a group follows a leader named on the lost site instead of asking for a campaign", WF,
     "    if (lost !== null && (l === null || l === undefined || l === lost)) return { campaign: true };\n",
     "    if (lost !== null && (l === null || l === undefined)) return { campaign: true };\n", WF_T),
    ("a group asks a member on the lost site", WF,
     "    const live = members.filter((m) => !(p.dead && m.site === p.from));\n",
     "    const live = members;\n", WF_T),
]

def main():
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    caught = 0
    for i in pick:
        name, f, old, new, (cmd, cwd) = MUTANTS[i]
        p = os.path.join(ROOT, f)
        orig = open(p, 'rb').read()
        text = orig.decode()
        if text.count(old) != 1:
            print(f'{i} {name}: anchor found {text.count(old)} times')
            sys.exit(2)
        try:
            open(p, 'w').write(text.replace(old, new))
            r = subprocess.run(cmd, cwd=os.path.join(ROOT, cwd), capture_output=True, text=True, timeout=1800)
            out = r.stdout + r.stderr
            if r.returncode == 0:
                verdict = 'MISSED'
            elif 'error[E' in out or 'could not compile' in out or '[build failed]' in out:
                verdict = 'DID NOT COMPILE'
            else:
                verdict = 'CAUGHT'
                caught += 1
        finally:
            open(p, 'wb').write(orig)
        print(f'{i} {name}: {verdict}', flush=True)
    print(f'{caught} of {len(pick)} caught')
    sys.exit(0 if caught == len(pick) else 1)


if __name__ == '__main__':
    main()
