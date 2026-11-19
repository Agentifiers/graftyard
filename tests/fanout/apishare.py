#!/usr/bin/env python3
"""apishare.py A1_ROW_JSONL -> the row's API served share, `tier-off-leader`'s flip line. `read:api` when the row
offers API reads (B0-29); B0-33 offers none (`apiReads: 0`), so for it the share is over its REST and GraphQL requests
(`side:*` and `fast-user`), most of which read the repository's tiers before they write. Client-capped sends are left
out; a 2xx (or `ok`) counts as served."""
import json, sys

OK = {"ok", "200", "201", "204"}
for line in open(sys.argv[1]):
    d = json.loads(line)
    if "byKind" not in d:
        continue
    kinds = ["read:api"] if "read:api" in d["byKind"] else [k for k in d["byKind"] if k.startswith("side:") or k == "fast-user"]
    for name, ks in (("total", kinds), *((k, [k]) for k in kinds)):
        sent = served = shed = 0
        for k in ks:
            for c, n in d["byKind"][k]["codes"].items():
                if c.startswith("skipped"):
                    continue
                sent += n
                served += n * (c in OK)
                shed += n * ("503" in c)
        print(f"  {name:18} sent {sent:6}  served {served:6} ({100 * served / max(1, sent):.1f}%)  503 {shed}")
