#!/usr/bin/env python3
"""b030-fixes: planted bugs in B0-30's three install fixes (with G21's pin in place of (i-2)), G22's try order, the re-placement gate, the create rule, and GET /log's byte cap. Plants one
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
HOST = 'workers/site/src/host.rs'
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
    # G21 (replaces (i-2)): a running install keeps its snapshots for its life
    ('G21 main\'s rule: a source naming other snapshots restarts the install', LOAD,
     '    if new.through < old.through {\n        // The same leader',
     '    if [old.tiers[0].key.clone(), old.tiers[1].key.clone()] != [new.tiers[0].key.clone(), new.tiers[1].key.clone()] {\n        return Next::Restart(new, "other snapshots");\n    }\n    if new.through < old.through {\n        // The same leader', S),
    ('G21 main\'s rule: every earlier through restarts, a late delivery included', LOAD,
     '        return if round(&old) == round(&new) {', '        return if false && round(&old) == round(&new) {', S),
    ('G21 another leader\'s earlier through is ignored too', LOAD,
     '        return if round(&old) == round(&new) {', '        return if true {', S),
    ('G21 the round ignores the term', LOAD,
     '            Kind::Install { from, install } => Some((*from, install.term, install.boot)),',
     '            Kind::Install { from, install } => Some((*from, install.boot)),', S),
    ('G21 an extended job takes the newer snapshot keys', LOAD,
     '    old.archives.extend(more);\n', '    old.archives.extend(more);\n    old.tiers[0].key = new.tiers[0].key.clone();\n', S),
    ('G21 an archive gap is extended over', LOAD,
     '    if more.first().is_some_and(|a| a.from != end + 1) || (more.is_empty() && new.through > end) {',
     '    if more.is_empty() && new.through > end {', S),
    ('G21 an extended job keeps its old through', LOAD,
     '    old.through = new.through;\n', '', S),
    ('G21 an extended job answers the old round', LOAD,
     '    old.kind = new.kind;\n', '', S),
    ('G21 main\'s rule: a failed install batch abandons the job', LOAD,
     '    install && tries < BATCH_TRIES', '    false && install && tries < BATCH_TRIES', S),
    ('G21 an install retries without end', LOAD,
     '    install && tries < BATCH_TRIES', '    install', S),
    ('G21 an audit is retried', LOAD,
     '    install && tries < BATCH_TRIES', '    tries < BATCH_TRIES', S),
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
    ('b030r4: the platform size limit is not counted as SQLITE_FULL', HOST,
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
    # G22: a re-placement runs its tries before it degrades the group
    ('G22 main\'s order: Degraded before the first try', MEMBER,
     '    let t0 = s.now();\n    let to = old.site;',
     '    let t0 = s.now();\n    let to = old.site;\n    let _ = s.change(degraded(Ready(()), p.clone(), to)).await;', S),
    ('G22 no second ready before Degraded', MEMBER,
     '    let ready = match s.ready().await? {\n        Ok(r) => r,',
     '    let ready = match Ok::<Ready, Value>(Ready(())) {\n        Ok(r) => r,', S),
    ('G22 a voter behind at the commit still commits', MEMBER,
     '            s.clear(&kept).await;\n            s.restore_row(&old, &joined).await;\n            return behind(b, &tried);',
     '            let _ = b;\n            Ready(())', S),
    ('G22 a voter behind at the commit leaves the chosen try', MEMBER,
     '            s.clear(&kept).await;\n            s.restore_row(&old, &joined).await;\n            return behind(b, &tried);',
     '            s.restore_row(&old, &joined).await;\n            return behind(b, &tried);', S),
    ('G22 no nearer try still goes back to Normal, as main did', MEMBER,
     '        s.restore_row(&old, &joined).await;\n        if keep_below.is_some() {',
     '        let _ = s.change(graftyard_raft::GroupConfig::Normal(p.clone())).await;\n        s.restore_row(&old, &joined).await;\n        if keep_below.is_some() {', S),
    ('G22 the first ready is skipped', MEMBER,
     '    if let Err(b) = s.ready().await? {\n        return behind(b, &[]);\n    }',
     '', S),
    ('G22 an over-the-bound try is cleared at once', MEMBER,
     '            Some((np, ms)) => over.push((new.clone(), np, ms, t.fresh)),',
     '            Some((np, ms)) => {\n                s.clear(&new).await;\n                over.push((new.clone(), np, ms, t.fresh))\n            }', S),
    ('G22 the registry row goes back to the old member after a placement', MEMBER,
     '    s.restore_row(&kept, &joined).await;\n    let mut v', '    s.restore_row(&old, &joined).await;\n    let mut v', S),
    # create -> /place only for a group it bootstrapped
    ('create places any group whose members answered its joins, as main did', LOOKUP,
     '    fresh * 2 > answers.len()', '    answers.iter().filter(|a| matches!(a, Some((200, _)))).count() * 2 > answers.len()', L),
    ('create places on one fresh join', LOOKUP, '    fresh * 2 > answers.len()', '    fresh > 0', L),
    # b030r6: GET /log bounded by bytes (a harness read of 100-400 entries reset the leader's isolate)
    ('/log unbounded by bytes, as main', HOST, '        if !out.is_empty() && bytes.saturating_add(len) > cap {',
     '        if false && !out.is_empty() && bytes.saturating_add(len) > cap {', S),
    ('/log keeps the row that crosses the cap', HOST, '        if !out.is_empty() && bytes.saturating_add(len) > cap {\n            break;\n        }\n        bytes = bytes.saturating_add(len);\n        out.push(row);',
     '        bytes = bytes.saturating_add(len);\n        out.push(row);\n        if bytes > cap {\n            break;\n        }', S),
    ('/log refuses a first entry over the cap', HOST, '        if !out.is_empty() && bytes.saturating_add(len) > cap {',
     '        if bytes.saturating_add(len) > cap {', S),
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
