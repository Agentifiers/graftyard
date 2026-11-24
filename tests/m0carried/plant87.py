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
SHARD = 'crates/git/src/shard.rs'
GIT = ['cargo', 'test', '-q', '-p', 'graftyard-git', '--test', 'shard']
OPS_TOML = 'workers/ops/Cargo.toml'
EXEC = 'workers/ops/src/exec.rs'
OPSFAULTS = ['bash', 'tests/m5/opsfaults.sh']

MUTANTS = [
    # R40 (2026-11-23): apply lag is N+1's share over N's p99, with a 5 s backstop (DECISIONS "`gate-relative`").
    ("apply lag is judged by the backstop alone", GATE,
     '    if n.entries >= STEP_MIN_ENTRIES && tail > GATE_APPLY_LAG_SHARE_BP {', '    if false && tail > GATE_APPLY_LAG_SHARE_BP {', REL),
    ("exactly 2% of N+1 over N's p99 breaches", GATE,
     '    if n.entries >= STEP_MIN_ENTRIES && tail > GATE_APPLY_LAG_SHARE_BP {',
     '    if n.entries >= STEP_MIN_ENTRIES && tail >= GATE_APPLY_LAG_SHARE_BP {', REL),
    ("apply lag may put 3% over N's p99", GATE,
     'pub const GATE_APPLY_LAG_SHARE_BP: u64 = 200;', 'pub const GATE_APPLY_LAG_SHARE_BP: u64 = 300;', REL),
    ("apply lag is judged against N's p50 bucket, not its p99", GATE,
     'if *n > 0 && seen * 100 >= total * 99 {', 'if *n > 0 && seen * 100 >= total * 50 {', REL),
    ("the share over N's p99 counts N's own bucket", GATE,
     '.filter(|(_, e)| *e > ms)', '.filter(|(_, e)| *e >= ms)', REL),
    ("there is no backstop", GATE,
     '    if stalled > GATE_APPLY_LAG_CAP_BP {', '    if false {', REL),
    ("the backstop sits at 10 s", GATE,
     'pub const GATE_APPLY_LAG_CAP_MS: u64 = 5_000;', 'pub const GATE_APPLY_LAG_CAP_MS: u64 = 10_000;', REL),
    ("exactly 1% over the backstop breaches", GATE,
     '    if stalled > GATE_APPLY_LAG_CAP_BP {', '    if stalled >= GATE_APPLY_LAG_CAP_BP {', REL),
    # R43 (2026-11-23): the ack rate is judged from STEP_MIN_ENTRIES answers, on an excess past 0.1 points by 3.09 SE.
    ("the ack rate is judged under the answer floor", GATE,
     '    if n1.entries < STEP_MIN_ENTRIES || n1.appends < STEP_MIN_ENTRIES {',
     '    if n1.entries < STEP_MIN_ENTRIES {', REL),
    ("the ack rate has no margin", GATE,
     'pub const GATE_ACK_RATE_MARGIN: f64 = 0.001;', 'pub const GATE_ACK_RATE_MARGIN: f64 = 0.0;', REL),
    ("the ack rate breaches at z 2", GATE, 'pub const GATE_ACK_RATE_Z: f64 = 3.09;', 'pub const GATE_ACK_RATE_Z: f64 = 2.0;', REL),
    ("the ack rate breaches only at z 4", GATE, 'pub const GATE_ACK_RATE_Z: f64 = 3.09;', 'pub const GATE_ACK_RATE_Z: f64 = 4.0;', REL),
    ("the ack rate is never judged", GATE, '        && z >= GATE_ACK_RATE_Z\n', '        && z >= f64::INFINITY\n', REL),
    ("the ack rate's error counts N+1's side only", GATE,
     'let var = (p1 * (1.0 - p1) / c1 + p0 * (1.0 - p0) / c0).max(ACK_VAR_FLOOR);',
     'let var = (p1 * (1.0 - p1) / c1).max(ACK_VAR_FLOOR);', REL),
    ("STEP_DOWN trips on N+1 do not breach", GATE,
     '    if n1.step_downs > 0 {', '    if n1.step_downs > 1 {', REL),
    ("a starved step passes", GATE,
     '        return Gate::Starved(n1.entries.min(n1.appends));', '        return Gate::Pass;', REL),
    ("too few entries hides a breach", GATE,
     '    if n1.exceptions > 0 {\n        return Gate::Breach',
     '    if n1.entries < STEP_MIN_ENTRIES {\n        return Gate::Starved(n1.entries);\n    }\n    if n1.exceptions > 0 {\n        return Gate::Breach', REL),
    # R47 replaced the basis-point rule with a chance test (plant121 to plant124); the 0.1-point margin is what stays.
    ("GATE_ERRORS has no 0.1-point margin over N", GATE,
     ' + f64::from(GATE_ERRORS_BP) / 10_000.0).min(1.0);', ').min(1.0);', REL),
    # `verdict`'s rule, not `loss`'s, which shares its first line: the anchors run on to `verdict`'s `calm`.
    ("one other site's report drains a site", GATE,
     '.filter(|o| **o != t).all(|o| reports.iter().any(|h| h.observer == *o && h.target == t && ok(h)))\n    };\n    let calm',
     '.filter(|o| **o != t).any(|o| reports.iter().any(|h| h.observer == *o && h.target == t && ok(h)))\n    };\n    let calm',
     REL),
    ("a site's report about itself counts", GATE,
     'voters.iter().filter(|o| **o != t).all(|o| reports.iter().any(|h| h.observer == *o && h.target == t && ok(h)))\n    };\n    let calm',
     'voters.iter().all(|o| reports.iter().any(|h| h.observer == *o && h.target == t && ok(h)))\n    };\n    let calm', REL),
    ("a report under MIN_CALLS counts", GATE,
     '        self.calls >= MIN_CALLS', '        self.calls > 0', REL),
    ("a burn of exactly 2 x counts as calm", GATE,
     'self.evidence() && self.missed * ALLOWED_MISS_DENOM < x * self.calls',
     'self.evidence() && self.missed * ALLOWED_MISS_DENOM <= x * self.calls', REL),
    # R41 (2026-11-23): a 1-in-100 budget, DRAIN_BURN 5 x, MIN_CALLS 300, calm under 2 x over 15 minutes.
    ("the budget is the write SLO's 1 in 2,000", GATE,
     'pub const ALLOWED_MISS_DENOM: u64 = 100;', 'pub const ALLOWED_MISS_DENOM: u64 = 2_000;', REL),
    ("DRAIN_BURN is 6 x", GATE, 'pub const DRAIN_BURN: u64 = 5;', 'pub const DRAIN_BURN: u64 = 6;', REL),
    ("MIN_CALLS is the old 100", GATE, 'pub const MIN_CALLS: u64 = 300;', 'pub const MIN_CALLS: u64 = 100;', REL),
    ("calm needs under 1 x", GATE, 'pub const UNDRAIN_BURN: u64 = 2;', 'pub const UNDRAIN_BURN: u64 = 1;', REL),
    ("the health window drops its first second", GATE,
     '.filter(|((s, t), _)| *s >= from && *t == target.0)', '.filter(|((s, t), _)| *s > from && *t == target.0)', REL),
    ("graft-ops never drains a burning site", OPS,
     '        None => mode(*sick, crate::types::Mode::Drain),',
     '        None => return None,', REL),
    ("an automatic drain does not undrain the rolling site first", OPS,
     '        Some((s, d)) if d.by_drain_key && day == Some(SiteId(*s)) => mode(*s, crate::types::Mode::Normal),\n',
     '', REL),
    ("the site day's own drain ends when its site is calm", OPS,
     '.find(|(s, d)| d.by_drain_key && day != Some(SiteId(**s)) && ends(s))',
     '.find(|(s, d)| d.by_drain_key && ends(s))', REL),
    ("a drained site is undrained while it is merely not burning", OPS,
     '!v.burning && if loss { v.recovered } else { v.calm }', '!v.burning && if loss { v.recovered } else { true }', REL),
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
    # The hosted half's pure parts (2026-11-22): the counters the host sums, the missing N baseline, the day's versions,
    # and the shard objects a ContainerDay step covers.
    ("an N under STEP_MIN_ENTRIES is a baseline", GATE,
     '    if n.entries >= STEP_MIN_ENTRIES && tail > GATE_APPLY_LAG_SHARE_BP {', '    if n.entries > 0 && tail > GATE_APPLY_LAG_SHARE_BP {', REL),
    ("the lag shares are judged on a step under STEP_MIN_ENTRIES", GATE,
     '    if n1.entries < STEP_MIN_ENTRIES || n1.appends < STEP_MIN_ENTRIES {\n        return Gate::Starved(n1.entries.min(n1.appends));\n    }\n',
     '', REL),
    ("with no N answers the ack rate judges against 0 refused", GATE,
     'let (p0, c0) = (n.appends > 0).then(|| refused(n))?;', 'let (p0, c0) = (n.appends > 0).then(|| refused(n)).unwrap_or((0.0, 1.0));', REL),
    ("the p99 bucket is the one at 98%", GATE,
     'if *n > 0 && seen * 100 >= total * 99 {', 'if *n > 0 && seen * 100 >= total * 98 {', REL),
    ("the side the gate judges drops the acks", GATE,
     '            acks: self.acks,\n', '            acks: 0,\n', REL),
    ("summing counters drops the other isolate's lag", GATE,
     '        for (a, b) in self.lag.iter_mut().zip(&o.lag) {\n            *a += b;\n        }', '', REL),
    ("the day's N is the intent's own release", GATE,
     '    let n = crate::ops::running(fold, script)\n        .filter(|n| n != release)\n',
     '    let n = Some(*release)\n', REL),
    ("a ContainerDay's 10% step covers half the shard objects", SHARD,
     'let pct = [10u64, 50, 100][usize::from(step.min(2))];', 'let pct = [50u64, 50, 100][usize::from(step.min(2))];', GIT),
    ("the 50% step picks shards apart from the 10% step's", SHARD,
     'if weight(key.as_bytes(), 0) % 100 < pct', 'if weight(format!("{key}#{pct}").as_bytes(), 0) % 100 < pct', GIT),
    # BF-39's test hooks (2026-11-22): a product graft-ops must not read OPS_KILL or OPS_STEP_MIN_ENTRIES.
    ("the product graft-ops builds with test-faults", OPS_TOML,
     'test-faults = []', 'default = ["test-faults"]\ntest-faults = []', OPSFAULTS),
    ("the product graft-ops lowers a starved gate", EXEC,
     '    #[cfg(feature = "test-faults")]\n    {\n        let floor = _env.var(', '    {\n        let floor = _env.var(',
     OPSFAULTS),
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
