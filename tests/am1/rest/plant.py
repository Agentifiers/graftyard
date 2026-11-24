#!/usr/bin/env python3
"""am1-rest: planted bugs in the new code (the archive alarm's logic and its site host, the install's agent Apps, the
ready-for-review and labels write surfaces, the session-ref filter in the git cache container, and the A1 driver's
pack builder). Plants one mutant at a time, runs the tests that gate it, and restores the file byte for byte. Each
line prints CAUGHT with the failing tests, MISSED, or DID NOT COMPILE. Never run it while a site day builds from this
worktree. Run from the worktree root:

    python3 tests/am1/rest/plant.py          # every mutant
    python3 tests/am1/rest/plant.py 0 3      # by number
"""
import os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..', '..'))
LA = 'crates/git/src/log_archive.rs'
ARCH = 'workers/site/src/archive.rs'
FAM = 'workers/site/src/family.rs'
APPS = 'tests/am1/rest/apps.mjs'
GQL = 'crates/api/src/graphql.rs'
REST = 'crates/api/src/rest.rs'
GO = 'containers/git/server/main.go'
A1 = 'tests/am1/rest/a1.mjs'
WIRE = 'tests/am1/rest/wire.mjs'
RAFT_INSTALL = 'crates/raft/src/member/install.rs'

ARCHIVE = ['cargo', '-p', 'graftyard-git', '--test', 'log_archive', '']
SITE = ['cargo', '-p', 'graftyard-site', '--lib', 'archive']
AGENT_APPS = ['cargo', '-p', 'graftyard-identity', '--test', 'agent_apps', '']
SURF = ['cargo', '-p', 'graftyard-api', '--test', 'surfaces', 'ready_for_review']
LF13 = ['cargo', '-p', 'graftyard-raft', '--test', 'lf13', '']
GOT = ['go', 'TestSession']
SELF = ['node', A1, '--selftest']

# (name, file, old, new, test args)
MUTANTS = [
    ('archive: no slack, so the log is cut to exactly the cap', LA,
     '    max_bytes - max_bytes / ARCHIVE_SLACK\n}',
     '    max_bytes\n}', ARCHIVE),
    ('archive: a gap in the stored log is not refused', LA,
     '            && i.0 != last.0 + 1\n', '            && i.0 < last.0\n', ARCHIVE),
    ('archive: next archives past the last promotion', LA,
     'Next::Archive { from, to: Index(to.0.min(done)) }', 'Next::Archive { from, to }', ARCHIVE),
    ('archive: next promotes again over a covered range', LA,
     'if done < from.0 { Next::Promote', 'if done < to.0 { Next::Promote', ARCHIVE),
    ('archive: a promotion takes an entry over PROMOTE_MAX', LA,
     '            if !first && packed + b > promote_max {',
     '            if !first && packed > promote_max {', ARCHIVE),
    ('archive: a stored pack is promoted under its pack id, not its checksum', LA,
     '                PackRef::Stored { checksum, .. } => checksum,',
     '                PackRef::Stored { id, .. } => Digest(Sha256::digest(id.0.0.to_be_bytes()).into()),', ARCHIVE),
    ('archive: recorded while held in one bucket', LA,
     '        if held < 2 {\n            return Err(format!("archive {key} is in {held} buckets"));',
     '        if held < 1 {\n            return Err(format!("archive {key} is in {held} buckets"));', ARCHIVE),
    ('archive: the promote request id ignores the range', LA,
     'request(&[b"graftyard-log-promote", chain.to_string().as_bytes(), &from.0.to_be_bytes(), &to.0.to_be_bytes()])',
     'request(&[b"graftyard-log-promote", chain.to_string().as_bytes()])', ARCHIVE),
    ('archive: before both tiers are snapshotted', LA,
     '            .all(|t| !crate::tiers::snapshots(&*fold.core, t).is_empty());',
     '            .any(|t| !crate::tiers::snapshots(&*fold.core, t).is_empty());', ARCHIVE),
    ('site: a Promoted entry\'s request row does not name its pack', FAM,
     '        json!(git::repo::packs_of(events).into_iter().chain(promoted).collect::<Vec<_>>())',
     '        let _ = promoted;\n        json!(git::repo::packs_of(events))', SITE),
    ('site: an Archived entry is not a range members truncate through', FAM,
     '            git::repo::RepoEvent::Archived(a) if matches!(a.key, git::R2Key::Archive { .. }) => {',
     '            git::repo::RepoEvent::Archived(a) if !matches!(a.key, git::R2Key::Archive { .. }) => {', SITE),
    ('apps: graftyard-workspace registered as an MCP App', APPS,
     '  { slug: "graftyard-workspace", name: "graftyard-workspace", kind: "graftyard_run", agentKind: "GraftyardRun",',
     '  { slug: "graftyard-workspace", name: "graftyard-workspace", kind: "mcp", agentKind: "Mcp",', AGENT_APPS),
    ('apps: Graftyard MCP flagged Graftyard-run', APPS,
     '  { slug: "graftyard-mcp", name: "Graftyard MCP", kind: "mcp", agentKind: "Mcp",',
     '  { slug: "graftyard-mcp", name: "Graftyard MCP", kind: "graftyard_run", agentKind: "GraftyardRun",', AGENT_APPS),
    ('gql: ready-for-review sets draft', GQL,
     '            let draft = field == "convertPullRequestToDraft";', '            let draft = true;', SURF),
    ('gql: removeLabelsFromLabelable adds', GQL,
     'if field == "addLabelsToLabelable" { (labels, SmallVec::new()) } else { (SmallVec::new(), labels) };',
     '(labels, SmallVec::new());', SURF),
    ('rest: PUT keeps the labels it should drop', REST,
     '            (add, have.iter().filter(|l| !want.contains(l)).copied().collect())', '            (add, Vec::new())', SURF),
    ('rest: POST of an unknown label is accepted', REST,
     'label_names(body).ok_or_else(invalid)?.iter().map(|x| id_of(x).ok_or_else(invalid)).collect()',
     'Ok(label_names(body).ok_or_else(invalid)?.iter().filter_map(|x| id_of(x)).collect())', SURF),
    ('rest: removing a label the issue lacks is not 404', REST,
     'let id = id_of(&unpercent(x)).filter(|id| have.contains(id)).ok_or_else(|| {',
     'let id = id_of(&unpercent(x)).ok_or_else(|| {', SURF),
    ('rest: delete-all answers 200', REST,
     '    if empty {\n        return Reply::empty(204);', '    if empty && false {\n        return Reply::empty(204);', SURF),
    ('go: host-owned refs advertised in v0', GO,
     '\ta := []string{"-c", "uploadpack.hideRefs=" + hostRefs}', '\ta := []string{}', GOT),
    ('go: a ref-prefix of refs/graftyard/ unhides the sessions', GO,
     '\t\tcase strings.HasPrefix(line, "ref-prefix "+sessionRefs):', '\t\tcase strings.HasPrefix(line, "ref-prefix "+hostRefs):', GOT),
    ('go: a sibling prefix lists every session ref', GO,
     '\tif len(sessions) > 0 {\n\t\t// An ls-refs reply: small, so buffered and cut.',
     '\tif false {\n\t\t// An ls-refs reply: small, so buffered and cut.', GOT),
    ('go: a fetch command unhides the sessions', GO,
     '\t\tcase strings.HasPrefix(line, "command="):\n\t\t\treturn nil', '\t\tcase strings.HasPrefix(line, "command="):\n\t\t\tls = true', GOT),
    ('a1: pack entries carry the wrong object type', WIRE,
     'const TYPE = { commit: 1, tree: 2, blob: 3 };', 'const TYPE = { commit: 1, tree: 3, blob: 2 };', SELF),
    ('a1: an ng reply reads as ok', WIRE,
     '  const ng = lines.map((l) => l.match(/^ng (\\S+) ([^\\n]*)/)).find(Boolean);', '  const ng = null;', SELF),
    ('raft (LF-13 premise): a follower behind the archive is sent entries, not an install', RAFT_INSTALL,
     '        if prev >= source.through {\n            return false;',
     '        if prev >= source.through || true {\n            return false;', LF13),
]


