#!/usr/bin/env python3
"""compare.py DIR BUCKET: the hosted vault row's check of what the vault holds (DIR/vault-keys.json) against the
install's export bucket (DIR/export-keys.json), both written by `r2.py ... snap`. Feed objects are single-part puts on
both sides, so equal etags (MD5) mean equal bytes. Also reads state/vault (DIR/state.json, from `r2.py ... get`) for the
cursors and the missed and undecoded counts. Prints one JSON line; exits 1 unless every export object the vault's
position has passed is under feed/ with the same etag and nothing is under a prefix the lock does not cover."""
import json
import os
import sys

d = sys.argv[1]
vault = json.load(open(f"{d}/vault-keys.json"))
export = json.load(open(f"{d}/export-keys.json"))
state = json.load(open(f"{d}/state.json")) if os.path.exists(f"{d}/state.json") else None
feed = {k[len("feed/"):]: v for k, v in vault.items() if k.startswith("feed/")}
exp = {k[len("export/"):]: v for k, v in export.items() if k.startswith("export/")}
last = None
if state and state["feed"].get("last"):
    p = state["feed"]["last"]
    last = f"{p['minute']:012}/{p['seq']:010}"
due = {k: v for k, v in exp.items() if last is None or k <= last}
missing = sorted(k for k in due if k not in feed)
differ = sorted(k for k in due if k in feed and feed[k][1] != due[k][1])
extra = sorted(k for k in feed if k not in exp)
prefixes = sorted({k.split("/")[0] for k in vault})
stray = [p for p in prefixes if p not in ("feed", "entries", "snapshot", "pack", "state")]
out = {
    "export_objects": len(exp), "vault_feed": len(feed), "position": last, "due": len(due),
    "missing": missing[:5], "missing_n": len(missing), "etag_differ": differ[:5], "extra_n": len(extra),
    "packs": sum(1 for k in vault if k.startswith("pack/")),
    "snapshots": sum(1 for k in vault if k.startswith("snapshot/")),
    "entries": sum(1 for k in vault if k.startswith("entries/")),
    "stray_prefixes": stray,
}
if state:
    out.update({"chains": len(state["feed"]["cursors"]), "owed": len(state["copies"]) + len(state["backfill"]),
                "missed": state["missed"], "undecoded": state["undecoded"], "gc": len(state["gc"])})
print(json.dumps(out))
sys.exit(0 if not missing and not differ and not stray and len(due) > 0 else 1)
