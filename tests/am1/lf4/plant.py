#!/usr/bin/env python3
"""lf4-rest: planted bugs in the six REST reads LF-4 needed (`crates/api/src/rest.rs`, `view.rs`) and the commit fetch
from the git cache shard (`crates/git/src/commit_fetch.rs`); lf4-rest-2 and lf4-rest-3: in the two REST writes and the
check-run create's id. Plants one mutant at a time, runs the tests that gate it,
and restores the file byte for byte. Prints CAUGHT with the failing tests, MISSED, or DID NOT COMPILE.

    python3 tests/am1/lf4/plant.py          # every mutant
    python3 tests/am1/lf4/plant.py 0 3      # by number
"""
import os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..', '..'))
REST = 'crates/api/src/rest.rs'
VIEW = 'crates/api/src/view.rs'
CF = 'crates/git/src/commit_fetch.rs'
SURF = ['-p', 'graftyard-api', '--test', 'surfaces', 'rest_reads']
FETCH = ['-p', 'graftyard-git', '--lib', 'commit_fetch']
ISSUE_OPS = 'crates/git/src/issue_ops.rs'
TIME = 'crates/wire/src/time.rs'
WRITES = ['-p', 'graftyard-api', '--test', 'surfaces', 'rest_writes']
GATES = ['-p', 'graftyard-git', '--test', 'agent_gates', 'a_check_run_is_recorded']
TIMES = ['-p', 'graftyard-wire', '--lib', 'time']
TIERS = 'crates/git/src/tiers.rs'
CALL = 'crates/git/src/issue_ops/call.rs'
IDS = ['-p', 'graftyard-git', '--test', 'agent_gates', 'check_run_creates']

