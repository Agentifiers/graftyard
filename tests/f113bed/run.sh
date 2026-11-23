#!/usr/bin/env bash
# F113's warm case in local workerd, on the site script's own build (workers/site, test-faults): three site scripts
# bound to each other's classes as exportbed binds them. `driver.mjs` builds a group, replaces site 1's voter through
# the leader's `/replace` (the placement retry B0-32 m3fk2B's setup ran), then sends read-index reads to site 1's
# first-generation member, as the edge's spread pass does, and one to its replacement. A control group skips the
# replacement. A member's `/ping` waits 40 ms (`PING_MS`), so a try passes MIN_PAIR_RTT. Exits 1 unless the
# replaced member, within 12 s of its first read, answers every read `notLeader` with the new config and
# `meta.removed` in well under FOLLOWER_WAIT, the replacement serves the value, and the control's reads serve it.
#   bash tests/f113bed/run.sh             # build, serve, drive, stop
#   OUT=dir bash tests/f113bed/run.sh     # reuse a site build in dir/out
# Needs worker-build and tests/m0's wrangler. Run from the repository root.
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd); root=$(cd "$here/../.." && pwd)
P=${P:-8797}; W=${WRANGLER:-$root/tests/m0/node_modules/.bin/wrangler}
[ -x "$W" ] || { echo "no wrangler at $W (npm ci in tests/m0, or WRANGLER=)"; exit 1; }
B=${OUT:-$(mktemp -d)}; cp "$here/driver.mjs" "$B/"
if [ ! -f "$B/out/index.js" ]; then
  (cd "$root/workers/site" && worker-build --release --out-dir "$B/out" --features test-faults) >"$B/build.log" 2>&1 \
    || { tail -20 "$B/build.log"; exit 1; }
fi
python3 - "$B" "$root" <<'PY'
import json, sys
b, root = sys.argv[1], sys.argv[2]
sites = json.load(open(f"{root}/tests/m3/finish/sites4.json"))[:3]
for s in sites:
    s["script"] = f"f113bed-{s['id']}"
classes = [("MEMBER", "Member"), ("REGISTRY", "Registry"), ("SHARD", "GitShard"), ("HOOKSEQ", "HookSequencer"),
           ("SEARCH", "SearchShard"), ("INBOX", "Inbox"), ("LOGRELAY", "LogRelay"), ("BROKER", "RunnerBroker")]
for s in sites:
    me = s["id"]
    json.dump({
        "name": s["script"], "main": f"{b}/driver.mjs", "compatibility_date": "2026-10-01",
        "vars": {"SITE_ID": str(me), "GRAFTYARD_SITES": json.dumps(sites), "PING_MS": "40"},
        "durable_objects": {"bindings": [
            {"name": f"{n}_{t['id']}", "class_name": c, **({} if t["id"] == me else {"script_name": t["script"]})}
            for n, c in classes for t in sites]},
        "r2_buckets": [{"binding": f"BUCKET_{t['id']}", "bucket_name": f"f113bed-{t['id']}"} for t in sites],
        "queues": {"producers": [{"binding": "PRSYNC", "queue": f"f113bed-q{me}"}]},
        "migrations": [{"tag": "v1", "new_sqlite_classes": [c for _, c in classes]}],
    }, open(f"{b}/wrangler{me}.json", "w"))
PY
setsid "$W" dev --port $P -c "$B/wrangler0.json" -c "$B/wrangler1.json" -c "$B/wrangler2.json" --persist-to "$B/state" \
  >"$B/dev.log" 2>&1 & DEV=$!
trap 's=$?; kill -- -$DEV 2>/dev/null; wait $DEV 2>/dev/null; [ -z "${OUT:-}" ] && rm -rf "$B"; exit $s' EXIT
for _ in $(seq 120); do grep -q "Ready on" "$B/dev.log" && break; sleep 1; done
u() { curl -s -m 120 "http://127.0.0.1:$P$1"; }
r=$(u "/drive?chain=repo:900201&replaced=1"); echo "replaced: $r"
c=$(u "/drive?chain=repo:900202"); echo "control: $c"
python3 - "$r" "$c" <<'PY'
import json, sys
r, c = json.loads(sys.argv[1]), json.loads(sys.argv[2])
ok = True
def fail(m):
    global ok
    print("FAIL:", m); ok = False
if r.get("role") != "leader" or r.get("put") != 200:
    fail("no leader or the put failed")
if (r.get("replace") or {}).get("status") != 200 or (r["replace"].get("body") or {}).get("old") is None:
    fail(f"the replacement did not go in: {r.get('replace')}")
if not r.get("voter1") or r["voter1"].endswith("#1#f1"):
    fail(f"site 1's voter is still the first generation: {r.get('voter1')}")
reads = r.get("reads") or []
# The reads before the member learned it was replaced wait FOLLOWER_WAIT as before; every one from the first `removed`
# answer on is answered from what it learned, within 12 s of the first read (two SUPERSEDED_CHECK_MS and a wait).
first = next((i for i, x in enumerate(reads) if x["nl"]), None)
later = reads[first:] if first is not None else []
if first is None or sum(x["ms"] + 300 for x in reads[:first]) > 12_000:
    fail(f"the replaced member never answered removed within 12 s: {len(reads)} reads")
print("replaced member's reads (ms, notLeader, overloaded, removed):",
      [(x["ms"], x["nl"], x["overloaded"], x["removed"]) for x in reads])
if not later or not all(x["nl"] and x["nl"]["configIndex"] == r["configIndex"] and x["removed"] for x in later):
    fail("from its first, the replaced member did not answer notLeader with the group's config and meta.removed")
if any(x["overloaded"] or x["ms"] >= 500 for x in later):
    fail("a later read on the replaced member shed or took 500 ms or more")
if (r.get("removedReads") or 0) < len(later):
    fail(f"removedReads {r.get('removedReads')} under {len(later)}")
f = r.get("fresh") or {}
if f.get("value") != "v":
    fail(f"the replacement did not serve the read: {f}")
cr = c.get("reads") or []
print("control reads (ms, value):", [(x["ms"], x["value"]) for x in cr])
if not cr or not all(x["value"] == "v" and not x["nl"] and not x["overloaded"] for x in cr):
    fail("a live follower's read-index read did not serve the value")
if c.get("removedReads"):
    fail(f"the control's follower answered {c['removedReads']} reads as removed")
sys.exit(0 if ok else 1)
PY
ok=$?
n=$(grep -c 'out of bounds\|RuntimeError\|panicked' "$B/dev.log"); echo "wasm traps: $n"; [ "$n" = 0 ] || ok=1
[ $ok = 0 ] && echo PASS || { echo FAIL; exit 1; }
