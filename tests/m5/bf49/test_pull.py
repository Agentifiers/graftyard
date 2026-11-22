#!/usr/bin/env python3
"""pull.py's decoding and checks against bodies built here as crates/git/src/export.rs writes them. No network."""
import json
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import pull  # noqa: E402


def obj(minute, seq, entries):
    """An ExportObject's bytes, as `encode_object` writes them. entries: [(chain, first, [row, ...])]."""
    head = {
        "batch": {"minute": minute, "seq": seq, "chains": [[c, f + len(rows) - 1] for c, f, rows in entries]},
        "entries": [[c, f, len(rows)] for c, f, rows in entries],
    }
    out = pull.frame(json.dumps(head).encode())
    for _, _, rows in entries:
        for r in rows:
            out += pull.frame(r)
    return out


def rows(chain, first, n):
    return [f"{chain}#{i}".encode() for i in range(first, first + n)]


def page(*objects):
    return b"".join(pull.frame(o) for o in objects)


class Decode(unittest.TestCase):
    def test_a_two_object_page_decodes_and_counts(self):
        a = obj(5, 0, [("release", 1, rows("release", 1, 2)), ("repo:12", 1, rows("repo:12", 1, 3))])
        b = obj(5, 1, [("repo:12", 4, rows("repo:12", 4, 2)), ("tokens:5", 7, rows("tokens:5", 7, 1))])
        feed = pull.Feed()
        for raw in pull.unframe(page(a, b)):
            feed.add(pull.decode_object(raw), len(raw))
        self.assertEqual(feed.errors, [])
        self.assertEqual(feed.counts(), {"objects": 2, "chains": 3, "entries": 8, "bytes": len(a) + len(b)})
        self.assertEqual(feed.position, (5, 1))
        self.assertEqual((feed.first["repo:12"], feed.last["repo:12"]), (1, 5))
        self.assertEqual(feed.first["tokens:5"], 7, "a chain's first appearance may start anywhere")
        self.assertEqual(feed.rows["repo:12"][4], b"repo:12#4")

    def test_a_corrupted_frame_fails_its_digest(self):
        body = bytearray(page(obj(1, 0, [("release", 1, rows("release", 1, 1))])))
        body[10] ^= 1
        with self.assertRaisesRegex(pull.FrameError, "frame 0 fails its digest"):
            pull.unframe(bytes(body))
        inner = bytearray(obj(1, 0, [("release", 1, rows("release", 1, 2))]))
        inner[-40] ^= 1  # inside the last entry row
        with self.assertRaisesRegex(pull.FrameError, "frame 2 fails its digest"):
            pull.decode_object(bytes(inner))
        with self.assertRaisesRegex(pull.FrameError, "shorter"):
            pull.unframe(page(b"abc")[:-33])
        with self.assertRaisesRegex(pull.FrameError, "digest"):
            pull.unframe(page(b"abc")[:-1])

    def test_a_header_that_counts_more_rows_than_follow_is_refused(self):
        whole = obj(1, 0, [("release", 1, rows("release", 1, 2))])
        cut = whole[:-(4 + len(b"release#2") + 32)]
        with self.assertRaisesRegex(ValueError, "1 of 2 entries"):
            pull.decode_object(cut)
        with self.assertRaisesRegex(ValueError, "past the last"):
            pull.decode_object(whole + pull.frame(b"extra"))

    def test_a_gap_a_repeat_and_an_old_position_are_errors(self):
        feed = pull.Feed()
        feed.add(pull.decode_object(obj(2, 0, [("repo:1", 1, rows("repo:1", 1, 3))])))
        feed.add(pull.decode_object(obj(2, 1, [("repo:1", 5, rows("repo:1", 5, 1))])))
        self.assertEqual(len(feed.errors), 1)
        self.assertIn("repo:1 starts at 5 after 3 (skips)", feed.errors[0])
        feed.add(pull.decode_object(obj(2, 2, [("repo:1", 5, rows("repo:1", 5, 1))])))
        self.assertIn("(repeats)", feed.errors[1])
        feed.add(pull.decode_object(obj(2, 1, [("repo:1", 6, rows("repo:1", 6, 1))])))
        self.assertIn("is not after 2.2", feed.errors[2])


class Checks(unittest.TestCase):
    def test_link_next(self):
        h = '<https://e.example/api/graftyard/v1/export/batches?after=5.1>; rel="next"'
        self.assertEqual(pull.next_link(h), "https://e.example/api/graftyard/v1/export/batches?after=5.1")
        self.assertEqual(pull.next_link('<https://x/a>; rel="prev", <https://x/b>; rel="next"'), "https://x/b")
        self.assertIsNone(pull.next_link(None))
        self.assertIsNone(pull.next_link('<https://x/a>; rel="prev"'))

    def test_endpoint_rows_compare_byte_for_byte_over_the_overlap(self):
        feed = {i: f"r{i}".encode() for i in range(4, 8)}
        served = [f"r{i}".encode() for i in range(1, 10)]
        self.assertEqual(pull.compare_rows(feed, 1, served), [])
        served[5] = b"r6!"
        self.assertEqual(pull.compare_rows(feed, 1, served), [6])

    def test_snapshot_frames_are_key_object_pairs_core_first(self):
        core = b"snapshot/repo:12/core/00000000000000000000-00000000000000000100-ab"
        meta = b"snapshot/repo:12/metadata/00000000000000000000-00000000000000000100-cd"
        self.assertEqual(pull.check_snapshot([core, b"x", meta, b"y"]), [])
        self.assertTrue(pull.check_snapshot([meta, b"y", core, b"x"]))
        self.assertTrue(pull.check_snapshot([core]))
        self.assertTrue(pull.check_snapshot([b"export/1/2", b"x"]))

    def test_revoke_passes_only_with_no_late_200_and_a_401(self):
        p = lambda ms, st: {"sent_ms": ms, "status": st}  # noqa: E731
        ok = pull.judge_revoke([p(0, 200), p(9_750, 200), p(10_000, 401), p(10_250, 401), p(10_500, 503)])
        self.assertEqual((ok["pass"], ok["last_ok_ms"], ok["first_401_ms"]), (True, 9_750, 10_000))
        self.assertEqual((ok["n_200"], ok["n_401"], ok["n_other"]), (2, 2, 1))
        self.assertFalse(pull.judge_revoke([p(0, 401), p(10_250, 200)])["pass"], "a 200 after READ_CACHE")
        self.assertFalse(pull.judge_revoke([p(0, 200), p(250, 503)])["pass"], "no 401 seen")
        self.assertTrue(pull.judge_revoke([p(0, 401)])["pass"])


if __name__ == "__main__":
    unittest.main()
