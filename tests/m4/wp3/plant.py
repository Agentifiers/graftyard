#!/usr/bin/env python3
"""M4-WP3's planted bugs: plants one mutant at a time, runs the tests that gate it, and restores the file byte for
byte. Each line prints CAUGHT with the failing tests, MISSED, or DID NOT COMPILE. Run from the worktree root:

    python3 tests/m4/wp3/plant.py          # every mutant
    python3 tests/m4/wp3/plant.py 0 3      # by number

`$CARGO_TARGET_DIR` defaults to `../target-m4a`.
"""
import os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..', '..'))
J = 'crates/actions/src/job.rs'
S = 'crates/actions/src/shard.rs'
O = 'crates/actions/src/run_owner.rs'
C = 'crates/git/src/run_checks.rs'
W = 'crates/actions/src/workflow/mod.rs'
WT = 'crates/actions/src/workflow/template.rs'
WE = 'crates/actions/src/workflow/expr.rs'
WM = 'crates/actions/src/workflow/matrix.rs'
WF = 'crates/actions/src/workflow/filter.rs'
ACT = ['-p', 'graftyard-actions', '--test', 'gates', '--lib']
WFT = ['-p', 'graftyard-actions', '--test', 'workflows', '--lib']

# (name, file, old, new, test args)
MUTANTS = [
    # ---- the job state machine ----
    ('a lease never expires a running job', J,
     '        (S::InProgress { lease_until, .. }, E::LeaseExpired) if now >= *lease_until => Some(done(Conclusion::Failure)),',
     '', ACT),
    ('an expired lease from InProgress requeues the job', J,
     '        (S::InProgress { lease_until, .. }, E::LeaseExpired) if now >= *lease_until => Some(done(Conclusion::Failure)),',
     '        (S::InProgress { lease_until, .. }, E::LeaseExpired) if now >= *lease_until => Some(S::Queued { labels: labels.clone(), since: now }),',
     ACT),
    ('every renew rewrites the lease', J,
     '            if lease_until.0 < now.0 + LEASE_REWRITE_UNDER.as_millis() as u64 {',
     '            if true {', ACT),
    ('a renew extends the lease by idle time past JOB_LEASE', J,
     '    let lease = after(now, JOB_LEASE);', '    let lease = after(now, JOB_LEASE * 2);', ACT),
    ('a completed job takes a cancel', J,
     '        (S::Completed { .. }, _) => None,', '', ACT),
    # ---- the shard ----
    ('a match commits against a stale version', S,
     '        if j.version != version {\n            return Err(ActionsErr::Stale(j.version));\n        }', '', ACT),
    ('a match ignores labels', S,
     '            && !s.runner(*r).is_some_and(|rec| j.spec.labels.fits(&rec.labels))',
     '            && s.runner(*r).is_none()', ACT),
    ('a second run for one trigger', S,
     '        if s.has_key(&run.key()) {\n            return Err(ActionsErr::Duplicate);\n        }', '', ACT),
    ('a stall moves a lease by more than its length', S,
     '            if *lease_until >= x.since =>\n        {\n            lease_until.0 += by\n        }',
     '            if *lease_until >= x.since =>\n        {\n            lease_until.0 += by * 2\n        }', ACT),
    ('a stall moves no lease', S,
     '            if *lease_until >= x.since =>\n        {\n            lease_until.0 += by\n        }',
     '            if *lease_until >= x.since =>\n        {\n            lease_until.0 += 0 * by\n        }', ACT),
    ('a stall revives a lease that had ended before it', S,
     '            if *lease_until >= x.since =>', '            if true =>', ACT),
    ('a failed need still releases its dependents', S,
     '            let event = if cs.iter().all(|c| *c == Conclusion::Success) {',
     '            let event = if cs.iter().all(|c| *c != Conclusion::Cancelled) {', ACT),
    ('a skip does not release its own dependents', S,
     '                done.insert(j.spec.id, Conclusion::Skipped);\n                JobEvent::NeedsFailed',
     '                JobEvent::NeedsFailed', ACT),
    ('the RunOwner cursor moves back', S,
     '                if *through <= s.owner_cursor() {', '                if *through == s.owner_cursor() {', ACT),
    # ---- RunOwner and the repository's writes ----
    ('RunOwner never cancels a gone repository\'s run', O,
     '                then.push(ActionsOp::CancelRun { run: *r });\n', '', ACT),
    ('a replayed check run moves its row back', C,
     '            if get_rec::<Timed<CheckRunUpdate>>(meta, "check_run", &key).is_some_and(|t| t.rec.version >= u.version) {',
     '            if get_rec::<Timed<CheckRunUpdate>>(meta, "check_run", &key).is_some_and(|t| t.rec.version > u.version) {',
     ACT),
    ('a completed check writes no conclusion', C,
     '                    conclusion: if done { u.conclusion } else { None },', '                    conclusion: None,',
     ACT),
    ('every job\'s check row is version 1', O,
     '        CheckStatus::Completed => 3,\n        CheckStatus::InProgress => 2,',
     '        CheckStatus::Completed => 1,\n        CheckStatus::InProgress => 1,', ACT),
    # ---- the workflow parser, against the oracle's JSON ----
    ('an if that calls a status function is wrapped anyway', W,
     '            Ok(Some(a)) if expr::has_status_call(&a) => cond,', '            Ok(Some(_)) if false => cond,', WFT),
    ('a status function is matched case-sensitively', WE,
     '        let name = id.lexeme.to_lowercase();', '        let name = id.lexeme.clone();', WFT),
    ('repeated step ids get no suffix', W,
     '            let suffix = if attempt == 1 { String::new() } else { format!("_{attempt}") };',
     '            let suffix = String::new();', WFT),
    ('a local action\'s step id is its path', W,
     '        return "self".into();', '        return uses.into();', WFT),
    ('an embedded expression keeps single quotes unescaped', WT,
     '''Seg::Lit(l) => fmt.push_str(&l.replace('\\'', "''").replace('{', "{{").replace('}', "}}")),''',
     '''Seg::Lit(l) => fmt.push_str(&l.replace('{', "{{").replace('}', "}}")),''', WFT),
    ('an all-literal expression stays an expression', WT,
     '            [Seg::Expr(e)] => match literal_string(e) {', '            [Seg::Expr(e)] => match None::<String> {', WFT),
    ('YAML 1.1 octal 012 reads as 10', WT,
     '        return T::Num(s.parse::<f64>().unwrap_or(f64::NAN));\n    }\n    // [-+]?',
     '        return T::Num(if s.starts_with(\'0\') && s.len() > 1 { 10.0 } else { s.parse::<f64>().unwrap_or(f64::NAN) });\n    }\n    // [-+]?',
     WFT),
    ('a duplicate key is accepted', WT,
     '                if seen.contains(&up) {\n                    self.errs.at(&key, format!("\'{name}\' is already defined"));\n                    continue;\n                }\n                seen.push(up);\n            }\n            let prop',
     '                seen.push(up);\n            }\n            let prop', WFT),
    ('a missing required property passes', WT,
     '                if *required && !seen.contains(&p.to_uppercase()) {', '                if false && *required {', WFT),
    ('a needs cycle is not reported', W,
     '    for w in &waiting {\n        for n in &w.needs {', '    for w in waiting.iter().take(0) {\n        for n in &w.needs {', WFT),
    ('a workflow with only dependent jobs passes', W,
     '    if ready.is_empty() {\n        cx.errs.at(jobs_node, "The workflow must contain at least one job with no dependencies.");',
     '    if ready.is_empty() && false {\n        cx.errs.at(jobs_node, "The workflow must contain at least one job with no dependencies.");', WFT),
    ('an unknown named-value is allowed', WE,
     '            if !self.allowed.named.contains(&name) {', '            if false {', WFT),
    ('an alias reads as null', WT,
     '                let n = self.anchors.get(&id).cloned().unwrap_or(Node { t: T::Null, line, col });',
     '                let n = Node { t: T::Null, line, col };', WFT),
    ('a job name falls back to its id only when absent', W,
     '        && n.str() != Some("")', '        && true', WFT),
    # ---- the matrix builder and the event filter, against GitHub's documented examples ----
    ('the first vector varies fastest', WM,
     '        for (name, vals) in &vectors {\n            block /= vals.len();',
     '        for (name, vals) in vectors.iter().rev() {\n            block /= vals.len();', ['-p', 'graftyard-actions', '--lib']),
    ('an include overwrites a vector value', WM,
     '            if vector_named(&key) {', '            if false {', ['-p', 'graftyard-actions', '--lib']),
    ('an unmatched include is dropped', WM,
     '        if !matched[j] {', '        if false {', ['-p', 'graftyard-actions', '--lib']),
    ('exclude compares case-sensitively', WM,
     '    filter.iter().all(|(p, want)| expr::equals(&lookup(combo, p), want))',
     '    filter.iter().all(|(p, want)| lookup(combo, p) == *want)', ['-p', 'graftyard-actions', '--lib']),
    ('ids collide across case', WM,
     '        if seen.insert(candidate.to_lowercase()) {', '        if seen.insert(candidate.clone()) {', ['-p', 'graftyard-actions', '--lib']),
    ('a star matches across slashes', WF,
     '            let n = s.iter().take_while(|c| **c != \'/\').count();', '            let n = s.len();',
     ['-p', 'graftyard-actions', '--lib']),
    ('the first matching pattern decides', WF,
     '            if p.matches(s) {\n                m = !p.negative;\n            }',
     '            if p.matches(s) {\n                m = !p.negative;\n                break;\n            }', ['-p', 'graftyard-actions', '--lib']),
    ('paths-ignore skips when any path is ignored', WF,
     '    !inputs.is_empty() && inputs.iter().all(|s| ps.iter().any(|p| p.matches(s) != p.negative))',
     '    !inputs.is_empty() && inputs.iter().any(|s| ps.iter().any(|p| p.matches(s) != p.negative))',
     ['-p', 'graftyard-actions', '--lib']),
    ('a pull request runs on every activity type', WF,
     '        None => defaults.contains(&action),', '        None => true,', ['-p', 'graftyard-actions', '--lib']),
]

env = dict(os.environ, PATH=os.path.expanduser('~/.cargo/bin:') + os.environ['PATH'],
           CARGO_TARGET_DIR=os.environ.get('CARGO_TARGET_DIR', os.path.join(ROOT, '..', 'target-m4a')),
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
