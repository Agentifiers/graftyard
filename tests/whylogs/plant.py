#!/usr/bin/env python3
"""why-logs' planted bugs: each must fail its test. Restores each file byte for byte. Run from the worktree root:
`python3 tests/whylogs/plant.py`."""
import subprocess

MB = "workers/site/src/member.rs"
HO = "workers/site/src/host.rs"
ED = "workers/edge/src/lib.rs"
HE = "crates/raft/src/member/health.rs"
SITE_GATE = ["cargo", "test", "-q", "-p", "graftyard-site", "--lib", "--", "every_read_is_shed_past_reads_max"]
SITE_LINE = ["cargo", "test", "-q", "-p", "graftyard-site", "--lib", "--", "a_step_down_window_logs"]
EDGE = ["cargo", "test", "-q", "-p", "graftyard-edge", "--lib", "--", "a_5xx_logs_its_kind"]
RAFT = ["cargo", "test", "-q", "-p", "graftyard-raft", "--test", "leadership", "--", "six_slow_confirms"]
ANSWER = """    if answers >= ANSWER_BYTES_MAX {
        return Gate::Shed(ShedGate::AnswerBytes);
    }
"""
HEAVY = """    if heavy && (leading || index) && !learner && heavies >= HEAVY_READS {
        return Gate::Shed(ShedGate::HeavyReads);
    }
"""
SLOW = "let slow = w.confirms.iter().filter(|c| c.1 > graftyard_raft::STEP_DOWN.confirm_p99_ms).count();"
WHY = "let why = msg.as_deref().unwrap_or(body).trim();"
NOTE = """            Ok(_) => {
                self.note_trip();"""
M = [("HEAVY_READS judged before ANSWER_BYTES_MAX, so a shed is named by the wrong gate", MB, ANSWER + HEAVY,
      HEAVY + ANSWER, SITE_GATE),
     ("every overloaded shed named readsMax", MB, "return Gate::Shed(ShedGate::AnswerBytes);",
      "return Gate::Shed(ShedGate::ReadsMax);", SITE_GATE),
     ("a confirm of exactly confirm_p99_ms counted slow", HO, SLOW, SLOW.replace("c.1 >", "c.1 >="), SITE_LINE),
     ("the 5xx line logs the whole JSON body, not its message", ED, WHY, "let why = body.trim(); let _ = msg;", EDGE),
     ("a StepDown transfer keeps no window", HE, NOTE, "            Ok(_) => {", RAFT)]
for name, f, old, new, cmd in M:
    s = open(f).read()
    assert s.count(old) == 1, name
    open(f, "w").write(s.replace(old, new))
    try:
        code = subprocess.run(cmd, capture_output=True, text=True, timeout=1200).returncode
    finally:
        open(f, "w").write(s)
    print("CAUGHT" if code else "MISSED", name, code, flush=True)
