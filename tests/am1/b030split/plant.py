#!/usr/bin/env python3
"""b030-split: planted bugs in the metadata child group (`crates/git/src/split.rs`, `workers/site/src/split.rs`,
`crates/git/src/repo.rs`), and the leader-only work rules (`workers/site/src/host.rs`, `paced` and `keep_led`). Plants
one mutant at a time, runs the tests that gate it, and restores the file byte for byte. Each line prints CAUGHT with the failing tests, MISSED, or DID NOT COMPILE. Never run it while a site day builds
from this worktree. Run from the worktree root:

    python3 tests/am1/b030split/plant.py          # every mutant
    python3 tests/am1/b030split/plant.py 0 3      # by number
"""
import os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..', '..'))
SPLIT = 'crates/git/src/split.rs'
REPO = 'crates/git/src/repo.rs'
SITE = 'workers/site/src/split.rs'
FAMILY = 'workers/site/src/family.rs'
HOST = 'workers/site/src/host.rs'
T = ('cargo', ['-p', 'graftyard-git', '--test', 'split'], '')
S = ('cargo', ['-p', 'graftyard-site', '--features', 'test-faults', '--lib'], 'split::')
H = ('cargo', ['-p', 'graftyard-site', '--features', 'test-faults', '--lib'], 'host::')

# (name, file, old, new, tests)
MUTANTS = [
    ('slim ignores when a row was last written: rows edited after the seed lose their text', SPLIT,
     '            if w.written(table, &k).is_some_and(|i| i <= cur.seed) {',
     '            if w.written(table, &k).is_some() {', T),
    ('slim counts the cursor row, which its scan returns first, against the batch, so a table ends a row early',
     SPLIT, '        let want = left + 1 + usize::from(!cur.last.is_empty());', '        let want = left + 1;', T),
    ('slim ends its pass after the first table', SPLIT,
     '            cur.done = cur.table as usize == SLIM_TABLES.len();', '            cur.done = true;', T),
    ('a fill overwrites a field the parent wrote after the seed', SPLIT,
     '        if &**mine == SLIMMED {\n            *mine = theirs.unwrap_or_default();',
     '        if true {\n            *mine = theirs.unwrap_or_default();', T),
    ('a task fill takes the child\'s prompt whether or not it was slimmed', SPLIT,
     '            if p.prompt.as_deref() == Some(SLIMMED) {\n                p.prompt = c.and_then(|c| c.prompt);',
     '            if p.prompt.is_some() {\n                p.prompt = c.and_then(|c| c.prompt);', T),
    ('Filled fills a row the parent deleted', SPLIT,
     '        let cols = self.meta.get(table, key)?;',
     '        let cols = self.meta.get(table, key).or_else(|| (self.child)(table, key).flatten())?;', T),
    ('a missed child row is not counted', SPLIT,
     '                self.missed.borrow_mut().push((table.to_owned(), key.to_vec()));', '', T),
    ('verify lets the parent drop twice', SPLIT,
     "            Some((p, _)) if !child && !st.dropped && p == seed => Ok(()),",
     "            Some((p, _)) if !child && p == seed => Ok(()),", T),
    ('verify takes a seed naming a snapshot no Archived committed', SPLIT,
     '            if !child && !crate::tiers::snapshots(core, Tier::Metadata).iter().any(|(_, _, a)| a.key == *k) {',
     '            if false {', T),
    ('plan drops before the child leader restored its tier', SPLIT,
     '        Some(c) if c.loaded => SplitStep::Drop { seed: at },', '        Some(_) => SplitStep::Drop { seed: at },', T),
    ('plan seeds at the oldest committed metadata snapshot', SPLIT,
     '        return match crate::tiers::snapshots(core, Tier::Metadata).pop() {',
     '        return match crate::tiers::snapshots(core, Tier::Metadata).first().cloned() {', T),
    ('a child member applies its own log to its restored tier', REPO,
     '        if self.meta.loaded() && !self.child {', '        if self.meta.loaded() {', T),
    ('a child serves rows before its restore finished', SITE,
     '    if !fold.child || !child_loaded(fold) {\n        return json!({ "error": "not a loaded metadata child" });',
     '    if !fold.child {\n        return json!({ "error": "not a loaded metadata child" });', S),
    ('a parent dump that missed child rows is cached and served without them', FAMILY,
     '    if let Some(miss) = crate::split::since(m0) {\n        return Some(serde_json::to_vec(&miss).unwrap_or_default().into());\n    }\n',
     '    let _ = crate::split::since(m0);\n', S),
    ('a parent answer does not fill slimmed rows from the child', SITE,
     '            Some(c) => *cols = git::split::fill(t, cols, c.as_deref()),', '            Some(_) => {}', S),
    # The leader-only work rules (`decisions/DECISIONS.md`, "A leader paces its own background writes").
    ('keep_led counts the bound from its first check, not from when its promise lapsed', HOST,
     '    let base = if lapsed > 0 { lapsed } else { found }.max(acted);', '    let base = found.max(acted);', H),
    ('keep_led acts again on every check after it first acted', HOST,
     '    let base = if lapsed > 0 { lapsed } else { found }.max(acted);',
     '    let base = if lapsed > 0 { lapsed } else { found };', H),
    ('keep_led campaigns at once, with no bound', HOST,
     '    if now < base + LEADERLESS_FOR.as_millis() as u64 {', '    if false {', H),
    ('keep_led acts on a member that still holds a promise to its leader', HOST,
     '    if heard || !needs {\n        return KeepLed::Off;', '    if !needs {\n        return KeepLed::Off;', H),
    ('keep_led campaigns past a leader it still names instead of probing it', HOST,
     '    } else if named {\n        KeepLed::Probe', '    } else if false {\n        KeepLed::Probe', H),
    ('keep_led acts with no leader-only work due', HOST,
     '    if heard || !needs {\n        return KeepLed::Off;', '    if heard {\n        return KeepLed::Off;', H),
    ('a paced write waits only its own commit time, not twice it', HOST,
     'pub const PACE: u32 = 2;', 'pub const PACE: u32 = 1;', H),
    ('paced writes are not capped', HOST,
     '    (Duration::from_millis(took_ms) * PACE).min(PACE_MAX)', '    Duration::from_millis(took_ms) * PACE', H),
    ('a split needs its leader on the child too', SITE,
     '    !fold.child && !matches!(git::split::plan', '    !matches!(git::split::plan', S),
    ('a split needs its leader once the slim pass is done', SITE,
     '    !fold.child && !matches!(git::split::plan(&*fold.core, &fold.meta, bytes, split_at, None), SplitStep::Idle)',
     '    !fold.child && git::split::slim_cursor(&fold.meta).is_none_or(|_| true)', S),
    # A `MetaSlim` entry's byte bound (`SLIM_BYTES`).
    ('a slim entry ignores its byte bound within a scan batch', SPLIT,
     '            if bytes >= SLIM_BYTES {\n                end = false;', '            if false {\n                end = false;', T),
    ('a slim entry cut by its bytes moves on to the next table, skipping the rest of this one', SPLIT,
     '                end = false;\n                break;', '                break;', T),
]


