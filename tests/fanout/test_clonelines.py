"""clonelines.py's two B0-34 clone lines: `python3 -m unittest tests/fanout/test_clonelines.py`."""
import os, sys, unittest

sys.path.insert(0, os.path.dirname(__file__))
import clonelines  # noqa: E402


def probes(phase, hits=(), misses=(), edge=None):
    out = [[0, f"read:cloneTtfb:hit@{phase}", v, "200"] for v in hits] + [[0, f"read:cloneTtfb:miss@{phase}", v, "composed"] for v in misses]
    if edge is not None:
        out += [[0, f"read:cloneTtfb:hit:edge@{phase}", v, "200"] for v in edge]
    return out


class Lines(unittest.TestCase):
    def test_the_hits_line_reads_steady_state_hits_only(self):
        # j9's 100 hits at 900 ms pass; a slow miss in j9 and slow hits in kill do not count against it.
        s = probes("j9", hits=[900] * 100, misses=[9000] * 20) + probes("kill", hits=[4000] * 50)
        by, v = clonelines.lines(s)
        self.assertTrue(v["hits"]["pass"])
        self.assertEqual((v["hits"]["p99"], v["hits"]["n"]), (900, 100))
        self.assertEqual(by["j9"]["miss"]["p99"], 9000)
        # p99 is a1's nearest rank, the value at floor(0.99 n): of 200 hits, two over 1 s put it at 1,001 ms and fail,
        # and one over leaves it at 900 and passes.
        _, v = clonelines.lines(probes("j9", hits=[900] * 198 + [1001, 1200]))
        self.assertEqual((v["hits"]["p99"], v["hits"]["pass"]), (1001, False))
        _, v = clonelines.lines(probes("j9", hits=[900] * 199 + [1200]))
        self.assertEqual((v["hits"]["p99"], v["hits"]["pass"]), (900, True))

    def test_every_clone_is_a_share_of_hits_and_misses_per_cold_phase(self):
        # kill: 9 of 100 over 5 s (91%) passes; second: 9 of 90 (90%) passes at the line; 5 s itself is within.
        s = probes("kill", hits=[1000] * 80, misses=[5000] * 11 + [5001] * 9) + probes("second", hits=[2000] * 81, misses=[7000] * 9)
        _, v = clonelines.lines(s)
        self.assertEqual(v["every"]["share"], {"kill": 0.91, "second": 0.9})
        self.assertTrue(v["every"]["pass"])
        self.assertFalse(v["every"]["asWritten"])
        # One phase under 90% fails the line, though the two together are over it.
        _, v = clonelines.lines(probes("kill", hits=[1000] * 99, misses=[6000]) + probes("second", hits=[1000] * 8, misses=[6000] * 2))
        self.assertEqual(v["every"]["share"]["second"], 0.8)
        self.assertFalse(v["every"]["pass"])
        # Hostile is not a cold phase: its clones do not count.
        _, v = clonelines.lines(probes("kill", hits=[100]) + probes("second", hits=[100]) + probes("hostile", misses=[60_000] * 50))
        self.assertTrue(v["every"]["asWritten"])

    def test_a_phase_with_no_probes_fails_rather_than_passing_empty(self):
        _, v = clonelines.lines(probes("kill", hits=[100] * 10))
        self.assertIsNone(v["every"]["share"]["second"])
        self.assertFalse(v["every"]["pass"])
        _, v = clonelines.lines(probes("kill", hits=[100]))
        self.assertFalse(v["hits"]["pass"])

    def test_edge_clock_is_kept_apart_from_the_client_clock(self):
        by, v = clonelines.lines(probes("j9", hits=[3000] * 10, edge=[400] * 10))
        self.assertEqual((by["j9"]["hit"]["p99"], by["j9"]["hitEdge"]["p99"]), (3000, 400))
        self.assertFalse(v["hits"]["pass"])
        # The unsplit kinds (read:cloneTtfb@j9 and its :edge) are not probes of either.
        by, _ = clonelines.lines([[0, "read:cloneTtfb@j9", 50, "200"], [0, "read:cloneTtfb:edge@j9", 50, "200"]])
        self.assertEqual(by["j9"]["hit"]["n"] + by["j9"]["miss"]["n"], 0)


if __name__ == "__main__":
    unittest.main()
