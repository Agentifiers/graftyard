#!/usr/bin/env python3
"""narrow-reads: planted bugs in the narrow API read (`crates/api/src/narrow.rs`, the selections in `view.rs`, `rest.rs`,
and `gql.rs`, the member's cache and spacing in `workers/site/src/family.rs` and `member.rs`). Plants one mutant at a
time, runs the tests that gate it, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT
COMPILE. Run from the worktree root:

    python3 tests/nr/plant.py          # every mutant
    python3 tests/nr/plant.py 0 3      # by number
"""
import os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
NARROW = 'crates/api/src/narrow.rs'
VIEW = 'crates/api/src/view.rs'
REST = 'crates/api/src/rest.rs'
GQL = 'crates/api/src/gql.rs'
FAM = 'workers/site/src/family.rs'
MEM = 'workers/site/src/member.rs'
API = ['graftyard-api', '--test', 'narrow']
SITE = ['graftyard-site', '--lib', 'tests']

MUTANTS = [
    # Page boundaries.
    ('rest list: the page is cut before the sort (first N by number)', REST,
     "        match sort {\n            Some(\"updated\")",
     "        all.truncate(page.end());\n        match sort {\n            Some(\"updated\")", API),
    ('rest list: page 2 starts one item early', REST,
     'all.iter().skip(page.skip()).take(page.per_page)', 'all.iter().skip(page.skip().saturating_sub(1)).take(page.per_page)', API),
    ('rest list: `more` from the page, not the whole list (no next link)', REST,
     '        let more = all.len() > page.end();\n        Pick::of_numbers',
     '        let more = all.len() > page.per_page;\n        Pick::of_numbers', API),
    ('graphql: the cursor page includes the cursor item', GQL,
     '.filter(|i| after.is_none_or(|a| *i < a)).collect();', '.filter(|i| after.is_none_or(|a| *i <= a)).collect();', API),
    ('graphql: totalCount is the page, not every match', GQL,
     '        Pick::of_numbers(page.iter().copied().take(n), total, page.len() > n)',
     '        Pick::of_numbers(page.iter().copied().take(n), page.len().min(n), page.len() > n)', API),
    ('search: `last` takes the first n', GQL,
     '        if a.contains_key("last") && !a.contains_key("first") {\n            page.drain(..total.saturating_sub(n));',
     '        if false {\n            page.drain(..total.saturating_sub(n));', API),
    # Filters applied after paging.
    ('rest list: the label filter runs after the page is cut', REST,
     '                all.extend(r.list(kind, state, None, usize::MAX, &has).0.into_iter()',
     '                all.extend(r.list(kind, state, None, usize::MAX, &|_| true).0.into_iter()', API),
    ('graphql: the state filter is dropped from the selection', GQL,
     '            r.numbers(kind, state)\n', '            r.numbers(kind, StateFilter::All)\n', API),
    # The selection key: two different lists must not share one pick.
    ('rest list: the pick key ignores the query (another page answered)', REST,
     'r.dir.pick(&format!("issues?{query}"), &mut select)', 'r.dir.pick("issues", &mut select)', API),
    ('graphql: the pick key ignores the arguments', GQL,
     'r.dir.pick(&format!("{kind:?}{}", serde_json::to_string(a).unwrap_or_default()), &mut select)',
     'r.dir.pick(&format!("{kind:?}"), &mut select)', API),
    ('member: the first pick of a key is not kept, a later one is', NARROW,
     "        if !picks.iter().any(|(k, _)| k == key) {\n            picks.push((key.to_owned(), p.clone()));\n        }",
     "        picks.retain(|(k, _)| k != key);\n        picks.push((key.to_owned(), Pick::default()));", API),
    # Single-object reads leaking rows.
    ('member: a selection is recorded too (every item leaks)', NARROW,
     '        let was = self.paused.replace(true);', '        let was = self.paused.replace(false);', API),
    ('member: a scan records every row it returns (the next commit\'s statuses leak)', NARROW,
     '        self.keep(table, &out[..n]);', '        let _ = n;\n        self.keep(table, &out);', API),
    ('view: review comments are found but not picked (every comment leaks at the edge)', VIEW,
     '        let pick = self.dir.pick(&format!("review_comments/{}", n.0), &mut select);',
     '        let pick = select();', API),
    ('view: check run by id is not picked', VIEW,
     '        let pick = self.dir.pick(&format!("check_run/{}", run.0.0), &mut select);', '        let pick = select();', API),
    ('member: the access rows are left out', NARROW, '    core_rows.extend(access);', '    let _ = access;', API),
    # Whole reads where they are needed.
    ('member: a selection by login is answered narrowly', NARROW,
     '    if dir.whole.get() {\n        return json!({ "whole": true });\n    }', '', API),
    ('member: a mutation is answered narrowly', NARROW,
     '            if req.is_mutation().unwrap_or(true) {', '            if false {', API),
    # Logins.
    ('member: logins of accounts the tiers do not name are sent', NARROW,
     '    let known: Vec<i64> = asked.intersection(&named).map(|a| a.0.0).collect();',
     '    let known: Vec<i64> = asked.iter().map(|a| a.0.0).collect();', API),
    ('edge: the viewer is left out of the logins', NARROW, '        named.extend(viewer);', '', API),
    # Trust view.
    ('member: the trust view\'s rows are not recorded', NARROW,
     '                if *agent {\n                    crate::trust::layer', '                if false {\n                    crate::trust::layer', API),
    # A selection the member did not make.
    ('edge: a missing pick is answered over the narrow rows without a whole read', NARROW,
     '            None => {\n                self.missed.set(true);', '            None => {\n                self.missed.set(false);', API),
    # Stale reads and the member's cache.
    ('member cache: keyed by query only, so an older state is served', FAM,
     "v.iter().find(|(t, at, l, _)| t == text && *at >= floor && *l == loaded)", "v.iter().find(|(t, _, l, _)| t == text && *l == loaded)", SITE),
    ('member cache: another query\'s answer is served', FAM,
     "v.iter().find(|(t, at, l, _)| t == text && *at >= floor && *l == loaded)", "v.iter().find(|(_, at, l, _)| *at >= floor && *l == loaded)", SITE),
    ('member spacing: never spaced', FAM,
     '            Some(at) if now < at + NARROW_EVERY_MS => Some(at + NARROW_EVERY_MS - now),', '', SITE),
    ('member spacing: a cached answer waits too', FAM,
     '    if narrow_hit(repo, &text, floor, fold.meta.loaded()).is_some() {\n        return None;\n    }', '', SITE),
    ('follower: a narrow read is served while the metadata tier loads', FAM,
     '        Some(r) if r.get("api").is_some() => meta_loaded,', '        Some(r) if r.get("api").is_some() => true,', SITE),
    ('follower: a narrow read past its deadline still waits', MEM,
     '    (learner || narrow) && index && now + ms < until', '    (learner || narrow) && index', SITE),
    ('follower: a narrow read never waits on a follower', MEM,
     '    (learner || narrow) && index && now + ms < until', '    learner && index && now + ms < until', SITE),
]

ENV = dict(os.environ, CARGO_INCREMENTAL='0')


def run(t):
    pkg, kind, name = t
    args = ['cargo', 'test', '-q', '-p', pkg, kind, name] if kind == '--test' else ['cargo', 'test', '-q', '-p', pkg, '--lib', name]
    r = subprocess.run(args, cwd=ROOT, env=ENV, capture_output=True, text=True, timeout=1800)
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
