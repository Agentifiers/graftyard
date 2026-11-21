#!/usr/bin/env python3
"""human-join: planted bugs in the late join of Human and MergeQueue commands into the newest frozen batch
(`crates/raft/src/member/leader.rs`, `late_join` and `decide_held`; `decisions/DECISIONS.md`, "`human-join`"). Plants
one mutant at a time, runs `crates/raft/tests/deps.rs`, and restores the file byte for byte. Each line prints CAUGHT,
MISSED, or DID NOT COMPILE. Mutant 0 is main's rule: nothing joins. Run from the repository root:

    python3 tests/m0carried/plant78.py          # every mutant
    python3 tests/m0carried/plant78.py 0 3      # by number
"""
import os
import signal
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
L = 'crates/raft/src/member/leader.rs'
T = ['cargo', 'test', '-q', '--release', '-p', 'graftyard-raft', '--test', 'deps']

MUTANTS = [
    ("main's rule: no command joins a frozen batch", L,
     '        if !matches!(class, AdmitClass::Human | AdmitClass::MergeQueue) || self.fold.request(q.id).is_some() {',
     '        if true || self.fold.request(q.id).is_some() {'),
    ("a joiner asks no read: it rides on the batch's", L,
     '        let ask = !chains.is_empty() && d.join_ask.is_none();', '        let ask = false;'),
    ("a joiner is decided on a read asked before its admission", L,
     '|c: &ChainId| j.read.is_some_and(|r| join_deps.iter().any(|(k, id, _)| k == c && *id >= r));',
     '|c: &ChainId| join_deps.iter().any(|(k, _, _)| k == c);'),
    ("a joiner that gains a chain at apply is decided without a read of it", L,
     '|c: &ChainId| j.read.is_some_and(|r| join_deps.iter().any(|(k, id, _)| k == c && *id >= r));',
     '|_: &ChainId| j.read.is_some_and(|r| join_deps.iter().any(|(_, id, _)| *id >= r));'),
    ("the entry passes the oldest read of a chain, not the newest", L,
     '.max_by_key(|i| have[*i].1);', '.min_by_key(|i| have[*i].1);'),
    ("an AgentCmds-capped batch takes no joiner", L,
     '            capped: matches!(end, CutEnd::Soft | CutEnd::EntryMax),',
     '            capped: matches!(end, CutEnd::Soft | CutEnd::EntryMax | CutEnd::AgentCmds),'),
    ("a Soft-capped batch takes a joiner", L,
     '            capped: matches!(end, CutEnd::Soft | CutEnd::EntryMax),', '            capped: matches!(end, CutEnd::EntryMax),'),
    ("agent commands join too", L,
     '        if !matches!(class, AdmitClass::Human | AdmitClass::MergeQueue) || self.fold.request(q.id).is_some() {',
     '        if self.fold.request(q.id).is_some() {'),
    ("a joiner joins the oldest frozen batch", L,
     '        let Some(d) = lead.deciding.back_mut() else { return Some(q) };',
     '        let Some(d) = lead.deciding.front_mut() else { return Some(q) };'),
    ("a joiner joins while the leader would still freeze another batch", L,
     '        if lead.pending.is_none() || lead.deciding.len() < ad.deps_ms.map_or(1, |d| read_ahead(ad.round_ms, d)) {',
     '        if lead.pending.is_none() || lead.deciding.is_empty() {'),
    ("a joiner left out of its batch is dropped, not queued again", L,
     '            lanes[j.class.lane()].push_front(j.actor, j.q);', '            let _ = (lanes.len(), j.q.seq);'),
    ("every joiner asks its own read, even while one is out", L,
     '        let ask = !chains.is_empty() && d.join_ask.is_none();', '        let ask = !chains.is_empty();'),
    ("a joiner joins a batch frozen with no entry in flight", L,
     '        if lead.pending.is_none() || lead.deciding.len() <', '        if lead.deciding.len() <'),
]


def main():
    # A stop ends in SystemExit, so `finally` restores the file (plant75's lesson, `sync-ab.md`, "The plant left in").
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))
    pick = [int(a) for a in sys.argv[1:]] or range(len(MUTANTS))
    caught = 0
    for i in pick:
        name, f, old, new = MUTANTS[i]
        p = os.path.join(ROOT, f)
        orig = open(p, 'rb').read()
        text = orig.decode()
        if text.count(old) != 1:
            print(f'{i} {name}: anchor found {text.count(old)} times')
            sys.exit(2)
        try:
            open(p, 'w').write(text.replace(old, new))
            r = subprocess.run(T, cwd=ROOT, capture_output=True, text=True, timeout=1800)
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