def run(tests):
    kind, args, filt = tests
    env = dict(os.environ, PATH=os.path.expanduser('~/.cargo/bin:') + os.environ['PATH'])
    r = subprocess.run(['cargo', 'test', *args, '--no-fail-fast', '--', filt], cwd=ROOT, env=env,
                       capture_output=True, text=True)
    out = r.stdout + r.stderr
    failed = sorted({l.split()[1] for l in out.splitlines() if l.startswith('test ') and l.endswith('FAILED')})
    return r, out, failed


def plant(n, name, path, old, new, tests):
    f = os.path.join(ROOT, path)
    orig = open(f, 'rb').read()
    src = orig.decode()
    assert src.count(old) == 1, (name, 'anchor not unique', src.count(old))
    open(f, 'w').write(src.replace(old, new, 1))
    try:
        r, out, failed = run(tests)
        compile_err = 'error[' in out or '\nerror: could not compile' in out
        if compile_err and not failed:
            verdict = 'DID NOT COMPILE'
        else:
            verdict = 'CAUGHT' if r.returncode != 0 and failed else 'MISSED'
        print(f'{n:2} {verdict:8} {name}: {", ".join(failed)}', flush=True)
    finally:
        open(f, 'wb').write(orig)
    assert open(f, 'rb').read() == orig


if __name__ == '__main__':
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    for n in pick:
        plant(n, *MUTANTS[n])
