#!/usr/bin/env python3
"""merge-budget: planted bugs in the test-merge budget (`graftyard_git::prsync::MergeBudget`, `Budgets`, `budgeted`,
`take`), the leader's `/stats` field, and the A1 driver's B0-35 window. Plants one mutant at a time, runs the tests
that gate it, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the
worktree root:

    python3 tests/budget/plant.py          # every mutant
    python3 tests/budget/plant.py 0 3      # by number
"""
import os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
PS = 'crates/git/src/prsync.rs'
HOST = 'workers/site/src/host.rs'
A1 = 'tests/am1/rest/a1.mjs'
GIT = ['cargo', '-p', 'graftyard-git', '--test', 'agent_load']
SYNC = ['cargo', '-p', 'graftyard-git', '--test', 'prsync']
SITE = ['cargo', '-p', 'graftyard-site', '--lib', 'first_attempts']
NODE = ['node', A1, '--selftest']

MUTANTS = [
    # The bugs A27 names, and the ones the task names.
    ('never charged: budgeted runs the merge and charges nothing', PS,
     '    budgets.borrow_mut().of(net, t0).charge(cause, MERGE_EST_MS, real);',
     '    let _ = (real, cause);', GIT + SYNC),
    ('charge keeps the estimate: the measured time never reaches the bucket', PS,
     '        self.ms -= real_ms - est_ms;', '        let _ = est_ms;', SYNC),
    ('charge counts nothing spent', PS, '        self.stats.spent_ms += f64::from(real_ms);', '', GIT),
    ('per caller, not per network: every call gets a fresh budget', PS,
     '        self.of.entry(net).or_insert_with(|| MergeBudget::new(self.vcpu, now))',
     '        self.of.insert(net, MergeBudget::new(self.vcpu, now));\n        self.of.get_mut(&net).expect("just put")', GIT),
    ('one budget for every network in the shard', PS,
     '        self.of.entry(net).or_insert_with(|| MergeBudget::new(self.vcpu, now))',
     '        let _ = net;\n        self.of.entry(NetworkId(graftyard_types::DbId(0))).or_insert_with(|| MergeBudget::new(self.vcpu, now))',
     GIT),
    ('never shed: a push runs however far past the budget', PS,
     '            MergeCause::HeadPush | MergeCause::Dispatch => self.ms > -est_ms,',
     '            MergeCause::HeadPush | MergeCause::Dispatch => true,', GIT + SYNC),
    ('shed answer is recorded as a merge (mergeable not null)', PS,
     '        MergeAnswer::Shed { .. } => shed.push((n, head)),', '        MergeAnswer::Shed { .. } => {}', SYNC),
    ('a list field computes', PS, '            MergeCause::List => false,', '            MergeCause::List => self.ms > 0.0,',
     GIT),
    ('list asks counted as shed', PS, '        } else if cause == MergeCause::List {\n            self.stats.list_asked += 1;',
     '        } else if false {\n            self.stats.list_asked += 1;', GIT),
    ('a list-field compute is not counted', PS,
     '        if cause == MergeCause::List {\n            self.stats.list_computes += 1;',
     '        if false {\n            self.stats.list_computes += 1;', GIT),
    ('shed merges not counted', PS, '            self.stats.shed += 1;', '', GIT + SYNC),
    ('the catch-up is charged with the merge', PS,
     '    prepare().await?;\n    let t0 = clock();', '    let t0 = clock();\n    prepare().await?;', GIT),
    ('the budget ignores the shard size (always standard-4)', PS,
     '        let rate = TEST_MERGE_BUDGET * vcpu * 1000.0;', '        let rate = TEST_MERGE_BUDGET * SHARD_VCPU * 1000.0;\n        let _ = vcpu;',
     SYNC),
    ('a shed merge still runs the catch-up', PS,
     '        return Ok(None);\n    }\n    prepare().await?;', '        prepare().await?;\n        return Ok(None);\n    }\n    prepare().await?;',
     GIT),
    ('/stats: a first attempt with no merge wipes the shard counts', HOST,
     '        if !v["budget"].is_null() {\n            self.test_merges = Some(v["budget"].clone());\n        }',
     '        self.test_merges = Some(v["budget"].clone());', SITE),
    ('/stats: asks overwritten, not added', HOST,
     '        self.test_merge_asks += v["asked"].as_u64().unwrap_or(0) as u32;',
     '        self.test_merge_asks = v["asked"].as_u64().unwrap_or(0) as u32;', SITE),
    ('driver: a restarted shard is subtracted from its old counts', A1,
     '    const restarted = !b || b.since !== a.since;', '    const restarted = !b;', NODE),
    ('driver: vCPU over the window counts only one site', A1,
     '  const vcpuUsed = +(sum("spentMs") / (seconds * 1000)).toFixed(4);',
     '  const vcpuUsed = +(sites[0].spentMs / (seconds * 1000)).toFixed(4);', NODE),
]

ENV = dict(os.environ, PATH=os.path.expanduser('~/.cargo/bin') + ':' + os.environ['PATH'], CARGO_INCREMENTAL='0')


def run(t):
    if t[0] == 'node':
        r = subprocess.run(t, cwd=ROOT, capture_output=True, text=True, timeout=120)
        out = r.stdout + r.stderr
        return r.returncode, out, 'SyntaxError' in out
    out, broke = '', False
    # Each cargo target in the list, in turn; the first failure decides.
    for i in range(0, len(t), 5):
        _, pkg_flag, pkg, kind, name = t[i:i + 5]
        args = ['cargo', 'test', '-q', pkg_flag, pkg, kind, name]
        r = subprocess.run(args, cwd=ROOT, env=ENV, capture_output=True, text=True, timeout=1200)
        out += r.stdout + r.stderr
        broke = broke or 'error[E' in out or 'could not compile' in out
        if r.returncode != 0:
            return r.returncode, out, broke
    return 0, out, broke


def main():
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    caught = 0
    for i in pick:
        name, f, old, new, t = MUTANTS[i]
        p = os.path.join(ROOT, f)
        orig = open(p, 'rb').read()
        s = orig.decode()
        if s.count(old) != 1:
            print(f'{i:2} NOT FOUND  {name}')
            continue
        open(p, 'w').write(s.replace(old, new))
        try:
            code, out, broke = run(t)
        finally:
            open(p, 'wb').write(orig)
        if broke:
            print(f'{i:2} DID NOT COMPILE  {name}')
        elif code != 0:
            caught += 1
            fails = [l.split()[1] for l in out.splitlines() if l.startswith('---- ')]
            print(f'{i:2} CAUGHT  {name}  [{", ".join(fails[:3])}]')
        else:
            print(f'{i:2} MISSED  {name}')
        sys.stdout.flush()
    print(f'{caught} of {len(list(pick))} caught')


if __name__ == '__main__':
    main()
