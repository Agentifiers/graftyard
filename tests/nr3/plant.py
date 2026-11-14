#!/usr/bin/env python3
"""narrow-reads-3: planted bugs in the follower-read floor (`crates/raft`: `ReadGrant::floor`, `Member::fresh_after`),
the member's narrow answer cache keyed by it (`workers/site/src/family.rs`, `host.rs`), the edge's genesis binding
(`crates/lookup`), a learner's entries ahead of its read-index answer, F67 rule 12, read-index batches in flight (`crates/raft`), and the edge's herding guard (`crates/lookup`). Plants one mutant at a time,
runs the tests that gate it, and restores the file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE.
The host's wiring of a waited read (`Host::answer_after` passing the grant's floor) runs only in a Durable Object; it
is covered hosted by `tests/nr3/fresh.mjs`. Run from the worktree root:

    python3 tests/nr3/plant.py          # every mutant
    python3 tests/nr3/plant.py 0 3      # by number
    python3 tests/nr3/plant.py --check  # residue only: exit 1 if a mutant is left in the tree (the pre-commit hook)

A mutant reached a commit twice on 2026-11-14 when a run was killed mid-mutant, which no `finally` survives. So the
original bytes go to a journal (`tests/nr3/raw/.plant-journal`) before each mutation and leave it only once restored;
a run, and `--check`, first restores any journal a killed run left. SIGTERM and SIGINT restore and exit. After the
run, and in `--check`, every mutant's original text must be in its file exactly once and `git diff` of the planted
files must be what it was before the run; otherwise the runner exits 1 and names the residue.
"""
import json, os, signal, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
INSTALL = 'crates/raft/src/member/install.rs'
LEADER = 'crates/raft/src/member/leader.rs'
FAM = 'workers/site/src/family.rs'
HOST = 'workers/site/src/host.rs'
LOOKUP = 'crates/lookup/src/lib.rs'
ELECTION = 'crates/raft/src/member/election.rs'
RAFT = ('graftyard-raft', '--test', 'leadership')
LK = ('graftyard-lookup', '--lib', 'tests')
SITE = ('graftyard-site', '--lib', 'tests')
SCEN = ('graftyard-raft', '--test', 'scenarios')

