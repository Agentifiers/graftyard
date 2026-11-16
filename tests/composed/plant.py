#!/usr/bin/env python3
"""composed-miss: planted bugs in this branch's new code (the container's composed miss, chain steps, chain install
and clone-cache spool, a want or chain behind HEAD, compose=only and the network lock; the shard object's request rule
and chain file layout). The edge's streamed hit is planted by tests/composed/bed/run.sh with WHOLE=1, and the miss line's
first-chunk forward by running it with FWD=1 against a planted `on_first_chunk`. Plants one mutant at a time, runs the
tests that gate it, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run
from the worktree root:

    python3 tests/composed/plant.py          # every mutant
    python3 tests/composed/plant.py 0 3      # by number
"""
import os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
ENV = dict(os.environ, CARGO_INCREMENTAL='0')
ENV['PATH'] = os.path.expanduser('~/.cargo/bin') + ':' + ENV.get('PATH', '')

CG = 'containers/git/server/compose.go'
MAIN = 'containers/git/server/main.go'
RS = 'crates/git/src/compose.rs'
KEY = 'crates/git/src/key.rs'
MISS = ('go', 'TestComposedMiss$')
FILL = ('go', 'TestCancelledFillStillSpoolsTheWholeAnswer')
COMPOSE_RS = ('cargo', 'graftyard-git', 'compose::')
KEY_RS = ('cargo', 'graftyard-git', 'key::')

