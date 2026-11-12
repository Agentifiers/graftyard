#!/usr/bin/env python3
"""hot-shards: planted bugs in the git cache shard's catch-up and rehydrate (`crates/git/src/catchup.rs`), the
whole-tier read's byte budget (`crates/api/src/hosted.rs`), and the shard container (`containers/git/server`). Plants one
mutant at a time, runs the tests that gate it, and restores the file byte for byte. Each line prints CAUGHT with the
failing tests, MISSED, or DID NOT COMPILE. Never run it while a site day builds from this worktree. Run from the
worktree root:

    python3 tests/am1/hotshards/plant.py          # every mutant
    python3 tests/am1/hotshards/plant.py 0 3      # by number
"""
import os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..', '..'))
CU = 'crates/git/src/catchup.rs'
HOSTED = 'crates/api/src/hosted.rs'
GO = 'containers/git/server/main.go'
CATCHUP = ('cargo', ['-p', 'graftyard-git', '--test', 'catchup'], '')
SURFACES = ('cargo', ['-p', 'graftyard-api', '--test', 'surfaces'], 'tier')
GOTEST = ('go', [], 'Derived|Have|409|Ingest')
PRSYNC = 'crates/git/src/prsync.rs'
PRSYNC_T = ('cargo', ['-p', 'graftyard-git', '--test', 'prsync'], 'a_leader_far')
PACK = 'crates/git/src/pack.rs'
PACK_T = ('cargo', ['-p', 'graftyard-git', '--lib'], 'base64')
EDGE = 'workers/edge/src/lib.rs'
EDGE_T = ('cargo', ['-p', 'graftyard-edge', '--lib'], 'validate_falls_back')
EDGE_C = ('cargo', ['-p', 'graftyard-edge', '--lib'], 'cold_shard_is_tried_last')
SHARD = 'crates/git/src/shard.rs'
SHARD_T = ('cargo', ['-p', 'graftyard-git', '--test', 'shard'], 'a_promotion')
SHARD_W = ('cargo', ['-p', 'graftyard-git', '--test', 'shard'], 'a_shard_answers_warming')
EDGE_N = ('cargo', ['-p', 'graftyard-edge', '--lib'], 'every_shard_try')

