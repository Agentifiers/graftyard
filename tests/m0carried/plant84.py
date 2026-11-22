#!/usr/bin/env python3
"""M5-WP5's hosted half, code only: planted bugs in the change feed's consumer rules and the export routes' reads
(`crates/git/src/export.rs`, `crates/lookup/src/lib.rs`, `crates/identity/src/export_token.rs`, and the registry
object's feed SQL in `workers/site/src/export_feed.rs`; `decisions/DECISIONS.md`, `export-host`, with the sweep of chains
no leader reports). Plants one mutant at a time, runs the tests that guard it, and restores the file byte for byte. Each line
prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant84.py          # every mutant
    python3 tests/m0carried/plant84.py 0 3      # by number
"""
import os
import signal
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
EXP = 'crates/git/src/export.rs'
LOOK = 'crates/lookup/src/lib.rs'
TOK = 'crates/identity/src/export_token.rs'
FEED = 'workers/site/src/export_feed.rs'
GIT = ['sh', '-c', 'cargo test -q -p graftyard-git --test export && cargo test -q -p graftyard-git --lib -- export::']
LOOKUP = ['cargo', 'test', '-q', '-p', 'graftyard-lookup', '--lib', '--', 'export_sources']
CLI = ['cargo', 'test', '-q', '-p', 'graftyard', '--', 'vault']
SITE = ['cargo', 'test', '-q', '-p', 'graftyard-site', '--lib', '--', 'export_feed']

