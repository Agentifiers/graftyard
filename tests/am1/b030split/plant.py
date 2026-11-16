#!/usr/bin/env python3
"""b030-split: planted bugs in the metadata child group (`crates/git/src/split.rs`, `workers/site/src/split.rs`,
`crates/git/src/repo.rs`), the leader-only work rules (`workers/site/src/host.rs`, `paced` and `keep_led`), and the archive
drain's throughput and its hysteresis band (`crates/git/src/log_archive.rs`, 27 to 41). Plants
one mutant at a time, runs the tests that gate it, and restores the file byte for byte. Each line prints CAUGHT with the failing tests, MISSED, or DID NOT COMPILE. Never run it while a site day builds
from this worktree. Run from the worktree root:

    python3 tests/am1/b030split/plant.py          # every mutant
    python3 tests/am1/b030split/plant.py 0 3      # by number
    python3 tests/am1/b030split/plant.py --residue  # before every commit (the pre-commit hook): no run in flight, every
                                                    # anchor once in the worktree and the index
"""
import os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..', '..'))
SPLIT = 'crates/git/src/split.rs'
REPO = 'crates/git/src/repo.rs'
SITE = 'workers/site/src/split.rs'
FAMILY = 'workers/site/src/family.rs'
HOST = 'workers/site/src/host.rs'
ARCH = 'crates/git/src/log_archive.rs'
SHARD = 'crates/git/src/shard.rs'
FAMILYS = FAMILY
T = ('cargo', ['-p', 'graftyard-git', '--test', 'split'], '')
S = ('cargo', ['-p', 'graftyard-site', '--features', 'test-faults', '--lib'], 'split::')
H = ('cargo', ['-p', 'graftyard-site', '--features', 'test-faults', '--lib'], 'host::')
A = ('cargo', ['-p', 'graftyard-git', '--test', 'log_archive'], '')
AS = ('cargo', ['-p', 'graftyard-site', '--features', 'test-faults', '--lib'], 'archive::')
G = ('cargo', ['-p', 'graftyard-git', '--test', 'shard'], 'warming')

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
    # Drain throughput (rerun stop at simulated day 1.82).
    ('the rerun\'s promotion size: 4 archive ranges a promotion', ARCH,
     'pub const PROMOTE_RANGES: u64 = 8;', 'pub const PROMOTE_RANGES: u64 = 4;', A),
    ('one archive object an entry, as in the rerun', ARCH,
     'pub const ARCHIVE_PIECES: usize = 8;', 'pub const ARCHIVE_PIECES: usize = 1;', A),
    # Was 'archiving goes before promoting ahead': under the band only a draining run or LOG_KEEP makes a range due, and
    # a draining run archives first anyway, so that mutant became equivalent (MISSED, third drain fix).
    ('promoting ahead before both tiers have a snapshot', ARCH,
     '    let ahead = v.snapshotted && v.unpromoted > max_bytes / PROMOTE_AHEAD;',
     '    let ahead = v.unpromoted > max_bytes / PROMOTE_AHEAD;', A),
    ('promoting ahead goes first however far into its run the log is', ARCH,
     '    let ahead_first = ahead && !run.draining;',
     '    let ahead_first = ahead;', A),
    ('an archive step reads the next range from the start of the last, so pieces overlap', ARCH,
     '        let more = due(total, h.rows(to)?, v.now, v.snapshotted, v.commit, target, range_max)?;',
     '        let more = due(total, h.rows(Index(from.0 - 1))?, v.now, v.snapshotted, v.commit, target, range_max)?;', A),
    ('an archive step\'s later pieces are cut against the trigger, not the run\'s target (the resume at day 1.82)', ARCH,
     '        let more = due(total, h.rows(to)?, v.now, v.snapshotted, v.commit, target, range_max)?;',
     '        let more = due(total, h.rows(to)?, v.now, v.snapshotted, v.commit, target + target / 14, range_max)?;', A),
    ('an archive step archives past the last promotion', ARCH,
     '        let Next::Archive { to, .. } = next(v.promoted.as_ref(), from, to) else { break };',
     '        let Next::Archive { .. } = next(v.promoted.as_ref(), from, to) else { break };', A),
    ('a promotion never pauses its read', ARCH,
     '            if taken >= paused + PAUSE_EVERY {', '            if false && taken >= paused + PAUSE_EVERY {', A),
    ('a promotion\'s read after a pause starts over from the last promotion', ARCH,
     '        for (i, row) in h.rows(p.to)? {', '        for (i, row) in h.rows(after)? {', A),
    ('members truncate through the first of an entry\'s archives only', FAMILYS,
     '            .collect()\n    }\n    async fn truncated', '            .take(1)\n            .collect()\n    }\n    async fn truncated', AS),
    ('a promotion waits the full minute on a shard with another after it', SHARD,
     '    if next { PROMOTE_NEXT_WAIT_S } else { PROMOTE_WAIT_S }', '    PROMOTE_WAIT_S', G),
    # Hysteresis (the second resume's stalls, simulated day 1.905 to 2.005).
    ('e9418153\'s rule: a run starts only over the cap and its target is set per step', ARCH,
     '        self.draining |= total > trigger(max_bytes);\n        if self.draining { floor(max_bytes) } else { trigger(max_bytes) }',
     '        self.draining = total > max_bytes;\n        if self.draining { floor(max_bytes) } else { max_bytes }', A),
    ('the trigger without the run\'s hold: the floor is the target only while the log is over the trigger', ARCH,
     '        self.draining |= total > trigger(max_bytes);', '        self.draining = total > trigger(max_bytes);', A),
    ('promoting ahead goes first until the log is over its cap, not until the run passes its trigger', ARCH,
     '    let ahead_first = ahead && !run.draining;', '    let ahead_first = ahead && v.total <= max_bytes;', A),
    ('a run ends after any archive step, wherever the log is', ARCH,
     '        reached = to == due_to && more.is_none();', '        reached = true;', A),
]