# (name, file, old, new, tests)
MUTANTS = [
    ('the cursor moves to the end of the range asked for, not the last entry received (blocker 12)', CU,
     '        Some(l) if ok && l.index <= to => Ok(l.index),', '        Some(l) if ok && l.index <= to => Ok(to),',
     CATCHUP),
    ('an empty page ends the catch-up as if caught up', CU,
     '        let last = page_end(chain, from, to, &page)?;',
     '        if page.is_empty() {\n            break;\n        }\n        let last = page_end(chain, from, to, &page)?;',
     CATCHUP),
    ('no page is read ahead while one applies', CU, '        if next <= to {\n', '        if false {\n', CATCHUP),
    ('a resumed rehydrate starts a new repository (blocker 11)', CU,
     '    let resumed = saved.rehydrating().await?.filter(|(d, _)| *d == disk).map(|(_, s)| s);',
     '    let resumed: Option<Rehydrating> = None;', CATCHUP),
    ('a resumed rehydrate reads its tail from the last Promoted again', CU,
     '        let from = Index(tail_start(last.as_ref()).0.max(state.tail.at(*chain).0 + 1));',
     '        let from = tail_start(last.as_ref());', CATCHUP),
    ('a resumed rehydrate ingests the marked packs again', CU, '    if !state.marked {', '    if true {', CATCHUP),
    ('packs the container already holds are sent again', CU,
     '    let held = if loads.len() > 1 { io.have(&sums).await? } else { BTreeSet::new() };',
     '    let held: BTreeSet<Digest> = BTreeSet::new();\n    let _ = &sums;', CATCHUP),
    ('a rehydrate reads the whole log, not only above the last Promoted (blocker 9)', CU,
     '        let from = Index(tail_start(last.as_ref()).0.max(state.tail.at(*chain).0 + 1));',
     '        let from = Index(1u64.max(state.tail.at(*chain).0 + 1));', CATCHUP),
    ('an unresolved pack keeps the cursor, so every call fails on it', CU,
     '            Err(Stop::Unresolved(e)) => {\n                saved.drop_cursor().await?;\n',
     '            Err(Stop::Unresolved(e)) => {\n', CATCHUP),
    ('the catch-up cursor is saved only at the end of the range', CU,
     '            cursor.advance(chain, last);\n            saved.put_cursor(disk, cursor).await?;\n',
     '            cursor.advance(chain, last);\n            if last == Index(u64::MAX) {\n'
     '                saved.put_cursor(disk, cursor).await?;\n            }\n', CATCHUP),
    ('a rehydrate loads only the last promotion first, not the earlier ones still in the log', CU,
     '                if e.promoted.as_ref().and_then(|p| p.pack.stored_id()) == Some(*id) {',
     '                if false && e.promoted.as_ref().and_then(|p| p.pack.stored_id()) == Some(*id) {', CATCHUP),
    ('the promotions load in marker order, not oldest first', CU,
     '            (!id.is_some_and(|id| promoted.contains(&id)), id.and_then(|id| at.get(&id).copied()))',
     '            (!id.is_some_and(|id| promoted.contains(&id)), id.and_then(|id| at.get(&id).map(|i| u64::MAX - i.0)))',
     CATCHUP),
    ('a leader scan reads every stored row in one alarm', PRSYNC,
     '        if bytes >= SCAN_BYTES {\n            return Ok(true);\n        }\n', '', PRSYNC_T),
    ('a scan cut at its byte budget says nothing is left', PRSYNC,
     '        if bytes >= SCAN_BYTES {\n            return Ok(true);\n', '        if bytes >= SCAN_BYTES {\n            return Ok(false);\n',
     PRSYNC_T),
    ('base64 decode accepts a byte outside the alphabet', PACK,
     'Some(VALUE[*b as usize]).filter(|v| *v != 0xff)?', 'Some(VALUE[*b as usize] & 63)?', PACK_T),
    ('a whole-tier read counts rows only (blocker 5)', HOSTED,
     '                    if budget.rows == 0 || budget.bytes < bytes {',
     '                    if budget.rows == 0 {', SURFACES),
    ('an ingest missing a named object answers 422, not 409', GO,
     ' ||\n\t\tstrings.Contains(s, "did not receive expected object")', '', GOTEST),
    ('derived git work counts in /load (A31)', GO, '\t\tmarked = marked || e == derivedEnv',
     '\t\tmarked = marked && e == derivedEnv', GOTEST),
    ('/have lists packs never ingested', GO,
     '\t\tif _, err := os.Stat(filepath.Join(rp, "graft", sum+".objs")); err == nil {',
     '\t\tif _, err := os.Stat(filepath.Join(rp, "graft")); err == nil {',
     GOTEST),
    ('a push validate falls back to a Witness site, which has no git cache', EDGE,
     "**s != record.home && **k == graftyard_types::MemberKind::Full", "**s != record.home", EDGE_T),
    ('a push validate tries the sites in record order, not the home site first', EDGE,
     '    std::iter::once(record.home).chain(others).collect()',
     '    record.sites.iter().copied().filter(|s| others.clone().any(|o| o == *s) || *s == record.home).collect()',
     EDGE_T),
    ('a cold shard is skipped altogether rather than tried last', EDGE,
     '    warm.into_iter().chain(cold).collect()', '    if warm.is_empty() { cold } else { warm }', EDGE_C),
    ('the warm shards are reordered by site id, losing the home-first order', EDGE,
     "    let (warm, cold): (Vec<_>, Vec<_>) = sites.into_iter().partition(|s| !cold(*s));",
     "    let (mut warm, cold): (Vec<_>, Vec<_>) = sites.into_iter().partition(|s| !cold(*s));\n    warm.sort();",
     EDGE_C),
    ('a promotion falls back to a Witness site, which has no git cache', SHARD,
     '        .filter(|(_, k)| **k == graftyard_types::MemberKind::Full)\n', '', SHARD_T),
    ('a promotion tries the home site first, not the leader\'s own', SHARD,
     '    std::iter::once(me).chain(full.filter(|s| *s != me)).collect()',
     '    let _ = me;\n    std::iter::once(record.home).chain(full.filter(|s| *s != record.home)).collect()', SHARD_T),
    ('a promotion moves on after the packs\' own 422', SHARD, '    status != 422', '    status != 0', SHARD_T),
    ('a promotion gives up on its own shard\'s 503 warming', SHARD, '    status != 422',
     '    status != 422 && status != 503', SHARD_T),
    ('a rehydrating shard answers warming at once even to a caller\'s last try', SHARD,
     '    pool_cold || (follows && rehydrating)', '    pool_cold || rehydrating', SHARD_W),
    ('a shard answers warming at once to any caller with somewhere else to go, rehydrating or not', SHARD,
     '    pool_cold || (follows && rehydrating)', '    pool_cold || follows', SHARD_W),
    ('a cold pool shard waits on a forwarded push', SHARD,
     '    pool_cold || (follows && rehydrating)', '    follows && rehydrating', SHARD_W),
    ('the edge\'s last shard try also says another follows', EDGE,
     '    let next = if i + 1 < n { "&next=1" } else { "" };', '    let next = "&next=1";', EDGE_N),
    ('the edge\'s home try waits as long as the others', EDGE,
     '    let wait = if i == 0 { VALIDATE_WAIT_HOME_S } else { VALIDATE_WAIT_OTHER_S };\n    let next',
     '    let wait = VALIDATE_WAIT_OTHER_S;\n    let next', EDGE_N),
]


def run(tests):
    kind, args, filt = tests
    if kind == 'go':
        r = subprocess.run(['go', 'test', '-count=1', '-run', filt, './...'], cwd=os.path.join(ROOT, 'containers/git/server'),
                           capture_output=True, text=True)
        out = r.stdout + r.stderr
        failed = sorted({l.split()[2] for l in out.splitlines() if l.startswith('--- FAIL')})
        return r, out, failed
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
        compile_err = 'error[' in out or '\nerror: could not compile' in out or 'build failed' in out
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
