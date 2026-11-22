#!/usr/bin/env python3
"""M5-WP4: planted bugs in the site-day gates and automatic drain (`crates/release/src/gate.rs`) and in how
`graft-ops` acts on them (`crates/release/src/ops.rs`; `decisions/DECISIONS.md`, "M5-WP4: site days and automatic
drain, as built"). Plants one mutant at a time, runs the release crate's tests, and restores the file byte for byte.
Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant87.py          # every mutant
    python3 tests/m0carried/plant87.py 0 3      # by number
"""
import os
import signal
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
GATE = 'crates/release/src/gate.rs'
OPS = 'crates/release/src/ops.rs'
GROUP = 'crates/release/src/group.rs'
REL = ['cargo', 'test', '-q', '-p', 'graftyard-release', '--lib']

MUTANTS = [
    ("apply lag is judged against N alone, not the 500 ms cap", GATE,
     'if n1.apply_lag_p99_ms > GATE_APPLY_LAG_MS || n1.apply_lag_p99_ms > 2 * n.apply_lag_p99_ms {',
     'if n1.apply_lag_p99_ms > 2 * n.apply_lag_p99_ms {', REL),
    ("apply lag may be three times N's", GATE,
     'n1.apply_lag_p99_ms > 2 * n.apply_lag_p99_ms {', 'n1.apply_lag_p99_ms > 3 * n.apply_lag_p99_ms {', REL),
    ("an ack rate exactly 0.1 points under N breaches", GATE,
     'if n1.ack_bp + GATE_ACK_RATE_BP < n.ack_bp {', 'if n1.ack_bp + GATE_ACK_RATE_BP <= n.ack_bp {', REL),
    ("STEP_DOWN trips on N+1 do not breach", GATE,
     '    if n1.step_downs > 0 {', '    if n1.step_downs > 1 {', REL),
    ("a starved step passes", GATE,
     '        return Gate::Starved(n1.entries);', '        return Gate::Pass;', REL),
    ("too few entries hides a breach", GATE,
     '    if n1.exceptions > 0 {\n        return Gate::Breach',
     '    if n1.entries < STEP_MIN_ENTRIES {\n        return Gate::Starved(n1.entries);\n    }\n    if n1.exceptions > 0 {\n        return Gate::Breach', REL),
    ("GATE_ERRORS breaches at exactly N plus 0.1 points", GATE,
     '    if n1_bp > n_bp + GATE_ERRORS_BP {', '    if n1_bp >= n_bp + GATE_ERRORS_BP {', REL),
    ("one other site's report drains a site", GATE,
     'voters.iter().filter(|o| **o != t).all(|o| reports', 'voters.iter().filter(|o| **o != t).any(|o| reports', REL),
    ("a site's report about itself counts", GATE,
     'voters.iter().filter(|o| **o != t).all(', 'voters.iter().all(', REL),
    ("a report under MIN_CALLS counts", GATE,
     '        self.calls >= MIN_CALLS', '        self.calls > 0', REL),
    ("a burn of exactly 1 x counts as calm", GATE,
     'self.evidence() && self.missed * ALLOWED_MISS_DENOM < x * self.calls',
     'self.evidence() && self.missed * ALLOWED_MISS_DENOM <= x * self.calls', REL),
    ("the health window drops its first second", GATE,
     '.filter(|((s, t), _)| *s >= from && *t == target.0)', '.filter(|((s, t), _)| *s > from && *t == target.0)', REL),
    ("graft-ops never drains a burning site", OPS,
     '        None => mode(*sick, crate::types::Mode::Drain),',
     '        None => return None,', REL),
    ("an automatic drain does not undrain the rolling site first", OPS,
     '        Some((s, d)) if d.by_drain_key && day == Some(SiteId(*s)) => mode(*s, crate::types::Mode::Normal),\n',
     '', REL),
    ("the site day's own drain ends when its site is calm", OPS,
     '.find(|(s, d)| d.by_drain_key && day != Some(SiteId(**s)) && verdict(s).calm && !verdict(s).burning)',
     '.find(|(s, d)| d.by_drain_key && verdict(s).calm && !verdict(s).burning)', REL),
    ("a drained site is undrained while it is merely not burning", OPS,
     'verdict(s).calm && !verdict(s).burning)', '!verdict(s).burning)', REL),
    ("a site day deploys while another site is drained", OPS,
     '            if *gated\n                && let Script::Site(s) = script\n                && let Some(d) = other_drained(*s)\n            {',
     '            if false\n                && let Script::Site(s) = script\n                && let Some(d) = other_drained(*s)\n            {', REL),
    ("a site day drains its site while another is drained", OPS,
     '        Step::Drain(s) if let Some(d) = other_drained(*s) => {',
     '        Step::Drain(s) if let Some(d) = other_drained(*s).filter(|_| false) => {', REL),
    ("STUCK_DAY never undrains", OPS,
     '                && w.now.0 >= from.0.saturating_add(STUCK_DAY) =>',
     '                && false =>', REL),
    ("STUCK_DAY undrains a minute early", OPS,
     'w.now.0 >= from.0.saturating_add(STUCK_DAY) =>', 'w.now.0 + 60_000 >= from.0.saturating_add(STUCK_DAY) =>', REL),
    ("a starved step waits forever instead of paging", OPS,
     '                    if w.now < cap {\n                        return Action::Wait { until: Some(cap), why: "STEP_MIN_ENTRIES".into() };',
     '                    if true {\n                        return Action::Wait { until: Some(cap), why: "STEP_MIN_ENTRIES".into() };', REL),
    ("a starved step pages at once", OPS,
     'let cap = Timestamp(start.0.saturating_add(2 * *dwell));', 'let cap = start;', REL),
    ("a ContainerDay's breach does not run its rollback", OPS,
     '        Step::Shards { pct, dwell } => {\n            if let Gate::Breach(why) = &w.gate {\n                return breach(fold, c, why);\n            }',
     '        Step::Shards { pct, dwell } => {', REL),
    ("a ContainerDay rolls 10% and then 100%", GROUP,
     '        Intent::ContainerDay { .. } => Some(&[10, 50, 100]),', '        Intent::ContainerDay { .. } => Some(&[10, 100]),', REL),
    ("a rolled-back ContainerDay leaves shards on the new image", GATE,
     '            IntentState::Undoing | IntentState::Aborting { .. } => None,',
     '            IntentState::Undoing | IntentState::Aborting { .. } => Some(c.next.min(2)),', REL),
    ("a site whose script moved on keeps its shards on the old image as git", GATE,
     '    (fold.site_release.get(&site.0) == Some(rolled)).then_some(2)', '    Some(2).filter(|_| !rolled.0.is_empty())', REL),
    ("the lead limit holds every chain off the site, not 99%", GATE,
     '        && h % 100 != 0\n', '', REL),
    ("the lead limit starts before the day drains its site", GATE,
     ') if c.next >= 1 && !fold.drained.contains_key(&s.0) => Some(*s),', ') if !fold.drained.contains_key(&s.0) => Some(*s),', REL),
    ("a restart redoes a committed step", OPS,
     '    let k = usize::from(c.next);', '    let k = usize::from(c.next.saturating_sub(u8::from(c.next % 3 == 2)));', REL),
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
