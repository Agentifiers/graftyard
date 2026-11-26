#!/usr/bin/env python3
"""B0-24a's front kill and reading: planted bugs in the SSH front's `POST /kill` and its open-session count
(`workers/site/ssh.mjs`, `kill` and `connect`), the variable that gates it (`workers/site/deploy.mjs`), and the row's
judge (`tests/m6/hosted/b024a-judge.mjs`). Plants one mutant at a time, runs the tests that guard it, and restores the
file byte for byte. Each line prints CAUGHT, MISSED, or DID NOT COMPILE. Run from the repository root:

    python3 tests/m0carried/plant283.py          # every mutant
    python3 tests/m0carried/plant283.py 0 3      # by number
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from plant121 import run  # noqa: E402

SSH = 'workers/site/ssh.mjs'
JUDGE = 'tests/m6/hosted/b024a-judge.mjs'
NODE = ['node', '--test', 'workers/site/tests/ssh.test.mjs']
ROW = ['node', '--test', 'tests/m6/hosted/b024a.test.mjs']

MUTANTS = [
    ("the kill answers on any version, the variable unread", SSH,
     '  if (o.env?.SSH_KILL !== "1") return new Response("Not Found", { status: 404 });\n', '', NODE),
    ("the kill answers on any truthy variable", SSH,
     '  if (o.env?.SSH_KILL !== "1") return', '  if (!o.env?.SSH_KILL || o.env.SSH_KILL === "0") return', NODE),
    ("the kill reads the open count after the destroy", SSH,
     '  const out = { at: Date.now(), running: !!c.running, open: o.inFlight ?? 0 };\n  if (out.running) await c.destroy("b024a kill");\n',
     '  if (c.running) await c.destroy("b024a kill");\n  await new Promise((r) => setTimeout(r, 5));\n'
     '  const out = { at: Date.now(), running: true, open: o.inFlight ?? 0 };\n', NODE),
    ("a session is counted before its banner", SSH,
     '  let front;\n  try {\n    front = await open(o, Date.now() + wait);',
     '  let front;\n  o.inFlight = (o.inFlight ?? 0) + 1;\n  try {\n    front = await open(o, Date.now() + wait);', NODE),
    ("a session's end is never counted", SSH,
     '  } finally {\n    o.inFlight--;\n  }\n', '  } finally {\n  }\n', NODE),
    ("the judge passes at 99%", JUDGE,
     'export const PASS = 0.999;', 'export const PASS = 0.99;', ROW),
    ("a lost acknowledged push does not fail the row", JUDGE,
     '  if (lost > 0 || rate < FAIL) return "fail";', '  if (rate < FAIL) return "fail";', ROW),
    ("the kill's time is shifted the wrong way", JUDGE,
     '  const at = kills.map((k) => k.at - (k.orbClockOffsetMs ?? 0))', '  const at = kills.map((k) => k.at + (k.orbClockOffsetMs ?? 0))',
     ROW),
    ("a failure in flight at a kill is counted after it", JUDGE,
     '    const c = inFlight(s) ? "inFlight" : afterKill(s) ? "afterKill" : "background";',
     '    const c = afterKill(s) ? "afterKill" : inFlight(s) ? "inFlight" : "background";', ROW),
    ("paused sessions are counted", JUDGE,
     '  const counted = sessions.filter((s) => !paused(s));', '  const counted = sessions;', ROW),
    ("the ceiling forgets the three fronts", JUDGE,
     '(kills.length * d) / (fronts * spanMs)', '(kills.length * d) / spanMs', ROW),
]

if __name__ == '__main__':
    run(MUTANTS)