MUTANTS = [
    ('raft: a follower grant\'s floor is its own applied index, not the leader\'s read index', INSTALL,
     '                let g = grant(self, i);', '                let g = grant(self, applied.min(i).min(Index(0)));', RAFT),
    ('raft: fresh_after answers after a restart', LEADER,
     '        if grant.boot != self.boot {\n            return Err(self.not_leader());\n        }\n        Ok(Fresh { fold: &self.fold.fold, index: self.fold.applied() })',
     '        Ok(Fresh { fold: &self.fold.fold, index: self.fold.applied() })', RAFT),
    ('raft: fresh_after answers at the grant\'s index, not the state now', LEADER,
     '        Ok(Fresh { fold: &self.fold.fold, index: self.fold.applied() })',
     '        Ok(Fresh { fold: &self.fold.fold, index: grant.index })', RAFT),
    ('cache: any cached answer serves, below the read\'s floor too', FAM,
     'v.iter().find(|(t, at, l, _)| t == text && *at >= floor && *l == loaded)',
     'v.iter().find(|(t, _, l, _)| t == text && *l == loaded)', SITE),
    ('cache: a floor one above the answer is still served (off by one)', FAM,
     'v.iter().find(|(t, at, l, _)| t == text && *at >= floor && *l == loaded)',
     'v.iter().find(|(t, at, l, _)| t == text && *at + 1 >= floor && *l == loaded)', SITE),
    ('cache: an answer built while the metadata tier loaded serves once it is loaded', FAM,
     'v.iter().find(|(t, at, l, _)| t == text && *at >= floor && *l == loaded)',
     'v.iter().find(|(t, at, _, _)| t == text && *at >= floor)', SITE),
    ('cache: another query\'s answer serves', FAM,
     'v.iter().find(|(t, at, l, _)| t == text && *at >= floor && *l == loaded)',
     'v.iter().find(|(_, at, l, _)| *at >= floor && *l == loaded)', SITE),
    ('cache: the answer is stamped with the floor, not the index it was built at', FAM,
     '    let at = graftyard_fold::meta(&*fold.core).applied.0;\n    let bytes',
     '    let at = floor.max(graftyard_fold::meta(&*fold.core).applied.0 + 1);\n    let bytes', SITE),
    ('spacing: a read whose floor the cache covers waits for a build', FAM,
     '    if narrow_hit(repo, &text, floor, fold.meta.loaded()).is_some() {\n        return None;\n    }', '', SITE),
    ('spacing: never spaced', FAM,
     '            Some(at) if now < at + NARROW_EVERY_MS => Some(at + NARROW_EVERY_MS - now),', '', SITE),
    ('shared: answers in one input are shared across floors', HOST,
     '    let key = format!("{floor} {q}");', '    let key = format!("{} {q}", floor * 0);', SITE),
    # Genesis on the edge (`crates/lookup`: `bind_genesis`, `settle_bound`).
    ('genesis: the binding is taken without accept_genesis (any install, any signer)', LOOKUP,
     '        graftyard_release::accept_genesis(anchor, &b)\n            .map_err(|e| format!("GRAFTYARD_GENESIS_SIGNED: {}: {e}", e.variant()))',
     '        graftyard_release::SignedEntry::decode(&b).map_err(|e| e.to_string()).and_then(|e| match e.body { graftyard_release::ReleaseEntry::Genesis(g) => Ok(*g), _ => Err("not a genesis".into()) }).map(|g| { let _ = anchor; g })', LK),
    ('genesis: a refused binding is tried again on the next request', LOOKUP,
     '        Err(e) => {\n            BOUND_OFF.with(|o| o.set(true));\n            Err(e)\n        }',
     '        Err(e) => Err(e),', LK),
    ('genesis: a different view never replaces the binding', LOOKUP,
     '    BOUND_OFF.with(|o| o.set(true));\n    GENESIS.with(|c| *c.borrow_mut() = Some(Held::View(view)));\n    true',
     '    let _ = view;\n    false', LK),
    ('genesis: an unreadable view drops the binding', LOOKUP,
     '    let Some(view) = view else { return false };',
     '    let Some(view) = view else {\n        GENESIS.with(|c| *c.borrow_mut() = None);\n        return true;\n    };', LK),
    # The edge's herding guard (`crates/lookup`: `timed_spread_order`, `Timed`).
    ('guard: no spread, only the best within the margin share the reads (gate 15\'s herding)', LOOKUP,
     '    near.extend(far.drain(..pad.min(ROUTE_SPREAD.saturating_sub(near.len()))));', '', LK),
    ('guard: a learner scored past ROUTE_LATE is padded in too', LOOKUP,
     '    let pad = far.iter().take_while(|l| l.2.is_some_and(|s| s < late)).count();',
     '    let pad = far.len();', LK),
    ('guard: the score is the average alone, with no deviation', LOOKUP,
     'then_some(self.mean + ROUTE_DEV * self.dev)', 'then_some(self.mean)', LK),
    ('guard: a learner untimed for ROUTE_STALE keeps its old score', LOOKUP,
     '        (now.saturating_sub(self.at) < ROUTE_STALE.as_millis() as u64).then_some(',
     '        (now >= self.at || true).then_some(', LK),
    ('guard: an untimed learner counts as the best, which shuts out the timed ones', LOOKUP,
     '    let best = learners.iter().filter_map(|l| l.2).fold(f64::INFINITY, f64::min);',
     '    let best = learners.iter().map(|l| l.2.unwrap_or(0.0)).fold(f64::INFINITY, f64::min);', LK),
    ('guard: padded to one more than ROUTE_SPREAD (off by one)', LOOKUP,
     'pad.min(ROUTE_SPREAD.saturating_sub(near.len()))', 'pad.min((ROUTE_SPREAD + 1).saturating_sub(near.len()))', LK),
    # A learner's entries ride ahead of its read-index answer (`Member::append_for_read`).
    ('read entries: none sent before the answer', INSTALL,
     '                if let Some((i, _)) = index {\n                    self.append_for_read(to, i);\n                }\n', '', SCEN),
    ('read entries: nothing after a refusal emptied the pipeline (f1ab1a61)', LEADER,
     '                _ if lead.matched(peer) < index => self.send_append(peer, true),\n', '', SCEN),
    ('read entries: re-sent from the acknowledged entry on every read (c95216cc)', LEADER,
     'if through < index && n < PIPELINE => self.append_to(peer, false, true),',
     'if through < index && n < PIPELINE => self.send_append(peer, true),', SCEN),
    ('read entries: none while a call is out', LEADER,
     'if through < index && n < PIPELINE => self.append_to(peer, false, true),',
     'if through < index && n < PIPELINE => return,', SCEN),
    ('read entries: sent only when two or more are missing (off by one)', LEADER,
     'if through < index && n < PIPELINE => self.append_to(peer, false, true),',
     'if through.next() < index && n < PIPELINE => self.append_to(peer, false, true),', SCEN),
    ('read entries: a lossy learner gets them too', LEADER,
     '        if quorum::is_voter(self.log_config(), peer) || self.link_lossy(peer) {\n            return;\n        }\n        // Up to',
     '        if quorum::is_voter(self.log_config(), peer) {\n            return;\n        }\n        // Up to', SCEN),
    ('read entries: one call per read, not up to PIPELINE (617ed5b9)', LEADER,
     '        for _ in 0..PIPELINE {\n            let Some(lead) = self.lead.as_ref() else { return };\n            let next = lead.next',
     '        for _ in 0..1 {\n            let Some(lead) = self.lead.as_ref() else { return };\n            let next = lead.next', SCEN),
    # Liveness (F67 rule 12).
    ('raft: an emptied read-index batch stays in flight', INSTALL,
     '        r.inflight.retain(|(_, _, reads)| !reads.is_empty());', '', RAFT),
    # Read-index batches in flight (`READ_PIPELINE`).
    ('pipeline: an answer grants every batch in flight, not its own', INSTALL,
     '        let (_, _, reads) = self.remote.inflight.remove(at);',
     '        let reads: Arrived = self.remote.inflight.drain(at..).flat_map(|b| b.2).collect();', RAFT),
    ('pipeline: an answer grants the oldest batch, whichever it answers', INSTALL,
     '        let Some(at) = self.remote.inflight.iter().position(|b| b.0 == batch) else { return };',
     '        let Some(at) = self.remote.inflight.iter().position(|b| b.0 <= batch) else { return };', RAFT),
    ('pipeline: reads queued since a batch left join it in flight', INSTALL,
     '        let reads = std::mem::take(&mut r.queued);\n        r.inflight.push((batch, self.now, reads));',
     '        let reads = std::mem::take(&mut r.queued);\n        match r.inflight.last_mut() {\n            Some(b) => b.2.extend(reads),\n            None => r.inflight.push((batch, self.now, reads)),\n        }', RAFT),
    ('pipeline: one batch in flight, as before', INSTALL,
     '        if r.inflight.len() >= READ_PIPELINE', '        if !r.inflight.is_empty()', RAFT),
]