# A hang counts as caught (README, "Planted bugs").
LIMIT = ['timeout', '-k', '10', '600']


def run(args):
    kind, *rest = args
    env = dict(os.environ, PATH=os.path.expanduser('~/.cargo/bin:') + os.environ['PATH'],
               CARGO_TARGET_DIR=os.environ.get('CARGO_TARGET_DIR', os.path.join(ROOT, 'target', 'plants')),
               CARGO_INCREMENTAL='0')
    if kind == 'cargo':
        *pkg, filt = rest
        cmd = ['cargo', 'test', '-j', '3', *pkg, '--no-fail-fast'] + ([filt] if filt else [])
        return subprocess.run([*LIMIT, *cmd], cwd=ROOT, env=env, capture_output=True, text=True)
    if kind == 'go':
        return subprocess.run([*LIMIT, 'go', 'test', '-count=1', '-run', rest[0], './...'],
                              cwd=os.path.join(ROOT, 'containers/git/server'), env=env, capture_output=True, text=True)
    return subprocess.run([*LIMIT, 'node', *rest], cwd=ROOT, env=env, capture_output=True, text=True)


def failures(kind, out):
    if kind == 'cargo':
        return sorted({l.split()[1] for l in out.splitlines() if l.startswith('test ') and l.endswith('FAILED')})
    if kind == 'go':
        return sorted({l.split()[2] for l in out.splitlines() if l.startswith('--- FAIL')})
    return ['selftest'] if out else []


def plant(n, name, path, old, new, args):
    f = os.path.join(ROOT, path)
    orig = open(f, 'rb').read()
    src = orig.decode()
    assert src.count(old) == 1, (name, 'anchor not unique', src.count(old))
    open(f, 'w').write(src.replace(old, new, 1))
    try:
        r = run(args)
        out = r.stdout + r.stderr
        failed = failures(args[0], out) if r.returncode != 0 else []
        if r.returncode == 124:
            failed = ['(timed out)', *failed]
        broke = ('error[' in out or '\nerror: could not compile' in out) if args[0] == 'cargo' else \
            ('build failed' in out or 'SyntaxError' in out)
        if broke and not failed:
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
