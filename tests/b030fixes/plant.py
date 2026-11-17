#!/usr/bin/env python3
"""b030-fixes: planted bugs in B0-30's three install fixes, the re-placement gate, and the create rule. Plants one
mutant at a time, runs the tests that gate it, and restores the file byte for byte. Each line prints CAUGHT with the
failing tests, MISSED, or DID NOT COMPILE. Run from the worktree root:

    python3 tests/b030fixes/plant.py          # every mutant
    python3 tests/b030fixes/plant.py 0 3      # by number
    python3 tests/b030fixes/plant.py --residue  # every anchor found once in the worktree
"""
import os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
SPLIT = 'crates/git/src/split.rs'
LOAD = 'workers/site/src/load.rs'
FAMILY = 'workers/site/src/family.rs'
SNAP = 'workers/site/src/snap.rs'
MEMBER = 'workers/site/src/member.rs'
LOOKUP = 'crates/lookup/src/lib.rs'
T = ['-p', 'graftyard-git', '--test', 'split']
S = ['-p', 'graftyard-site', '--features', 'test-faults', '--lib']
L = ['-p', 'graftyard-lookup', '--lib']

MUTANTS = [
    # (i-1) a slim snapshot when the pass ends
    ('(i-1) the slimmed tier is due mid-pass', SPLIT,
     '    if !slim_cursor(meta).is_some_and(|c| c.done) {\n        return false;',
     '    if slim_cursor(meta).is_none() {\n        return false;', T),
    ('(i-1) a snapshot at the pass end does not satisfy it', SPLIT,
     '.last().is_none_or(|(to, _, _)| *to < ended)', '.last().is_none_or(|(to, _, _)| *to <= ended)', T),
    ('(i-1) any snapshot after MetaDropped satisfies it', SPLIT,
     '    let Some(ended) = Store::get(meta, "split", &key("slim")).map(|s| s.written) else { return false };',
     '    let Some(ended) = Some(graftyard_types::Index(0)) else { return false };', T),
    # (i-2) an install extends instead of restarting
    ('(i-2) every new source restarts the install, as main did', LOAD,
     '    if keys(&old) != keys(&new) || new.through < old.through {',
     '    if true {', S),
    ('(i-2) a source naming other snapshot keys extends the old job', LOAD,
     '    if keys(&old) != keys(&new) || new.through < old.through {',
     '    if new.through < old.through {', S),
    ('(i-2) an archive gap is extended over', LOAD,
     '    if more.first().is_some_and(|a| a.from != end + 1) || (more.is_empty() && new.through > end) {',
     '    if more.is_empty() && new.through > end {', S),
    ('(i-2) an extended job keeps its old through', LOAD,
     '    old.through = new.through;\n', '', S),
    ('(i-2) an extended job answers the old round', LOAD,
     '    old.kind = new.kind;\n', '', S),
    # (i-3) an installing parent drops its own metadata tables
    ('(i-3) an audit drops the tier it refolds beside', LOAD,
     '    install && applied < through', '    applied < through', S),
    ('(i-3) a member at through drops its tier', LOAD,
     '    install && applied < through', '    install && applied <= through', S),
    # b030r4: the old condition, which waited for the member's own `MetaDropped`. It reads the member's fold in
    # `batch`, which no in-memory test reaches, so this plant is expected MISSED here; the hosted short run checks it.
    ('(i-3) the old condition: an install waits for its own MetaDropped', LOAD,
     '        let drop = drop_due(job.install(), applied, job.through);',
     '        let drop = drop_due(job.install(), applied, job.through)\n            && h.core.borrow().as_ref().is_some_and(|c| crate::split::dropped(&c.m.group_fold().fold));', S),
    ('b030r4: the platform size limit is not counted as SQLITE_FULL', 'workers/site/src/host.rs',
     '"database or disk is full", "Exceeded the maximum database size"]', '"database or disk is full"]', S),
    ('(i-3) a member with its tables dropped loads on restore', FAMILY,
     "    !dropped && if fold.child { crate::split::child_loaded(fold) } else { caught_up }",
     "    if fold.child { crate::split::child_loaded(fold) } else { caught_up }", S),
    ('(i-3) an unloaded tier answers its digest to the audit', SNAP,
     '"at": at.filter(|_| s.loaded()).and_then(|a| digest_at(s, Index(a))).map(|d| d.hex()),',
     '"at": at.and_then(|a| digest_at(s, Index(a))).map(|d| d.hex()),', S),
    ('(i-3) a member that may not lead takes a transfer', MEMBER,
     '        raft::Message::TimeoutNow { term, transfer } if !may_lead => {',
     '        raft::Message::TimeoutNow { term, transfer } if false && !may_lead => {', S),
    ('(i-3) the transfer refusal goes back with the wrong term', MEMBER,
     'msg: raft::Message::TransferRefused { term, transfer } })',
     'msg: raft::Message::TransferRefused { term: raft::Term(term.0 + 1), transfer } })', S),
    # (ii)(a) no re-placement while a voter it needs is behind
    ('(ii)(a) a voter behind is taken as ready', MEMBER,
     '        .filter(|(_, a)| a.is_none_or(|a| commit.saturating_sub(a) > PLACE_LAG_MAX))',
     '        .filter(|(_, a)| a.is_none())', S),
    ('(ii)(a) a voter that did not answer is taken as ready', MEMBER,
     '        .filter(|(_, a)| a.is_none_or(|a| commit.saturating_sub(a) > PLACE_LAG_MAX))',
     '        .filter(|(_, a)| a.is_some_and(|a| commit.saturating_sub(a) > PLACE_LAG_MAX))', S),
    ('(ii)(a) the lag bound is exclusive', MEMBER,
     '        .filter(|(_, a)| a.is_none_or(|a| commit.saturating_sub(a) > PLACE_LAG_MAX))',
     '        .filter(|(_, a)| a.is_none_or(|a| commit.saturating_sub(a) >= PLACE_LAG_MAX))', S),
    # create -> /place only for a group it bootstrapped
    ('create places any group whose members answered its joins, as main did', LOOKUP,
     '    fresh * 2 > answers.len()', '    answers.iter().filter(|a| matches!(a, Some((200, _)))).count() * 2 > answers.len()', L),
    ('create places on one fresh join', LOOKUP, '    fresh * 2 > answers.len()', '    fresh > 0', L),
]