ENV = dict(os.environ, CARGO_INCREMENTAL='0')
JOURNAL = os.path.join(ROOT, 'tests', 'nr3', 'raw', '.plant-journal')


def recover():
    """Restores the file a killed run left mutated, from its journal."""
    if not os.path.exists(JOURNAL):
        return
    j = json.load(open(JOURNAL))
    open(os.path.join(ROOT, j['file']), 'wb').write(bytes.fromhex(j['orig']))
    os.remove(JOURNAL)
    print(f'restored {j["file"]} from a killed run\'s journal', flush=True)


def planted_diff():
    files = sorted({m[1] for m in MUTANTS})
    return subprocess.run(['git', 'diff', '--', *files], cwd=ROOT, capture_output=True, text=True).stdout


def residue():
    """Mutants left in the tree: a mutant's original text missing from its file, or not exactly once."""
    out = []
    for i, (name, f, old, new, t) in enumerate(MUTANTS):
        if open(os.path.join(ROOT, f)).read().count(old) != 1:
            out.append(f'{i:2} {name} ({f})')
    return out


def stop(signum, frame):
    raise KeyboardInterrupt(f'signal {signum}')


def run(t):
    pkg, kind, name = t
    args = ['cargo', 'test', '-q', '-p', pkg, kind, name] if kind == '--test' else ['cargo', 'test', '-q', '-p', pkg, '--lib', name]
    r = subprocess.run(args, cwd=ROOT, env=ENV, capture_output=True, text=True, timeout=1800)
    out = r.stdout + r.stderr
    return r.returncode, out, 'error[E' in out or 'could not compile' in out


def main():
    recover()
    if sys.argv[1:] == ['--check']:
        left = residue()
        for r in left:
            print(f'RESIDUE {r}')
        sys.exit(1 if left else 0)
    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    before = planted_diff()
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    caught = 0
    try:
        for i in pick:
            name, f, old, new, t = MUTANTS[i]
            p = os.path.join(ROOT, f)
            orig = open(p, 'rb').read()
            s = orig.decode()
            if s.count(old) != 1:
                print(f'{i:2} NOT FOUND  {name}', flush=True)
                continue
            with open(JOURNAL + '.tmp', 'w') as j:
                json.dump({'file': f, 'orig': orig.hex()}, j)
            os.replace(JOURNAL + '.tmp', JOURNAL)
            try:
                open(p, 'w').write(s.replace(old, new))
                code, out, broke = run(t)
            finally:
                open(p, 'wb').write(orig)
                os.remove(JOURNAL)
            if broke:
                print(f'{i:2} DID NOT COMPILE  {name}', flush=True)
            elif code != 0:
                caught += 1
                fails = [l.split()[1] for l in out.splitlines() if l.startswith('---- ')]
                print(f'{i:2} CAUGHT  {name}  [{", ".join(fails[:3])}]', flush=True)
            else:
                print(f'{i:2} MISSED  {name}', flush=True)
    finally:
        left, after = residue(), planted_diff()
        for r in left:
            print(f'RESIDUE {r}', flush=True)
        if after != before:
            print('RESIDUE: git diff of the planted files changed during the run', flush=True)
        if left or after != before:
            sys.exit(1)
    print(f'{caught} of {len(pick)} caught')


if __name__ == '__main__':
    main()
