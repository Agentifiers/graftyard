#!/usr/bin/env python3
"""merge-budget: planted bugs in the test-merge budget (`graftyard_git::prsync::MergeBudget`, `Budgets`, `budgeted`,
`take`, `on_demand`), the edge's GraphQL merge asks (`graftyard_api::gql::MergeAsks`), the leader's `/stats` field,
the A1 driver's B0-35 window, and merge-budget-3's REST asks, stored counts, and in-line count. Plants one mutant at a time, runs the tests
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
GQL = 'crates/api/src/gql.rs'
REST = 'crates/api/src/rest.rs'
GIT = ['cargo', '-p', 'graftyard-git', '--test', 'agent_load']
SYNC = ['cargo', '-p', 'graftyard-git', '--test', 'prsync']
SITE = ['cargo', '-p', 'graftyard-site', '--lib', 'first_attempts']
NODE = ['node', A1, '--selftest']
API = ['cargo', '-p', 'graftyard-api', '--test', 'surfaces']

MUTANTS = [
    # merge-budget's bugs, on the code as merge-budget-2 left it.
    ('never charged: the merge runs and nothing is charged', PS,
     '    budgets.borrow_mut().of(net, t0).charge(cause, MERGE_EST_MS, cpu, wall);', '    let _ = (cpu, wall, t0);',
     GIT + SYNC),
    ('charge keeps the estimate: the measured cost never reaches the bucket', PS,
     '        self.ms -= cpu_ms - est_ms;', '        let _ = est_ms;', GIT + SYNC),
    ('charge counts nothing spent', PS, '        s.spent_ms += f64::from(cpu_ms);', '', GIT),
    ('per caller, not per network: every call gets a fresh budget', PS,
     '        self.of.entry(net).or_insert_with(|| MergeBudget::new(self.vcpu, now))',
     '        self.of.insert(net, MergeBudget::new(self.vcpu, now));\n        self.of.get_mut(&net).expect("just put")', GIT),
    ('one budget for every network in the shard', PS,
     '        self.of.entry(net).or_insert_with(|| MergeBudget::new(self.vcpu, now))',
     '        let _ = net;\n        self.of.entry(NetworkId(graftyard_types::DbId(0))).or_insert_with(|| MergeBudget::new(self.vcpu, now))',
     GIT),
    ('never shed: a push runs however far past the budget', PS,
     '            MergeCause::HeadPush | MergeCause::Dispatch => self.ms > 0.0,',
     '            MergeCause::HeadPush | MergeCause::Dispatch => true,', GIT + SYNC),
    ('shed answer is recorded as a merge (mergeable not null)', PS,
     '        MergeAnswer::Shed { .. } => shed.push((n, head)),', '        MergeAnswer::Shed { .. } => {}', SYNC),
    ('a list ask passes on to the budget', PS, '                self.stats.list_asked += 1;\n                false',
     '                self.stats.list_asked += 1;\n                true', GIT),
    ('list asks not counted', PS, '                self.stats.list_asked += 1;\n', '', GIT),
    ('a list-field compute is not counted', PS, '            MergeCause::List => s.list_computes += 1,',
     '            MergeCause::List => {}', GIT),
    ('shed merges not counted', PS, '            self.stats.shed += 1;', '', GIT + SYNC),
    ('the catch-up is charged with the merge', PS,
     [('    prepare().await?;\n    let turn', '    let turn'),
      ('    let t0 = clock();\n    let r = run().await;', '    let t0 = clock();\n    prepare().await?;\n    let r = run().await;')],
     None, GIT),
    ('the budget ignores the shard size (always standard-4)', PS,
     '        let rate = TEST_MERGE_BUDGET * vcpu * 1000.0;', '        let rate = TEST_MERGE_BUDGET * 4.0 * 1000.0;\n        let _ = vcpu;',
     GIT + SYNC),
    ('a shed merge still runs the catch-up', PS, '        if !b.would(cause, MERGE_EST_MS, now) {',
     '        if false && !b.would(cause, MERGE_EST_MS, now) {', GIT),
    ('/stats: a first attempt with no merge wipes the shard counts', HOST,
     '        if !v["budget"].is_null() {\n            self.test_merges = Some(v["budget"].clone());\n        }',
     '        self.test_merges = Some(v["budget"].clone());', SITE),
    ('/stats: asks overwritten, not added', HOST,
     '        self.test_merge_asks += v["asked"].as_u64().unwrap_or(0) as u32;',
     '        self.test_merge_asks = v["asked"].as_u64().unwrap_or(0) as u32;', SITE),
    ('driver: a restarted shard is subtracted from its old counts', A1,
     '    const restarted = !b || b.since !== a.since;', '    const restarted = !b;', NODE),
    ('driver: the overrun line ignores the bucket low', A1,
     '  const overrun = sites.some((x) => x.lowMs < -Math.max(x.maxMs, 50) - 0.5);', '  const overrun = false;', NODE),
    ('driver: a shard restarted after the load loses the run\'s last sample', A1,
     '? seen[s] ?? b : b);', '? b : b);', NODE),
    ('driver: vCPU over the window counts only one site', A1,
     '  const vcpuUsed = +(sum("spentMs") / (seconds * 1000)).toFixed(4);',
     '  const vcpuUsed = +(sites[0].spentMs / (seconds * 1000)).toFixed(4);', NODE),
    # A28: admission that lets concurrent merges overrun.
    ('A28: no turn, so merges in flight are admitted before the ones ahead are charged', PS,
     '    let _turn = turn.turn().await;', '    let _turn = &turn;', GIT),
    ('A28: the budget decides once, before the catch-up and the turn', PS,
     '    if !budgets.borrow_mut().of(net, now).decide(cause, n, MERGE_EST_MS, now) {', '    if !true {', GIT),
    ('A28: pushes admitted down to minus the estimate, as before', PS,
     '            MergeCause::HeadPush | MergeCause::Dispatch => self.ms > 0.0,',
     '            MergeCause::HeadPush | MergeCause::Dispatch => self.ms > -est_ms,', GIT),
    ('A28: the merge is charged its wall time, not its CPU', PS,
     '    let cpu = r.as_ref().ok().and_then(|(_, c)| *c).unwrap_or(wall);', '    let cpu = wall;', GIT),
    ('A28: the overrun instrument never records the low', PS, '        s.low_ms = s.low_ms.min(self.ms);', '', GIT),
    # A30: GETs.
    ('A30: GETs never coalesce', PS,
     '                let fresh = self.got.get(&n).is_none_or(|t| now >= t + GET_COALESCE_MS);',
     '                let fresh = true;', GIT),
    ('A30: a shed GET keeps its coalescing slot', PS, '                self.got.remove(&n);', '', GIT),
    ('A30: GET asks not counted', PS, '                self.stats.get_asked += 1;\n', '', GIT),
    ('A30: GET computes not counted', PS, '            MergeCause::Get => s.get_computes += 1,',
     '            MergeCause::Get => {}', GIT),
    ('A30: a GET ignores the reserve kept for pushes', PS,
     '            MergeCause::Get => self.ms - est_ms >= GET_RESERVE * self.rate,',
     '            MergeCause::Get => self.ms > 0.0,', GIT),
    ('A30: an on-demand merge moves the cursor', PS,
     '        through: None,\n        heads: Vec::new(),\n        merges: vec![m],',
     '        through: Some(Index(1)),\n        heads: Vec::new(),\n        merges: vec![m],', SYNC),
    ('A30: an on-demand merge past the command bound is committed', PS,
     '    if matches!(&m.pack, Some(PackRef::Inline(b)) if b.len() > MERGE_BYTES_PER_COMMAND) {',
     '    if false && matches!(&m.pack, Some(PackRef::Inline(b)) if b.len() > MERGE_BYTES_PER_COMMAND) {', SYNC),
    # A29 and A30 on the edge's read.
    ('A29: a list read asks for computes', GQL, '                    if self.listed {', '                    if false {', API),
    ('A30: a PR with its current merge recorded still asks', GQL, '        (None, true, Some(base)) => Some((base, head.head_sha)),',
     '        (_, true, Some(base)) => Some((base, head.head_sha)),', API),
    ('A30: the ask names the recorded base, not the tip', GQL,
     '        (None, true, Some(base)) => Some((base, head.head_sha)),',
     '        (None, true, Some(_)) => Some((pull.base_sha, head.head_sha)),', API),
    # merge-budget-3.
    ('mb3 1: REST asks nothing', REST, '            if let Some(pair) = ask {\n                asks.single.insert(n, pair);',
     '            if let Some(pair) = ask.filter(|_| false) {\n                asks.single.insert(n, pair);', API),
    ('mb3 1: REST never renders a computed merge (always null)', REST,
     '            if let (Some(ok), Some((_, head))) = (got, open) {',
     '            if let (Some(ok), Some((_, head))) = (got.filter(|_| false), open) {', API),
    ('mb3 1: REST renders a computed merge as clean whatever blocks it', REST,
     '                    _ => crate::gql::merge_state(r, &it, got).to_ascii_lowercase(),',
     '                    _ => if ok { "clean".to_owned() } else { "dirty".to_owned() },', API),
    ('mb3 1: REST renders a stale merge (ignores the base tip)', GQL,
     '    let got = graftyard_git::prsync::mergeable(m.as_ref(), head.head_sha, base);',
     '    let got = graftyard_git::prsync::mergeable(m.as_ref(), head.head_sha, m.as_ref().map(|m| m.base));', API),
    ('mb3 4: a restore overwrites the newer counts this object holds', PS,
     '        self.of.entry(net).or_insert_with(|| MergeBudget::restored(vcpu, now, stats));',
     '        self.of.insert(net, MergeBudget::restored(vcpu, now, stats));', GIT),
    ('mb3 4: a restore drops the stored counts', PS, '        b.stats = BudgetStats { vcpu: b.rate / 1000.0, ..stats };',
     '        let _ = stats;', GIT),
    ('mb3 4: a restore keeps the stored vCPU, not the shard\'s now', PS,
     '        b.stats = BudgetStats { vcpu: b.rate / 1000.0, ..stats };', '        b.stats = stats;', GIT),
    ('mb3 3: in_flight_max never counted', PS, '        b.stats.in_flight_max = b.stats.in_flight_max.max(b.in_flight);', '', GIT),
    ('mb3 3: in_flight never left (counts every merge ever)', PS, '                b.in_flight = b.in_flight.saturating_sub(1);',
     '', GIT),
    ('mb3 3: driver drops inFlightMax from the window', A1,
     '    inFlightMax: Math.max(0, ...sites.map((x) => x.inFlightMax)),', '    inFlightMax: 0,', NODE),
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
        # One edit, or a list of (old, new) edits applied together.
        edits = old if isinstance(old, list) else [(old, new)]
        if any(s.count(o) != 1 for o, _ in edits):
            print(f'{i:2} NOT FOUND  {name}')
            continue
        for o, n in edits:
            s = s.replace(o, n)
        open(p, 'w').write(s)
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
