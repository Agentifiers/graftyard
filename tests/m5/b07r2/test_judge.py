#!/usr/bin/env python3
"""judge.py's four rules from m4ar2 (the measured intent, the newest pre-fault baseline, POLL_MS, fault.jsonl) still
fail a chain that really moved late. Each case builds raws by hand: one c-homed chain, active in the 50 s window, led
on c at the drain, and moved to a at drain + LATE. The control moves it at drain + 15 s and passes. No network.

    python3 tests/m5/b07r2/test_judge.py
"""
import json
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import judge  # noqa: E402

CHAIN, INTENT = "repo:1", "01TESTINTENT"
TD = 1_800_000_000_000  # the measured intent's Drain c
FAULT = TD - 180_000
LATE = 25_000
ROLE, LEADER, FOLLOWER = 3, 2, 0


def role(site, at, term, new):
    return [site, at, ROLE, term, None, 0, new]


def mode(index, at, site, m):
    return {"index": index, "at": at, "kind": "SiteMode", "sigs": ["477de5"],
            "entry": f"SiteMode {{ site: SiteId({site}), mode: {m} }}"}


def day(first, intent, td):
    """A SiteDay's entries from index `first`: Intent, Drain b, Normal b, Drain c at td, Normal c, IntentDone."""
    return [
        {"index": first, "at": td - 600_000, "kind": "Intent", "sigs": ["d38725"], "entry": f"SiteDay {{ }} id {intent}"},
        mode(first + 1, td - 500_000, 1, "Drain"),
        mode(first + 2, td - 3_000, 1, "Normal"),
        mode(first + 3, td, 2, "Drain"),
        mode(first + 4, td + 900_000, 2, "Normal"),
        {"index": first + 5, "at": td + 1_500_000, "kind": "IntentDone", "sigs": ["d38725"],
         "entry": f"IntentDone id {intent}"},
    ]


def poll(at, members):
    return {"at": at, "groups": {CHAIN: members}}


def led_by(site, term):
    return [[s, "leader" if s == site else "follower", term, site, term] for s in range(3)]


def raws(d, entries, status, rows, fault=FAULT):
    def put(name, xs):
        with open(f"{d}/{name}", "w") as f:
            f.writelines(json.dumps(x) + "\n" for x in xs)
    put("entries.jsonl", entries)
    put("status.jsonl", status)
    put("elections.jsonl", [{"at": TD + 60_000, "rows": {CHAIN: rows}, "missed": []}])
    put("ops.jsonl", [{"chain": CHAIN, "t": TD - 30_000 + i * 5_000, "ms": 100, "ok": True} for i in range(6)])
    normal_c = next(e["at"] for e in entries if e["kind"] == "SiteMode" and "SiteId(2)" in e["entry"]
                    and "Normal" in e["entry"] and e["at"] > TD)
    put("ops-log.jsonl", [{"at": normal_c + 5_000, "msg": 'Committed("SiteMode")'}])
    put("fault.jsonl", [{"at": fault}] if fault else [])
    with open(f"{d}/chains.json", "w") as f:
        json.dump({"c": [CHAIN], "ab": []}, f)


class StillFailsLate(unittest.TestCase):
    def verdict(self, entries, status, rows, fault=FAULT):
        with tempfile.TemporaryDirectory() as d:
            raws(d, entries, status, rows, fault)
            return {r["line"]: r for r in judge.judge(d, INTENT)}

    def assert_late(self, res, moved=LATE):
        g = res["every group led on c moves within 20 s (active: of the drain; idle: of its next request)"]
        self.assertFalse(g["pass"])
        self.assertEqual([(x["chain"], x["active"], x["moved_ms"]) for x in g["late"]], [(CHAIN, True, moved)])

    def plain(self, move):
        status = [poll(TD - 400_000, led_by(2, 2))]
        return day(0, INTENT, TD), status, [role(0, TD + move, 3, LEADER), role(2, TD + move + 50, 3, FOLLOWER)]

    def test_control_a_move_inside_20_s_passes(self):
        res = self.verdict(*self.plain(15_000))
        self.assertTrue(all(r["pass"] for r in res.values()), res)

    def test_late_move_fails(self):
        self.assert_late(self.verdict(*self.plain(LATE)))

    def test_measured_intent_an_earlier_days_drain_does_not_stand_in(self):
        # An earlier day drained c at TD - 1 h and the chain moved 5 s after it; c won it back before the measured
        # day. Judged against the earlier drain the line would read 5 s; against the measured one it is 25 s.
        old = TD - 3_600_000
        entries = day(0, "01OLDINTENT", old) + day(10, INTENT, TD)
        rows = [role(0, old + 5_000, 3, LEADER), role(2, TD - 300_000, 4, LEADER),
                role(0, TD + LATE, 5, LEADER), role(2, TD + LATE + 50, 5, FOLLOWER)]
        self.assert_late(self.verdict(entries, [poll(TD - 400_000, led_by(2, 4))], rows))

    def test_baseline_first_poll_names_no_leader_newest_pre_fault_does(self):
        # Restored from storage, the first poll's members know no leader; c's leadership predates every role row
        # in the trace, so only the later pre-fault poll places it on c.
        restored = [[s, "follower", 2, None, 2 if s == 2 else None] for s in range(3)]
        status = [poll(TD - 500_000, restored), poll(TD - 400_000, led_by(2, 2))]
        rows = [role(0, TD + LATE, 3, LEADER), role(2, TD + LATE + 50, 3, FOLLOWER)]
        self.assert_late(self.verdict(day(0, INTENT, TD), status, rows))

    def test_poll_ms_a_win_during_the_polls_span_is_applied(self):
        # The poll read the chain early in its span (a leading, term 2); c won term 3 5 s before the poll's `at`.
        # The role row lands inside POLL_MS, so c leads at the drain; an older row in the span does not undo it.
        status = [poll(TD - 400_000, led_by(0, 2))]
        rows = [role(0, TD - 420_000, 2, LEADER), role(2, TD - 405_000, 3, LEADER), role(0, TD - 404_000, 3, FOLLOWER),
                role(0, TD + LATE, 4, LEADER), role(2, TD + LATE + 50, 4, FOLLOWER)]
        self.assert_late(self.verdict(day(0, INTENT, TD), status, rows))

    def test_fault_baseline_ignores_polls_under_the_fault(self):
        # Under the fault c's member hangs and the poll keeps a's stale term-1 claim; the pre-fault poll names c.
        under = [[0, "leader", 1, 0, 1], [1, "follower", 2, 2, None]]
        status = [poll(FAULT - 30_000, led_by(2, 2)), poll(FAULT + 60_000, under)]
        rows = [role(0, TD + LATE, 3, LEADER), role(2, TD + LATE + 50, 3, FOLLOWER)]
        self.assert_late(self.verdict(day(0, INTENT, TD), status, rows))

    def test_the_measured_intent_missing_fails(self):
        entries, status, rows = self.plain(15_000)
        with tempfile.TemporaryDirectory() as d:
            raws(d, entries, status, rows)
            res = judge.judge(d, "01NOTTHERE")
        self.assertEqual([(r["line"], r["pass"]) for r in res], [("the measured intent is in the group", False)])


if __name__ == "__main__":
    unittest.main()
