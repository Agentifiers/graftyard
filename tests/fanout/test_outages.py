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

    def test_a_row_under_one_edge_request_a_second_is_not_judged(self):
        # m3fg11's shape: one edge request every 6 s, so 5 of every 6 seconds read edge 0 while the sites serve.
        r = rows([(1, 9) if i % 6 == 0 else (0, 9) for i in range(60)])
        wins = arrivals.outages(r)
        self.assertEqual(len(wins), 10)
        s = arrivals.summary(wins, T, T + 60_000, r)
        self.assertEqual((s["outages"], s["valid"]), ("n/a (low rate)", True))
        # The same idle gaps in a row at the rule's rate (here 2 a second outside them) are outages, and invalidate it.
        busy = rows([(2, 9) if i % 6 == 0 else (0, 9) if i % 6 == 1 else (2, 9) for i in range(60)])
        s = arrivals.summary(arrivals.outages(busy), T, T + 60_000, busy)
        self.assertEqual((s["outageSeconds"], s["valid"]), (10.0, False))

    def test_the_window_opens_at_the_first_request_not_the_announced_start(self):
        # m3fwi3's shape: the driver announced at T, its first request left 13.5 s later, and the edge was idle until
        # then while the sites served. From T the idle seconds read as outages (over 5%); from the first request the
        # burst's own 4 edge-0 seconds remain, a valid row.
        idle, load = 13, 167
        r = rows([(0, 90)] * idle + [(200, 800)] * 64 + [(0, 100)] * 4 + [(200, 800)] * (load - 64 - 4))
        b = T + (idle + load) * 1000
        self.assertFalse(arrivals.summary(arrivals.outages(r), T, b, r)["valid"])
        samples = [[T + 14_000, "read:api", 9, "200"], [T + 13_500, "side:human", 2202, "ok"], [T + 90_000, "push:burst", 4092, "ok"]]
        f = tempfile.NamedTemporaryFile("wb", suffix=".gz", delete=False)
        f.write(gzip.compress("\n".join(json.dumps(s) for s in samples).encode()))
        f.close()
        a = outagecut.first(f.name)
        self.assertEqual(a, T + 13_500, "the earliest sample, not the file's first line")
        a -= a % 1000
        late = [x for x in r if x["t"] >= a]
        s = arrivals.summary(arrivals.outages(late), a, b, late)
        self.assertEqual((s["outageSeconds"], s["valid"]), (4.0, True))

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
