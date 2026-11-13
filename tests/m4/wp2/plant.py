#!/usr/bin/env python3
"""M4-WP2's planted bugs: plants one mutant at a time, runs the tests that gate it, and restores the file byte for
byte. Each line prints CAUGHT with the failing tests, MISSED, or DID NOT COMPILE. Run from the worktree root:

    python3 tests/m4/wp2/plant.py          # every mutant
    python3 tests/m4/wp2/plant.py 0 3      # by number

`$CARGO_TARGET_DIR` defaults to `../target-m4b`.
"""
import os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..', '..'))
W = 'crates/git/src/watch.rs'
N = 'crates/git/src/inbox.rs'
S = 'crates/git/src/search.rs'
I = 'crates/identity/src/identity.rs'
A = 'crates/api/src/activity.rs'
GATES = ['-p', 'graftyard-api', '--test', 'activity_gates']
ID = ['-p', 'graftyard-identity', '--test', 'wp3']

# (name, file, old, new, test args)
MUTANTS = [
    # ---- the watcher index ----
    ('decide keeps stale updates', W,
     'watcher(meta, *account).is_none_or(|o| o.src < u.src())', 'true', GATES),
    ('decide keeps stale thread subscriptions', W,
     'thread_sub(meta, *n, *account).is_none_or(|o| o.src < u.src())', 'true', GATES),
    ('an unstar leaves the stargazer row', W,
     '                    w.delete("stargazer", &[int(t.0 as i64), acct(account)]);', '', GATES),
    ('an ignored repository still counts a subscriber', W,
     '        self.watching && !self.ignored', '        self.watching', GATES),
    ('the plan reads only past the cursor up to nothing (applied bound ignored)', W,
     '.filter(|e| e.index > u.counts_cursor && e.index <= applied)', '.filter(|e| e.index > applied)', GATES),
    ('the consumer cursor moves backward', W,
     '|| self.through <= cur {', '|| self.through == cur {', GATES),
    # ---- Notify ----
    ('the actor is notified of its own comment', N,
     '            if Some(a) == t.actor {\n                continue;\n            }', '', GATES),
    ('an ignored repository still notifies', N,
     '            if w.is_some_and(|w| w.ignored) {\n                continue;\n            }', '', GATES),
    ('a thread unsubscription is ignored', N,
     '            if unsubscribed && reason != Reason::Mention {', '            if unsubscribed && false {', GATES),
    ('watchers are not notified', N,
     '        for a in watch::subscribers(meta) {', '        for a in watch::subscribers(meta).into_iter().take(0) {', GATES),
    # ---- the Inbox ----
    ('a replayed entry applies again (sources ignored)', N,
     '        if self.sources.get(&chain).is_some_and(|i| *i >= index) {', '        if false {', GATES),
    ('an older notice overwrites a newer one', N,
     '                Some(r) if (r.updated, r.index) >= (n.at, n.index) => r.participating |= participating,',
     '                Some(r) if false => r.participating |= participating,', GATES),
    ('done never hides a thread', N,
     '                if mark.is_some_and(|m| m.done && !fresh(m)) {', '                if false {', GATES),
    ('read state is ignored', N,
     '                let unread = mark.is_none_or(|m| !m.read || fresh(m));', '                let unread = true;', GATES),
    ('the rebuild skips watched repositories (no INBOX_WINDOW)', N,
     '        for r in watched.chain(subscribed) {', '        for r in watched.chain(subscribed).take(0) {', GATES),
    ('the rebuild ignores sources', N,
     'self.sources.iter().map(|(c, i)| (*c, From::After(*i))).collect();', 'BTreeMap::new();', GATES),
    ('a snapshot is never due', N,
     '        self.dirty && self.snapshot_day.is_none_or(|d| d < now.0 / DAY_MS)', '        false', GATES),
    # ---- search ----
    ('an EU domain has no jurisdiction', S,
     '            Domain::Eu => Some(Jurisdiction::Eu),', '            Domain::Eu => None,', GATES),
    ('every domain admits every site', S,
     '                Domain::Eu | Domain::Us => s.jurisdiction == self.jurisdiction(),', '                Domain::Eu | Domain::Us => true,',
     GATES),
    ('a rebuild lands on the same site', S,
     'ok[self.generation.saturating_sub(1) as usize % ok.len()]', 'ok[0]', GATES),
    ('query text is FTS5 syntax (terms unquoted)', S,
     'format!("\\"{}\\"", t.replace(\'"\', "\\"\\""))', 't.clone()', GATES),
    ('a partial result without incomplete_results', S,
     '    let incomplete = answers.iter().any(Option::is_none);', '    let incomplete = false;', GATES),
    ('private documents without reach', S,
     '    let mut vis = "d.public = 1".to_string();', '    let mut vis = "1 = 1".to_string();', GATES),
    ('comments are not indexed', S,
     '            comments.push_str(&c.body);', '            let _ = &c.body;', GATES),
    ('a query reaches only the Default domain', S,
     '    for d in domains {', '    for d in domains.iter().take(0) {', GATES),
    # ---- identity ----
    ('the operator sets a record (SetRecord guard gone)', I,
     '            IdentityOp::SetRecord(r) => {\n                if self.cred.account != Some(account) {',
     '            IdentityOp::SetRecord(r) => {\n                if false {', ID),
    ('a repeat record emits again', I, '                if f.user().holds(r) {\n                    return none();', '                if false {\n                    return none();', ID),
    ('the counts cursor commits a backward entry', I,
     '                if *through <= f.user().counts_cursor {', '                if *through == f.user().counts_cursor {', ID),
    # ---- rendering ----
    ('the fold applies a cursor other than the one committed', I,
     '            IdentityEvent::CountsCursor(i) => self.more.more.counts_cursor = *i,',
     '            IdentityEvent::CountsCursor(i) => self.more.more.counts_cursor = Index(i.0.saturating_sub(1)),', ID),
    ('stargazers render without the star media type', A, 'const STAR_MEDIA: &str = "application/vnd.github.star+json";',
     'const STAR_MEDIA: &str = "application/vnd.github.star-json";', GATES),
]

env = dict(os.environ, PATH=os.path.expanduser('~/.cargo/bin:') + os.environ['PATH'],
           CARGO_TARGET_DIR=os.environ.get('CARGO_TARGET_DIR', os.path.join(ROOT, '..', 'target-m4b')),
           CARGO_INCREMENTAL='0')
only = sys.argv[1:] and set(int(x) for x in sys.argv[1:])
for n, (name, path, old, new, args) in enumerate(MUTANTS):
    if only and n not in only:
        continue
    f = os.path.join(ROOT, path)
    orig = open(f, 'rb').read()
    src = orig.decode()
    assert src.count(old) == 1, (name, 'anchor not unique', src.count(old))
    open(f, 'w').write(src.replace(old, new, 1))
    try:
        r = subprocess.run(['cargo', 'test', '-j', '3', *args, '--no-fail-fast'], cwd=ROOT, env=env,
                           capture_output=True, text=True)
        out = r.stdout + r.stderr
        failed = sorted({l.split()[1] for l in out.splitlines() if l.startswith('test ') and l.endswith('FAILED')})
        if ('error[' in out or out.lstrip().startswith('error:') or '\nerror: ' in out) and not failed:
            verdict = 'DID NOT COMPILE'
        else:
            verdict = 'CAUGHT' if r.returncode != 0 else 'MISSED'
        print(f'{n:2} {verdict:8} {name}: {", ".join(failed)}', flush=True)
    finally:
        open(f, 'wb').write(orig)
    assert open(f, 'rb').read() == orig