MUTANTS = [
    # The consumer's rules.
    ("fill takes a source that skipped past a carried cursor", EXP,
     'if f.first.0 < from || (from > 1 && f.first.0 != from) || f.first > *to {',
     'if f.first.0 < from || f.first > *to {', GIT),
    ("fill takes rows from before the cursor", EXP,
     'if f.first.0 < from || (from > 1 && f.first.0 != from) || f.first > *to {',
     'if (from > 1 && f.first.0 > from) || f.first > *to {', GIT),
    ("fill carries rows past the batch's index", EXP,
     'for r in f.rows.iter().take((to.0 + 1 - f.first.0) as usize) {', 'for r in f.rows.iter() {', GIT),
    ("fill ignores the object byte cap", EXP,
     'if bytes > 0 && bytes + r.len() > OBJECT_BYTES_MAX {', 'if false {', GIT),
    ("fill keeps a chain whose source gave nothing", EXP, '''        if rows.is_empty() {
            continue;
        }
        chains.push''', '''        chains.push''', GIT),
    ("a first carry must start at 1", EXP,
     'let starts = if from == 1 { first.0 >= 1 && first.0 <= to.0 } else { first.0 == from };',
     'let starts = first.0 == from;', GIT),
    ("a leader reports at every alarm", EXP,
     'Some((i, at)) => commit > i && now_ms.saturating_sub(at) >= FEED_EVERY_MS,',
     'Some((i, _)) => commit > i,', GIT),
    ("a leader reports a commit it already reported", EXP,
     'Some((i, at)) => commit > i && now_ms.saturating_sub(at) >= FEED_EVERY_MS,',
     'Some((_, at)) => now_ms.saturating_sub(at) >= FEED_EVERY_MS,', GIT),
    ("a deposed leader's lower report moves the feed back", EXP,
     'held.is_none_or(|h| reported > h)', 'held.is_none_or(|h| reported != h)', GIT),
    ("a kick and a queued round both run", EXP,
     'last_ms.is_none_or(|l| now_ms.saturating_sub(l) >= FEED_EVERY_MS - 1_000)', 'true', GIT),
    # The sweep (`export-sweep`): chains no leader reports.
    ("a round plans from the leaders' reports only", EXP,
     'for (c, i) in reports.iter().chain(probes) {', 'for (c, i) in reports.iter().chain(probes.iter().take(0)) {', GIT),
    ("a probe lowers a leader's higher report", EXP, '*e = (*e).max(*i);', '*e = *i;', GIT),
    ("Ids is not probed every round", EXP,
     'pub const ROUND_PROBES: [ChainId; 2] = [ChainId::Release, ChainId::Ids];',
     'pub const ROUND_PROBES: [ChainId; 2] = [ChainId::Release, ChainId::Release];', GIT),
    ("the sweep reads only the first site's shards", EXP,
     'let total = per * sites as u64;', 'let total = per * (sites as u64).min(1);', GIT),
    ("the sweep probes a chain on every site that holds it", EXP,
     "rows.iter().filter(|r| r.sites.first() == Some(&site) && !ROUND_PROBES.contains(&r.chain)).collect()",
     "rows.iter().filter(|r| !ROUND_PROBES.contains(&r.chain)).collect()", GIT),
    ("the sweep's counter reads another column", FEED,
     '"SELECT n FROM export_sweep WHERE k = 0"', '"SELECT k FROM export_sweep WHERE k = 0"', SITE),
    # The member's `/log_rows` page.
    ("a leader with followers serves /log_rows", EXP,
     '!leading || (voters <= 1 && learners == 0)', '!leading || voters <= 3', GIT),
    ("a log page serves past the commit index", EXP, 'let end = to.min(commit);', 'let end = to;', GIT),
    ("a log page crosses a gap", EXP,
     "if i > end || first.is_some_and(|f: Index| i.0 != f.0 + rows.len() as u64) {", 'if i > end {', GIT),
    ("a log page ignores its byte cap", EXP,
     'if !rows.is_empty() && bytes + row.len() > page_bytes {', 'if false {', GIT),
    ("a page's archive range is lost on the wire", EXP,
     '#[serde(default, skip_serializing_if = "Option::is_none")]\n    pub archive', '#[serde(skip)]\n    pub archive', GIT),
    # The routes.
    ("an org token reads the feed", EXP,
     "(Route::Batches { .. }, _) if !covers(ChainId::Release) => Err(ExportError::NotFound),", '', GIT),
    ("a batches page ignores its byte cap", EXP,
     'held < BATCHES_PAGE && (held == 0 || bytes < BATCHES_PAGE_BYTES)', 'held < BATCHES_PAGE', GIT),
    ("an archive read starts at the object's first row", EXP,
     '            if self.at >= self.from {\n                self.bytes', '            if true {\n                self.bytes', GIT),
    ("an archive read runs past `to`", EXP,
     'self.at > self.to || self.rows.len() as u64 >= self.limit', 'self.rows.len() as u64 >= self.limit', GIT),
    ("an archive read skips the header check", EXP,
     'graftyard_fold::check_archive_header(&self.buf).map_err(|e| e.to_string())?;', '', GIT),
    ("an entries page cut short by its member does not link on", EXP,
     'Ok((out, (n > 0 && want <= to).then_some(want)))', 'Ok((out, None))', GIT),
    ("a source read goes to the leader", LOOK,
     'learners.iter().chain(voters).filter(|m| Some(*m) != leader).cloned().collect();',
     'learners.iter().chain(voters).cloned().collect();', LOOKUP),
    ("a source read asks voters before learners", LOOK,
     'learners.iter().chain(voters).filter(|m| Some(*m) != leader).cloned().collect();',
     'voters.iter().chain(learners).filter(|m| Some(*m) != leader).cloned().collect();', LOOKUP),
    ("a Solo group has no source", LOOK, 'if others.is_empty() { voters.to_vec() } else { others }', 'others', LOOKUP),
    ("a token's scope text does not round-trip", TOK,
     'Scope::Owner(a) => format!("owner:{}", a.0.0),', 'Scope::Owner(a) => format!("org:{}", a.0.0),', CLI),
    # The registry object's feed SQL (m3fx49: every report and claim failed on `commit` as a column).
    ("the reports table names a reserved word as a column", FEED,
     '(chain TEXT PRIMARY KEY, idx INTEGER NOT NULL', '(chain TEXT PRIMARY KEY, commit INTEGER NOT NULL', SITE),
    ("a report's held index reads another column", FEED,
     '"SELECT idx FROM export_report WHERE chain = ?"', '"SELECT rowid FROM export_report WHERE chain = ?"', SITE),
]


def main():
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    caught = 0
    for i in pick:
        name, f, old, new, cmd = MUTANTS[i]
        p = os.path.join(ROOT, f)
        orig = open(p, 'rb').read()
        text = orig.decode()
        if text.count(old) != 1:
            print(f'{i} {name}: anchor found {text.count(old)} times')
            sys.exit(2)
        try:
            open(p, 'w').write(text.replace(old, new))
            r = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, timeout=1800)
            out = r.stdout + r.stderr
            if r.returncode == 0:
                verdict = 'MISSED'
            elif 'error[E' in out or 'could not compile' in out:
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
