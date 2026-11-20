"""purge.py's gate and prefixes, against a fake bucket: `python3 -m unittest tests/fanout/test_purge.py`."""
import os, sys, unittest

sys.path.insert(0, os.path.dirname(__file__))
from purge import prefixes, purge, purge_twice, retired, second_pass_at, site_buckets  # noqa: E402

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

    def test_every_site_bucket_of_the_install_is_purged_and_no_other(self):
        # graft-m1's fourth site d holds every pack; purging a, b and c alone left 1048577's to be refilled from d.
        names = ["graft-m1-packs-a", "graft-m1-packs-b", "graft-m1-packs-c", "graft-m1-packs-d", "graft-m1-load",
                 "graft-m1-lin-packs-a", "graft-m1-wp6-a", "graft-m2b-site-a-packs", "graft-m1-packs-ab"]
        self.assertEqual(site_buckets(names, "graft-m1", "{install}-packs-{site}"),
                         [("a", "graft-m1-packs-a"), ("b", "graft-m1-packs-b"), ("c", "graft-m1-packs-c"),
                          ("d", "graft-m1-packs-d")])
        self.assertEqual(site_buckets(names, "graft-m2b", "{install}-site-{site}-packs"),
                         [("a", "graft-m2b-site-a-packs")])

    def test_prefixes_end_in_a_slash(self):
        self.assertTrue(all(p.endswith(("/131322/", ":131322/")) for p in prefixes(131322)))

    def test_second_pass_at_on_both_sides_of_a_slot_boundary(self):
        self.assertEqual(second_pass_at(1794933000), 1794933000 + 600)  # Exactly a boundary.
        self.assertEqual(second_pass_at(1794932999), 1794932100 + 600)  # One second before it.
        self.assertEqual(second_pass_at(1794932400), 1794932100 + 600)  # Mid-slot.

    def test_second_pass_waits_for_a_sweep_and_removes_its_late_marker(self):
        mine = ["pack/131349/a.pack", "committed/pack/131349/original", "upload/131349/u"]
        others = ["pack/131350/a.pack", "committed/pack/13132/x", "archive/repo:131350/r"]
        b = Bucket(mine + others)
        clock = [1794932400]
        sleeps = []

        def sleep(seconds):
            sleeps.append(seconds)
            b.keys.add("committed/pack/131349/x")
            clock[0] += seconds

        def purge_one(net):
            return {"a": purge(b, "x", net, False)}

        result = purge_twice(purge_one, [131349], lambda: clock[0], sleep)

        self.assertEqual(sleeps, [1794932100 + 600 - 1794932400])
        self.assertEqual(sleeps, [300])
        self.assertEqual(result, [(131349, {"a": (len(mine), 0)}, {"a": (1, 0)})])
        self.assertEqual(b.keys, set(others))

    def test_second_pass_does_not_wait_when_first_pass_has_outlasted_the_settle(self):
        clock = [1794933000]
        calls = []

        def purge_one(net):
            calls.append(net)
            if len(calls) == 1:
                clock[0] = 1794934000  # In the next slot, beyond the first slot's settle time.
            return {"a": (0, 0)}

        def sleep(seconds):
            self.fail(f"unexpected sleep({seconds})")

        result = purge_twice(purge_one, [131349], lambda: clock[0], sleep)
        self.assertEqual(calls, [131349, 131349])
        self.assertEqual(result, [(131349, {"a": (0, 0)}, {"a": (0, 0)})])

    def test_second_pass_with_no_networks_never_purges_or_sleeps(self):
        result = purge_twice(lambda net: self.fail("unexpected purge"), [], lambda: 1794932400,
                             lambda seconds: self.fail("unexpected sleep"))
        self.assertEqual(result, [])


if __name__ == "__main__":
    unittest.main()
