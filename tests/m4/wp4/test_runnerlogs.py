"""runnerlogs.fetch's chunks, retries and busy-span splits: `python3 -m unittest tests/m4/wp4/test_runnerlogs.py`."""
import os, sys, unittest

sys.path.insert(0, os.path.dirname(__file__))
import runnerlogs  # noqa: E402

H = 3_600_000
T = 1_795_000_000_000 - 1_795_000_000_000 % H


def ev(t, i):
    return {"timestamp": t, "$metadata": {"id": f"e{i}"}}


class FakeApi:
    """The events view over `events`: newest first, at most LIMIT. `fail(lo)` says whether a query starting at `lo`
    raises this time."""

    def __init__(self, events, fail=lambda lo, n: False):
        self.events, self.fail, self.calls = events, fail, []

    def __call__(self, lo, hi, timeout):
        self.calls.append((lo, hi))
        if self.fail(lo, len(self.calls)):
            raise TimeoutError("_ssl.c:1064: The handshake operation timed out")
        got = sorted((e for e in self.events if lo <= e["timestamp"] < hi), key=lambda e: -e["timestamp"])
        return got[:runnerlogs.LIMIT]


def run(api, a, b):
    sleeps = []
    events, failed = runnerlogs.fetch(a, b, api, sleep=sleeps.append)
    return events, failed, sleeps


class Fetch(unittest.TestCase):
    def test_a_window_reads_in_hour_chunks_and_a_quiet_hour_costs_one_query(self):
        # 2.5 hours, one event in each part: three chunks, the last 30 min, one query each.
        api = FakeApi([ev(T + 10, 0), ev(T + H + 10, 1), ev(T + 2 * H + 10, 2)])
        events, failed, _ = run(api, T, T + 2 * H + H // 2)
        self.assertEqual([e["$metadata"]["id"] for e in events], ["e0", "e1", "e2"])
        self.assertEqual(failed, [])
        self.assertEqual(api.calls, [(T, T + H), (T + H, T + 2 * H), (T + 2 * H, T + 2 * H + H // 2)])

    def test_a_query_that_fails_twice_is_tried_a_third_time_with_backoff(self):
        api = FakeApi([ev(T + 5, 0), ev(T + H + 5, 1)], fail=lambda lo, n: lo == T + H and n <= 3)
        events, failed, sleeps = run(api, T, T + 2 * H)
        self.assertEqual(len(events), 2, "the retried chunk's event is read")
        self.assertEqual(failed, [])
        self.assertEqual(sleeps, [5, 20], "two backoffs, growing")

    def test_a_chunk_that_never_answers_is_named_and_the_rest_still_read(self):
        api = FakeApi([ev(T + 5, 0), ev(T + H + 5, 1), ev(T + 2 * H + 5, 2)], fail=lambda lo, n: lo == T + H)
        events, failed, sleeps = run(api, T, T + 3 * H)
        self.assertEqual([e["$metadata"]["id"] for e in events], ["e0", "e2"])
        self.assertEqual([(f["from"], f["to"]) for f in failed],
                         [(runnerlogs.iso(T + H), runnerlogs.iso(T + 2 * H))])
        self.assertIn("handshake", failed[0]["error"])
        self.assertEqual(sleeps, [5, 20], "three tries, then given up")
        self.assertEqual(sum(1 for c in api.calls if c[0] == T + 2 * H), 1, "the next chunk is read once")
        line = runnerlogs.chunks_line(T, T + 3 * H, failed)
        self.assertEqual((line["chunks"], line["complete"]), (3, False))

    def test_a_busy_hour_is_split_so_no_event_past_the_limit_is_lost(self):
        # 250 events in one hour, spread over its 12 parts unevenly: a single query would answer only 100.
        evs = [ev(T + (i * 37_001) % H, i) for i in range(250)]
        api = FakeApi(evs)
        events, failed, _ = run(api, T, T + H)
        self.assertEqual(sorted(e["$metadata"]["id"] for e in events), sorted(f"e{i}" for i in range(250)))
        self.assertEqual(failed, [])
        self.assertEqual([e["timestamp"] for e in events], sorted(e["timestamp"] for e in events))

    def test_a_failure_deep_in_a_busy_hour_retries_that_query_not_the_hour(self):
        evs = [ev(T + (i * 37_001) % H, i) for i in range(250)]
        api = FakeApi(evs, fail=lambda lo, n: n == 5)
        events, failed, sleeps = run(api, T, T + H)
        self.assertEqual((len(events), failed, sleeps), (250, [], [5]))
        self.assertEqual(sum(1 for c in api.calls if c == (T, T + H)), 1, "the hour's first query is not repeated")

    def test_a_chunk_past_its_deadline_is_named(self):
        # A busy hour of 25 queries on a clock that moves 100 s per reading passes CHUNK_S (900 s) partway through.
        now = [0.0]
        api = FakeApi([ev(T + (i * 37_001) % H, i) for i in range(250)])
        def clock():
            now[0] += 100
            return now[0]
        events, failed = runnerlogs.fetch(T, T + H, api, clock=clock, sleep=lambda s: None)
        self.assertEqual(len(failed), 1)
        self.assertIn("deadline", failed[0]["error"])

    def test_a_busy_30s_slice_is_walked_back_by_timestamp(self):
        # 230 events in 10 s: no split helps below 30 s, so the slice is walked back from its newest event.
        evs = [ev(T + 1000 + i * 40, i) for i in range(230)]
        events, failed, _ = run(FakeApi(evs), T, T + 30_000)
        self.assertEqual(len(events), 230)
        self.assertEqual(failed, [])


if __name__ == "__main__":
    unittest.main()