def run(tests):
    kind, args, filt = tests
    env = dict(os.environ, PATH=os.path.expanduser('~/.cargo/bin:') + os.environ['PATH'])
    # A mutant that loops a test forever is caught by the time limit (mutant 32 of the third drain fix did).
    try:
        r = subprocess.run(['cargo', 'test', *args, '--no-fail-fast', '--', filt], cwd=ROOT, env=env,
                           capture_output=True, text=True, timeout=1200)
    except subprocess.TimeoutExpired:
        return subprocess.CompletedProcess([], 1), '', ['(timed out after 1200 s)']
    out = r.stdout + r.stderr
    failed = sorted({l.split()[1] for l in out.splitlines() if l.startswith('test ') and l.endswith('FAILED')})
    return r, out, failed


# Held while a run has a file mutated. 3117f62 was committed while this script had mutant 18 planted in host.rs (the
# full checks ran the plants in a service at the same time), and that site day shipped the mutant. `--residue`, which
# the branch's pre-commit hook runs, fails while the lock is held.
LOCK = os.path.join(ROOT, '.git', 'b030split-plant.lock')


def tracked_diff():
    """The worktree's diff of tracked files against HEAD, which a plant run must leave as it found it."""
    return subprocess.run(['git', 'diff', 'HEAD'], cwd=ROOT, capture_output=True, text=True).stdout


def plant(n, name, path, old, new, tests):
    f = os.path.join(ROOT, path)
    orig = open(f, 'rb').read()
    src = orig.decode()
    assert src.count(old) == 1, (name, 'anchor not unique', src.count(old))
    try:
        open(f, 'w').write(src.replace(old, new, 1))
        r, out, failed = run(tests)
        compile_err = 'error[' in out or '\nerror: could not compile' in out
        if compile_err and not failed:
            verdict = 'DID NOT COMPILE'
        else:
            verdict = 'CAUGHT' if r.returncode != 0 and failed else 'MISSED'
        print(f'{n:2} {verdict:8} {name}: {", ".join(failed)}', flush=True)
    finally:
        open(f, 'wb').write(orig)
    assert open(f, 'rb').read() == orig, (name, 'not restored')


def residue(rev='HEAD'):
    """Mutants whose text is in `rev`'s file where their anchor is not: a planted mutation that reached a commit
    (3117f62 carried mutant 18 into a site day). Run before every commit on this branch:
    `python3 tests/am1/b030split/plant.py --residue`."""
    found = []
    for n, (name, path, old, new, _) in enumerate(MUTANTS):
        if rev == 'WORKTREE':
            src = open(os.path.join(ROOT, path)).read()
        else:
            spec = f':{path}' if rev == 'INDEX' else f'{rev}:{path}'
            src = subprocess.run(['git', 'show', spec], cwd=ROOT, capture_output=True, text=True).stdout
        if src.count(old) != 1:
            found.append(f'{n:2} {name} ({path}): anchor found {src.count(old)} times')
    return found


if __name__ == '__main__':
    if sys.argv[1:2] == ['--residue']:
        bad = residue('WORKTREE') + residue('INDEX')
        if os.path.exists(LOCK):
            bad.insert(0, f'a plant run holds {LOCK}: a file may be mutated right now')
        print('\n'.join(bad) or f'no plant residue: {len(MUTANTS)} anchors each found once in the worktree and index')
        sys.exit(1 if bad else 0)
    before = tracked_diff()
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    open(LOCK, 'w').write(str(os.getpid()))
    try:
        for n in pick:
            plant(n, *MUTANTS[n])
    finally:
        os.remove(LOCK)
        after = tracked_diff()
        if after != before:
            print('PLANT RESIDUE: the tracked diff changed during this run', file=sys.stderr)
            sys.exit(2)
