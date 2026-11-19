"""arrivals.py's outage windows and outagecut.py's two lines: `python3 -m unittest tests/fanout/test_outages.py`."""
import contextlib, gzip, io, json, os, sys, tempfile, unittest

sys.path.insert(0, os.path.dirname(__file__))
import arrivals  # noqa: E402
import outagecut  # noqa: E402

T = 1_800_000_000_000


def rows(spec):
    """spec: (edge, sites) per second from T."""
    return [{"t": T + i * 1000, "edge": e, "sites": s, "tools": 0} for i, (e, s) in enumerate(spec)]


class Outages(unittest.TestCase):
    def test_only_seconds_with_a_silent_edge_and_serving_sites(self):
        # 2-4: edge 0, sites up -> outage. 6: both 0 (everything idle) -> not an outage. 8: edge 0, sites 0 -> not.
        r = rows([(5, 9), (5, 9), (0, 9), (0, 3), (0, 1), (5, 9), (0, 0), (5, 9), (0, 0), (5, 9)])
        self.assertEqual(arrivals.outages(r), [(T + 2000, T + 5000)])

    def test_share_over_five_percent_invalidates_the_row(self):
        a, b = T, T + 100_000
        self.assertTrue(arrivals.summary([(T, T + 5000)], a, b)["valid"])
        s = arrivals.summary([(T, T + 3000), (T + 50_000, T + 53_000)], a, b)
        self.assertEqual((s["outageSeconds"], s["valid"]), (6.0, False))

    def test_cut_removes_samples_in_and_just_before_a_window(self):
        samples = [[T + 1000, "read:api", 300, "200"],  # 4 s before the window: kept
                   [T + 3000, "read:api", 2000, "0"],  # 2 s before: its retries ran into it, removed
                   [T + 6000, "read:api", 200, "0"],  # inside: removed
                   [T + 12_000, "read:api", 5000, "200"],  # after: kept
                   [T + 13_000, "read:api", 0, "skipped: cap"]]
        f = tempfile.NamedTemporaryFile("wb", suffix=".gz", delete=False)
        f.write(gzip.compress("\n".join(json.dumps(s) for s in samples).encode()))
        f.close()
        outs = json.dumps({"outages": [["2027-01-15T08:00:05Z", "2027-01-15T08:00:10Z"]]})
        # T is 2027-01-15T08:00:00Z.
        self.assertEqual(arrivals.iso(T), "2027-01-15T08:00:00Z")
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            outagecut.main([f.name, outs, "read:api"])
        v = json.loads(out.getvalue())
        self.assertEqual(v["measured"], {"n": 4, "served": 0.5, "p50": 2000, "p99": 5000, "cap": 1})
        self.assertEqual(v["outagesRemoved"], {"n": 2, "served": 1.0, "p50": 5000, "p99": 5000, "cap": 1})


if __name__ == "__main__":
    unittest.main()