MUTANTS = [
    ('an issue answers as a pull request', REST,
     '        Some(it) if it.is_pull() => {\n            let mut asks',
     '        Some(it) => {\n            let mut asks', SURF),
    ('an issue comment renders without its author', REST,
     '"user": simple_user(r.host, r.dir, row.author), "created_at": iso(row.created),',
     '"user": simple_user(r.host, r.dir, None), "created_at": iso(row.created),', SURF),
    ('pending reviews listed', REST,
     '    rows.retain(|(_, row)| row.state != ReviewState::Pending);\n', '', SURF),
    ('a dismissed review keeps its state', REST,
     'let state = if row.dismissed.is_some() {', 'let state = if false {', SURF),
    ('review comments of every pull request', VIEW,
     'matches!(row.on, CommentTarget::Review { n: on, .. } if on == n)',
     'matches!(row.on, CommentTarget::Review { .. })', SURF),
    ('review comments in id order, not oldest first', VIEW,
     'rows.sort_by_key(|(id, row)| (row.created, *id));', 'rows.sort_by_key(|(id, _)| *id);', SURF),
    ('check runs of every commit', VIEW,
     '.prefix("check_run", &[sha_key(sha)])', '.prefix("check_run", &[])', SURF),
    ('check runs oldest first', REST,
     'rows.sort_by_key(|t| std::cmp::Reverse(t.rec.run));', 'rows.sort_by_key(|t| t.rec.run);', SURF),
    ('a check run by id finds the first row', VIEW,
     '"check_run", k).is_some_and(|t| t.rec.run == run)', '"check_run", k).is_some_and(|_| true)', SURF),
    ('a commit message keeps its trailing newline', REST,
     'String::from_utf8_lossy(message.strip_suffix(b"\\n").unwrap_or(message))', 'String::from_utf8_lossy(message)', SURF),
    ('only the first parent', REST,
     '            parents.push(parent);', '            if parents.is_empty() {\n                parents.push(parent);\n            }', SURF),
    ('the author read from the committer line', REST,
     '        } else if let Some(user) = line.strip_prefix(b"author ") {',
     '        } else if let Some(user) = line.strip_prefix(b"committer ") {\n            author = raw_commit_user(user);\n            committer = author.clone();\n        } else if let Some(user) = line.strip_prefix(b"author ") {', SURF),
    ('fetch: the commit is not checked against its id', CF,
     '    if id != oid.0 {', '    if id != oid.0 && false {', FETCH),
    ('fetch: progress packets read as pack data', CF,
     '                Some((1, rest)) => pack.extend_from_slice(rest),',
     '                Some((1 | 2, rest)) => pack.extend_from_slice(rest),', FETCH),
    ('fetch: the size read as the first header byte only', CF,
     '    while more {', '    while more && false {', FETCH),
    ('fetch: the request asks for the history', CF,
     '"deepen 1", "filter tree:0", "done"] {', '"done"] {', FETCH),
    # lf4-rest-2: the two writes.
    ('a reply keeps the line the body names, not its parent\'s', REST,
     '            Some((id, at))\n        }) else {',
     '            Some((id, LineAnchor { line: body.get("line").and_then(Value::as_u64).unwrap_or(1) as u32, ..at }))\n        }) else {',
     WRITES),
    ('a review comment\'s side defaults to LEFT', REST,
     '        None => DiffSide::Right,', '        None => DiffSide::Left,', WRITES),
    ('a start_line at or past line accepted', REST,
     '.filter(|start| *start < line)', '.filter(|start| *start <= line + 10)', WRITES),
    ('a conclusion leaves the status as sent', REST,
     '    if conclusion.is_some() {\n        status = CheckStatus::Completed;', '    if false {\n        status = CheckStatus::Completed;', WRITES),
    ('completed_at not defaulted for a completed run', REST,
     '        None if status == CheckStatus::Completed => Some(now),', '        None if false => Some(now),', WRITES),
    ('a not-pull refusal answers 422', REST,
     '        E::NotFound | E::NotPull => Reply::not_found(),', '        E::NotFound => Reply::not_found(),', WRITES),
    ('a first version may take another run\'s id', ISSUE_OPS,
     'if run.version == 1 && v.meta().prefix("check_run", &[]).iter().any(|(k, _)| other(k)) {',
     'if false && v.meta().prefix("check_run", &[]).iter().any(|(k, _)| other(k)) {', GATES),
    ('a first version refused on any taken id, a re-delivery too', ISSUE_OPS,
     '(t.rec.head_sha, t.rec.app, &t.rec.name) != (run.head_sha, run.app, &run.name)', 't.rec.version > 0', GATES),
    ('an App records a run as another App', ISSUE_OPS,
     '                    && run.app != app\n', '                    && run.app != app && false\n', GATES),
    # lf4-rest-3: a create's id from the block, in decide.
    ('a create keeps the id the edge sent', ISSUE_OPS,
     '                        let id = v.next_id(Tier::Metadata)?;\n', '                        let id = c.run.0;\n', IDS),
    ('a created run does not move the cursor', TIERS,
     '                assigned(w, ev, Tier::Metadata, u.run.0);\n', '', IDS),
    ('a create\'s suite keeps the id the edge sent', ISSUE_OPS,
     'suite: crate::issues::CheckSuiteId(id), version: 1', 'version: 1', IDS),
    ('the 201 renders the minted id, not the acked one', REST,
     '        graftyard_git::issue_ops::IssueOut::Id(id) => id.0,\n', '', WRITES),
    ('a re-sent create answers Done', CALL,
     '            IssueOp::CreateCheck(c) => IssueOp::CreateCheck(c.clone()),\n', '', IDS),
    ('a create sent in format 32.5', CALL,
     '        if matches!(self.op, IssueOp::CreateCheck(_)) {\n            return REPO_FORMAT_CHECKS;\n        }\n', '', IDS),
    ('an offset read with the wrong sign', TIME,
     "if *sign == b'+' { seconds } else { -seconds }", "if *sign == b'+' { -seconds } else { seconds }", TIMES),
]


def run(args):
    env = dict(os.environ, PATH=os.path.expanduser('~/.cargo/bin:') + os.environ['PATH'], CARGO_INCREMENTAL='0')
    return subprocess.run(['cargo', 'test', '-j', '4', *args], cwd=ROOT, env=env, capture_output=True, text=True)


def plant(n, name, path, old, new, args):
    f = os.path.join(ROOT, path)
    orig = open(f, 'rb').read()
    src = orig.decode()
    assert src.count(old) == 1, (name, 'anchor not unique', src.count(old))
    open(f, 'w').write(src.replace(old, new, 1))
    try:
        r = run(args)
        out = r.stdout + r.stderr
        failed = sorted({l.split()[1] for l in out.splitlines() if l.startswith('test ') and l.endswith('FAILED')})
        if r.returncode != 0 and not failed and ('error[' in out or 'could not compile' in out):
            verdict = 'DID NOT COMPILE'
        else:
            verdict = 'CAUGHT' if r.returncode != 0 and failed else 'MISSED'
        print(f'{n:2} {verdict:8} {name}: {", ".join(failed)}', flush=True)
    finally:
        open(f, 'wb').write(orig)
    assert open(f, 'rb').read() == orig


only = sys.argv[1:] and set(int(x) for x in sys.argv[1:])
for n, (name, path, old, new, args) in enumerate(MUTANTS):
    if not only or n in only:
        plant(n, name, path, old, new, args)
