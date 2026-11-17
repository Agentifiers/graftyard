"""purge.py's gate and prefixes, against a fake bucket: `python3 -m unittest tests/fanout/test_purge.py`."""
import os, sys, unittest

sys.path.insert(0, os.path.dirname(__file__))
from purge import prefixes, purge, retired  # noqa: E402

NJ = {"joined": False}


class Bucket:
    """A list_objects_v2 paginator and delete_objects over a set of keys, two keys a page."""

    def __init__(self, keys):
        self.keys = set(keys)

    def get_paginator(self, _):
        outer = self

        class P:
            def paginate(self, Bucket, Prefix):
                ks = sorted(k for k in outer.keys if k.startswith(Prefix))
                for i in range(0, max(len(ks), 1), 2):
                    yield {"Contents": [{"Key": k} for k in ks[i:i + 2]]}
        return P()

    def delete_objects(self, Bucket, Delete):
        for o in Delete["Objects"]:
            self.keys.discard(o["Key"])
        return {}


class PurgeTest(unittest.TestCase):
    def test_the_gate_takes_only_an_all_not_joined_group_on_every_site(self):
        self.assertTrue(retired([(0, NJ), (1, NJ), (1, NJ), (2, NJ)]))
        # A live follower, a failed status, a missing site, or no answers at all refuses the network.
        self.assertFalse(retired([(0, NJ), (1, {"role": "follower", "term": 2}), (2, NJ)]))
        self.assertFalse(retired([(0, NJ), (1, {"error": "timeout"}), (2, NJ)]))
        self.assertFalse(retired([(0, NJ), (1, None), (2, NJ)]))
        self.assertFalse(retired([(0, NJ), (1, NJ)]))
        self.assertFalse(retired([]))
        # `joined` missing is not "not joined".
        self.assertFalse(retired([(0, NJ), (1, {}), (2, NJ)]))

    def test_a_purge_deletes_every_kind_of_the_network_and_nothing_of_another(self):
        mine = ["pack/131322/a.pack", "pack/131322/a.idx", "committed/pack/131322/a", "upload/131322/u",
                "compose/131322/f", "clone/131322/k/0", "archive-cache/131322/x.tar", "blob/131322/o",
                "committed/blob/131322/o", "archive/repo:131322/r", "snapshot/repo:131322/core/s",
                "snapshot/repo-meta:131322/metadata/s", "inline-idx/repo:131322/1"]
        others = ["pack/13132/a.pack", "pack/1313220/a.pack", "archive/repo:1313221/r", "snapshot/repo:13132/core/s",
                  "committed/pack/131323/a", "archive/repo-meta:131323/r", "pack/131322"]
        b = Bucket(mine + others)
        self.assertEqual(purge(b, "x", 131322, dry=True), (len(mine), len(mine)))
        self.assertEqual(b.keys, set(mine + others), "a dry run deletes nothing")
        self.assertEqual(purge(b, "x", 131322, dry=False), (len(mine), 0))
        self.assertEqual(b.keys, set(others))

    def test_prefixes_end_in_a_slash(self):
        self.assertTrue(all(p.endswith(("/131322/", ":131322/")) for p in prefixes(131322)))


if __name__ == "__main__":
    unittest.main()