ENV = dict(os.environ, PATH=os.path.expanduser('~/.cargo/bin') + ':' + os.environ['PATH'], CARGO_INCREMENTAL='0')


def run(t):
    r = subprocess.run(['cargo', 'test', '-q'] + t, cwd=ROOT, env=ENV, capture_output=True, text=True, timeout=1800)
    out = r.stdout + r.stderr
    return r.returncode, out, 'error[E' in out or 'could not compile' in out and 'test failed' not in out


def residue():
    bad = []
    for n, (name, path, old, _, _) in enumerate(MUTANTS):
        c = open(os.path.join(ROOT, path)).read().count(old)
        if c != 1:
            bad.append(f'{n:2} {name} ({path}): anchor found {c} times')
    return bad


def main():
    if sys.argv[1:2] == ['--residue']:
        bad = residue()
        print('\n'.join(bad) or f'no plant residue: {len(MUTANTS)} anchors each found once')
        sys.exit(1 if bad else 0)
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    caught = 0
    for i in pick:
        name, f, old, new, t = MUTANTS[i]
        p = os.path.join(ROOT, f)
        orig = open(p, 'rb').read()
        s = orig.decode()
        if s.count(old) != 1:
            print(f'{i:2} NOT FOUND  {name}', flush=True)
            continue
        open(p, 'w').write(s.replace(old, new))
        try:
            code, out, broke = run(t)
        finally:
            open(p, 'wb').write(orig)
        if broke:
            print(f'{i:2} DID NOT COMPILE  {name}', flush=True)
        elif code != 0:
            caught += 1
            fails = [l.split()[1] for l in out.splitlines() if l.startswith('---- ')]
            print(f'{i:2} CAUGHT  {name}  [{", ".join(fails[:3])}]', flush=True)
        else:
            print(f'{i:2} MISSED  {name}', flush=True)
    print(f'{caught} of {len(pick)} caught')


if __name__ == '__main__':
    main()
