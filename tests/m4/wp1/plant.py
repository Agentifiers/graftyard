#!/usr/bin/env python3
"""M4-WP1's planted bugs: plants one mutant at a time, runs the tests that gate it, and restores the file byte for
byte. Each line prints CAUGHT with the failing tests, MISSED, or DID NOT COMPILE. Run from the worktree root:

    python3 tests/m4/wp1/plant.py          # every mutant
    python3 tests/m4/wp1/plant.py 0 3      # by number

`$CARGO_TARGET_DIR` defaults to `target/plants`.
"""
import os, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..', '..'))
S = 'crates/git/src/sequencer.rs'
H = 'crates/git/src/hooks.rs'
I = 'crates/git/src/issue_ops.rs'
T = 'crates/git/src/tiers.rs'
W = 'crates/api/src/webhooks.rs'
K = 'crates/identity/src/sealed.rs'
# One cargo invocation per package list: `--lib` may appear once, and applies to every `-p` named.
GIT = ['-p', 'graftyard-git', '--test', 'hooks_gates', '--test', 'issues', '--test', 'agent_gates', '--lib']
API = ['-p', 'graftyard-api', '--test', 'webhooks', '--lib']
BOTH = ['-p', 'graftyard-git', '-p', 'graftyard-api', '--test', 'hooks_gates', '--test', 'issues', '--test',
        'agent_gates', '--test', 'webhooks', '--lib']
ID = ['-p', 'graftyard-identity', '-p', 'graftyard-api', '--test', 'webhooks', '--lib']