MUTANTS = [
    # The composed pack.
    ('pack header counts only the chain, not the want\'s own pack', CG,
     '\tcount := binary.BigEndian.Uint32(inc[8:12])\n', '\tcount := uint32(0)\n', MISS),
    ('the want\'s own objects are left out', CG, '\tpw.Write(inc[12 : len(inc)-20])\n', '', MISS),
    ('each part is sent with its own trailer', CG, 'io.NewSectionReader(p.f, 12, p.body)',
     'io.NewSectionReader(p.f, 12, p.body+20)', MISS),
    ('no shallow line, so the client takes a depth-1 pack as whole history', CG,
     '\tpktLine(bw, "shallow "+want)\n', '', MISS),
    ('objects the chain holds are sent again', CG, 'if strings.HasSuffix(l, " missing") && !seen[id] {',
     'if (strings.HasSuffix(l, " missing") || true) && !seen[id] {', MISS),
    ('a tip move\'s new trees are left out of its part', CG, 'g(rp, env, nil, "diff-tree", "-r", "-t", from, to)',
     'g(rp, env, nil, "diff-tree", "-r", from, to)', MISS),
    # When to compose.
    ('include-tag on an annotated tag is composed without the tag', CG, 'if includeTag && annotatedTag.Match(refs) {',
     'if false && annotatedTag.Match(refs) {', MISS),
    ('a want no ref names is composed', CG, 'len(bytes.TrimSpace(refs)) == 0 {', 'len(refs) < 0 {', MISS),
    ('a have is ignored', CG, '\t\tcase l == "thin-pack" || l == "no-progress":\n',
     '\t\tcase l == "thin-pack" || l == "no-progress" || strings.HasPrefix(l, "have "):\n', MISS),
    ('ofs-delta is not required', CG, 'fetch && depth1 && done && ofs && want != ""', 'fetch && depth1 && done && (ofs || true) && want != ""',
     MISS),
    ('git clone\'s repeated want is refused (every real clone goes stock)', CG, '(want != "" && want != l[5:])',
     'want != ""', MISS),
    ('a want behind HEAD goes to stock', CG,
     '\t\tif _, err := m.git(rp, nil, nil, "merge-base", "--is-ancestor", want, "HEAD"); err != nil {',
     '\t\tif _, err := m.git(rp, nil, nil, "merge-base", "--is-ancestor", want, "HEAD"); err == nil || err != nil {', MISS),
    ('any want is composed, behind HEAD or not', CG,
     '\t\tif _, err := m.git(rp, nil, nil, "merge-base", "--is-ancestor", want, "HEAD"); err != nil {',
     '\t\tif _, err := m.git(rp, nil, nil, "merge-base", "--is-ancestor", want, "HEAD"); err != nil && false {', MISS),
    ('the diff cap is never applied', CG, '\tif len(objs) > composeDiffMax {', '\tif len(objs) < 0 {', MISS),
    ('compose=only falls through to stock', MAIN, '\tif only {\n\t\tw.WriteHeader(409)\n\t\treturn\n\t}',
     '\tif false {\n\t\tw.WriteHeader(409)\n\t\treturn\n\t}', MISS),
    # The network lock.
    ('a composed miss waits for the network lock', MAIN,
     '\t\tw.Header().Set("content-type", "application/x-git-upload-pack-result")\n\t\tanswered, why := composeMiss(',
     '\t\tl.RLock()\n\t\tdefer l.RUnlock()\n\t\tw.Header().Set("content-type", "application/x-git-upload-pack-result")\n\t\tanswered, why := composeMiss(',
     ('go', 'TestComposedMissesDoNotQueueBehindTheLock')),
    ('a spooled answer holds the lock to its end', MAIN, 'out = &tee{file: f, client: out, gone: func() { release() }}',
     'out = &tee{file: f, client: out, gone: func() {}}', ('go', 'TestComposedMissesDoNotQueueBehindTheLock')),
    # Chain steps.
    ('no fold', CG, 'if !have || len(h.Parts)+1 >= foldEvery {', 'if !have {', MISS),
    ('a part holds every new object, the base\'s too', CG,
     '\t\t\tobjs, err = missingFromChain(net, rp, objs, m.gitLow)\n\t\t}\n\t\tif err == nil {\n\t\t\tname = "part-"',
     '\t\t}\n\t\tif err == nil {\n\t\t\tname = "part-"', MISS),
    ('a head is installed with a part missing', CG, '\tif len(missing) > 0 {\n\t\tw.Header().Set("content-type"',
     '\tif len(missing) < 0 {\n\t\tw.Header().Set("content-type"', MISS),
    # Priority.
    ('chain packs built at normal priority', CG,
     '\tout, err := m.gitLow(rp, env, strings.NewReader(strings.Join(objs, "\\n")+"\\n"), "pack-objects", "-q",',
     '\tout, err := m.git(rp, env, strings.NewReader(strings.Join(objs, "\\n")+"\\n"), "pack-objects", "-q",',
     ('go', 'TestChainBuildsRunAtNice19')),
    ('a part\'s diff runs at normal priority', CG, 'objs, err := newObjects(rp, env, h.Tree, tip, m.gitLow)',
     'objs, err := newObjects(rp, env, h.Tree, tip, m.git)', ('go', 'TestChainBuildsRunAtNice19')),
    ('the composed miss runs at nice 19', CG, 'objs, err := newObjects(rp, env, h.Tree, want, m.git)',
     'objs, err := newObjects(rp, env, h.Tree, want, m.gitLow)', ('go', 'TestChainBuildsRunAtNice19')),
    ('the niced build drops the CPU meter', MAIN,
     '\treturn runGit(exec.Command("nice", append([]string{"-n", "19", "git"}, args...)...), m, dir, env, stdin, args...)',
     '\treturn runGit(exec.Command("nice", append([]string{"-n", "19", "git"}, args...)...), nil, dir, env, stdin, args...)',
     ('go', 'TestChainBuildsRunAtNice19')),
    # The fill.
    ('a cancelled client cuts the spool too', CG, '\tif t.clientErr == nil {\n\t\tif _, t.clientErr = t.client.Write(p); t.clientErr != nil && t.gone != nil {',
     '\tif _, err := t.client.Write(p); err != nil {\n\t\treturn 0, err\n\t}\n\tif false {\n\t\tif _, t.clientErr = t.client.Write(p); t.clientErr != nil && t.gone != nil {', FILL),
    ('a cut spool is kept', CG, 'if ok && wholeResponse(spoolPath(id, ".part")) {', 'if true {', FILL),
    ('two fills of one key spool at once', CG, 'if _, busy := spools.LoadOrStore(id, ch); busy {',
     'if _, busy := spools.LoadOrStore(id+fmt.Sprint(time.Now().UnixNano()), ch); busy {', FILL),
    ('the stock answer is not spooled', MAIN, '} else if _, err := io.CopyBuffer(out, stdout, make([]byte, 64<<10)); err != nil {',
     '} else if _, err := io.CopyBuffer(w, stdout, make([]byte, 64<<10)); err != nil {', FILL),
    # The shard object's rule and layout.
    ('the shard waits for the chain on a fetch with a filter', RS, '            a if OTHER_ARGS.contains(&a) => {}',
     '            a if OTHER_ARGS.contains(&a) || a.starts_with("filter ") => {}', COMPOSE_RS),
    ('the shard refuses git clone\'s repeated want', RS, '&& want.is_none_or(|o| o == w)', '&& want.is_none()',
     COMPOSE_RS),
    ('after a fold the old base is kept as a file to fetch', RS,
     '        self.files().into_iter().filter(|f| !have.contains(f)).collect()',
     '        self.files().into_iter().filter(|f| !have.contains(f) || f.starts_with("base-")).collect()', COMPOSE_RS),
    ('a part is named by its tip alone', RS, '            out.push(format!("part-{from}-{p}"));',
     '            out.push(format!("part-{p}-{p}"));', COMPOSE_RS),
    ('compose keys accept any file name', KEY, 'if *file == "head" || crate::compose::file_name_ok(file)',
     'if !file.is_empty()', KEY_RS),
    # The shard object's `shard miss` line.
    ('a miss line times each step from the one before, not from arrival', RS,
     '        let ms = |t: u64| t.saturating_sub(self.arrived);',
     '        let ms = |t: u64| t.saturating_sub(self.ready.unwrap_or(self.arrived));', COMPOSE_RS),
    ('a stock 200 with a reason is logged as composed', RS,
     '            Some((_, 200, c)) => c.clone().unwrap_or_else(|| "stock: not composable".into()),',
     '            Some((_, 200, _)) => "composed".into(),', COMPOSE_RS),
    ('a miss line drops the first chunk\'s time', RS, '            "first_ms": self.first.map(ms),',
     '            "first_ms": self.served.as_ref().map(|(t, ..)| ms(*t)),', COMPOSE_RS),
]


