"""lsrefsab.py's setup baseline and minute pairing, on a built trace: `python3 -m unittest tests/fanout/test_lsrefsab.py`."""
import contextlib, io, json, os, sys, tempfile, unittest

sys.path.insert(0, os.path.dirname(__file__))
import lsrefsab  # noqa: E402

W = 60_000
T0 = 1_800_000_000_000 - 1_800_000_000_000 % (2 * W)  # an even (edge) window starts here


def trace():
    """Leader site 0. Setup 0-5 min at confirm 30; load from 5 min to 13 min, learners at 50 /read/s. In load, edge
    (even) minutes confirm 90 and shard (odd) minutes 40, so the paired difference is +50; ls-refs reads come only in
    edge minutes."""
    lead, lrn = [], []
    for t in range(T0, T0 + 13 * W, 250):
        m = (t - T0) // W
        ms = 30 if m < 5 else (90 if (t // W) % 2 == 0 else 40)
        lead.append([t, 10, 0, 0, 0, ms])
        lead.append([t + 1, 16, 0, 0, 1, 0])
    for t in range(T0 + 5 * W, T0 + 13 * W, 20):
        lrn.append([t, 25, 255, 0, 2, 1])
        if (t // W) % 2 == 0:
            lrn.append([t, 26, 0, 0, 25, 1])
    lines = [{"site": 0, "leader": 0, "rows": lead}, {"learner": 192, "rows": lrn},
             {"site": 0, "isolate": "aaa", "since": 1}, {"learner": 192, "isolate": "aaa", "since": 1},
             {"learner": 193, "isolate": "bbb", "since": 1}]
    f = tempfile.NamedTemporaryFile("w", suffix=".jsonl", delete=False)
    f.write("\n".join(json.dumps(x) for x in lines))
    f.close()
    return f.name


class LsRefsAB(unittest.TestCase):
    def run_main(self, every=None):
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            lsrefsab.main(trace(), every)
        return out.getvalue()

    def test_setup_baseline_is_before_load(self):
        # Setup confirms are all 30; a baseline that took any load minute would not be.
        self.assertIn("setup confirm p50 30 ", self.run_main())

    def test_pairs_edge_minus_shard(self):
        out = self.run_main(60)
        self.assertIn("paired edge minus shard confirm p50: median +50.0 ms", out)
        self.assertIn("edge slower in 3, shard slower in 0", out)
        self.assertRegex(out, r"edge  learner ls-refs\+refs\s+50\.0/s")
        self.assertRegex(out, r"shard learner ls-refs\+refs\s+0\.0/s")

    def test_shared_isolate_named(self):
        self.assertIn("shared: l192+v0", self.run_main())


if __name__ == "__main__":
    unittest.main()