# (name, file, old, new, test args)
MUTANTS = [
    # ---- the sequencer ----
    ('two attempts in flight per queue (order lost)', S,
     '            if q.running.is_some() || now < q.not_before {',
     '            if now < q.not_before {', BOTH),
    ('failing hooks not held to half the slots', S,
     '            if is_failing && failing >= cap / 2 {', '            if is_failing && failing >= cap {', GIT),
    ('no owner-wide cap (HOOK_OWNER_CONCURRENCY ignored)', S,
     '            if total >= cap {', '            if total >= cap * 4 {', GIT),
    ('record is not idempotent by GUID', S,
     '        if self.deliveries.contains_key(&r.guid) {\n            return;\n        }', '', BOTH),
    ('replay_from ignores unfinished deliveries', S,
     '            Some(((c, i), _)) if *c == chain => *i,', '            Some(((c, _), _)) if *c == chain && false => Index(0),',
     GIT),
    ('a rebuilt store starts past the committed read (replay_from ignored)', S,
     '        None => from_replay,\n    }\n}',
     '        None => cursor.read,\n    }\n}', GIT),
    ('an unreported attempt is never re-sent', S,
     '            if q.running.is_some_and(|r| now.0 >= r.started.0 + RESEND_AFTER_MS) {',
     '            if q.running.is_some_and(|r| now.0 >= r.started.0 + RESEND_AFTER_MS * 1_000_000) {', BOTH),
    ('a default hook is retried (retry opt-in ignored)', S,
     '        let again = outcome != Outcome::Ok && retry && !manual && now.0 < first.0 + DELIVERY_KEEP_MS;',
     '        let again = outcome != Outcome::Ok && !manual && now.0 < first.0 + DELIVERY_KEEP_MS;', GIT),
    ('a retry hook is never retried', S,
     '        let again = outcome != Outcome::Ok && retry && !manual && now.0 < first.0 + DELIVERY_KEEP_MS;',
     '        let again = false && retry;', GIT),
    ('redelivery past its 3 days', S,
     '        if now >= d.expires {\n            return Err(RedeliverErr::Expired);\n        }', '', BOTH),
    ('redelivery of another hook\'s delivery', S,
     '        if d.hook != hook || !d.attempts.iter().any(|a| a.id == id) {',
     '        if !d.attempts.iter().any(|a| a.id == id) {', GIT),
    ('a rebuilt store reuses the lost store\'s attempt ids', S,
     '            next_attempt: now.0.saturating_mul(1_000).max(1),', '            next_attempt: 1,', BOTH),
    ('round robin restarts at the first queue', S,
     '                after.chain(self.queues.range(..=l)).map(|(k, _)| *k).collect()',
     '                let _ = after;\n                self.queues.keys().copied().collect()', BOTH),
    # ---- the consumer's planner and cursor ----
    ('a hook gets entries from before it existed', S,
     '        .filter(|(id, _)| crate::hooks::hook_row(core, *id).is_some_and(|r| r.created <= e.at))',
     '        .filter(|_| true)', BOTH),
    ('a ping goes to every hook', S,
     '                Some(t) => t == *id && r.active,', '                Some(_) => r.active,', BOTH),
    ('the cursor commits only on the stride (read passes nothing it recorded)', S,
     '    (want != committed && (recorded_rows || replay_moved || stride)).then_some(want)',
     '    (want != committed && stride && (recorded_rows || replay_moved || true)).then_some(want)', GIT),
    ('HookCursorCmd moves read backward', H,
     '        let read = cur.read.max(self.cursor.read);', '        let read = self.cursor.read;', GIT),
    ('HookCursorCmd lets replay_from pass read + 1', H,
     '        let replay_from = cur.replay_from.max(self.cursor.replay_from.min(Index(read.0 + 1)));',
     '        let replay_from = cur.replay_from.max(self.cursor.replay_from);', GIT),
    ('the GUID ignores the hook', H, '        .chain_update(hook.0.0.to_be_bytes())', '', BOTH),
    ('the GUID ignores the slot', H, '        .chain_update(slot.to_be_bytes())', '', BOTH),
    # ---- what an entry carries ----
    ('a queue ref sends push', H,
     '                for u in updates.iter().filter(|u| tag_or_branch(&u.name) && !is_queue_ref(&u.name)) {',
     '                for u in updates.iter().filter(|u| tag_or_branch(&u.name)) {', API),
    ('an edit pings (created ignored)', H,
     '            RepoEvent::Hook { id, record: Some(r), created: true, .. } if r.active => out.push(Webhook {',
     '            RepoEvent::Hook { id, record: Some(r), .. } if r.active => out.push(Webhook {', BOTH),
    ('no create event for a new branch', H,
     '                        (None, Some(_)) => out.push(w("create", None, r)),', '                        (None, Some(_)) => {}',
     API),
    # ---- decide ----
    ('a writer may edit hooks', I,
     '            IssueOp::PutHook { id, record } => {\n                need(RepoPermission::Admin)?;',
     '            IssueOp::PutHook { id, record } => {\n                need(RepoPermission::Write)?;', GIT),
    ('an agent may edit hooks', I,
     '        IssueOp::PutHook { .. } | IssueOp::DeleteHook { .. } => {\n            Some(AgentRefusal::new(AgentRule::Rules, "refused: agents may not edit webhooks"))\n        }',
     '', GIT),
    ('two hooks on one URL', I,
     '                        if existing.iter().any(|(_, h)| h.url == record.url) {',
     '                        if existing.iter().any(|(_, h)| h.url == record.url) && false {', BOTH),
    ('an identical edit commits an entry', I,
     '                        if old == *record {', '                        if old == *record && false {', BOTH),
    ('a create\'s id does not advance the id cursor', T,
     '                if record.is_some() {\n                    assigned(w, ev, Tier::Core, id.0);\n                }', '', BOTH),
    # ---- rendering and signing ----
    ('the secret is rendered in the hook object', W,
     '        config["secret"] = json!("********");', '        config["secret"] = json!(format!("{:?}", h.secret));',
     API),
    ('no X-Hub-Signature-256', W,
     '        headers.push(("x-hub-signature-256", signature_256(s, &body)));', '        let _ = s;', API),
    ('the signature is over the JSON, not the form body sent', W,
     '        headers.push(("x-hub-signature-256", signature_256(s, &body)));',
     '        headers.push(("x-hub-signature-256", signature_256(s, &d.body)));', API),
    ('the payload names the caller\'s permissions', W, '        m.remove("permissions");', '', API),
    ('edited carries no changes', W,
     '        if w.action == Some("edited") {', '        if w.action == Some("never") {', API),
    ('HMAC with the wrong inner pad', K,
     '    let inner = Sha256::new().chain_update(pad(0x36)).chain_update(body).finalize();',
     '    let inner = Sha256::new().chain_update(pad(0x5c)).chain_update(body).finalize();', ID),
]

env = dict(os.environ, PATH=os.path.expanduser('~/.cargo/bin:') + os.environ['PATH'],
           CARGO_TARGET_DIR=os.environ.get('CARGO_TARGET_DIR', os.path.join(ROOT, 'target', 'plants')),
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
        # A hang counts as caught (README, "Planted bugs").
        r = subprocess.run(['timeout', '-k', '10', '600', 'cargo', 'test', '-j', '3', *args, '--no-fail-fast'],
                           cwd=ROOT, env=env, capture_output=True, text=True)
        out = r.stdout + r.stderr
        failed = sorted({l.split()[1] for l in out.splitlines() if l.startswith('test ') and l.endswith('FAILED')})
        if ('error[' in out or out.lstrip().startswith('error:') or '\nerror: ' in out) and not failed:
            # A compile error, or a cargo usage error: never counted as caught.
            verdict = 'DID NOT COMPILE'
        else:
            verdict = 'CAUGHT' if r.returncode != 0 else 'MISSED'
            if r.returncode == 124:
                failed = ['(timed out)', *failed]
        print(f'{n:2} {verdict:8} {name}: {", ".join(failed)}', flush=True)
    finally:
        open(f, 'wb').write(orig)
    assert open(f, 'rb').read() == orig