def run(t):
    if t[0] == 'go':
        r = subprocess.run(['go', 'test', '-count=1', '-timeout', os.environ.get('PLANT_GO_TIMEOUT', '300s'), '-run', t[1], '.'],
                           cwd=os.path.join(ROOT, 'containers/git/server'), capture_output=True, text=True, timeout=600)
        out = r.stdout + r.stderr
        return r.returncode, out, 'build failed' in out or 'syntax error' in out or 'undefined:' in out
    _, pkg, name = t
    r = subprocess.run(['cargo', 'test', '-q', '-p', pkg, '--lib', name], cwd=ROOT, env=ENV, capture_output=True,
                       text=True, timeout=1200)
    out = r.stdout + r.stderr
    return r.returncode, out, 'error[E' in out or 'could not compile' in out


def main():
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    caught = 0
    for i in pick:
        name, f, old, new, t = MUTANTS[i]
        p = os.path.join(ROOT, f)
        orig = open(p, 'rb').read()
        s = orig.decode()
        if s.count(old) != 1:
            print(f'{i:2} NOT FOUND  {name}')
            continue
        open(p, 'w').write(s.replace(old, new))
        try:
            code, out, broke = run(t)
        finally:
            open(p, 'wb').write(orig)
        if broke:
            print(f'{i:2} DID NOT COMPILE  {name}')
        elif code != 0:
            caught += 1
            fails = [l.split()[1] for l in out.splitlines() if l.startswith('---- ') or l.startswith('--- FAIL')]
            print(f'{i:2} CAUGHT  {name}  [{", ".join(fails[:3])}]')
        else:
            print(f'{i:2} MISSED  {name}')
        sys.stdout.flush()
    print(f'{caught} of {len(list(pick))} caught')


if __name__ == '__main__':
    main()
