#!/usr/bin/env bash
# R55 in local workerd (`decisions/OPEN-QUESTIONS.md`, R55): a leader whose peer refuses every AppendEntries goes quiet
# once its alarm stops. Three site scripts, the site's own build (workers/site, test-faults), bound to each other's
# classes as f113bed binds them. The driver joins one register group on sites 0 and 2 and, in the `refuse` arm, not
# on site 1, so site 1's member answers each AppendEntries 409 "member not joined", as a half-made chain's unjoined
# member did on graft-m6a. It elects site 0, commits a put, waits 75 s for the drain-check alarm to stop, then reads
# whether the leader's object was evicted (a new `boot`) and counts its calls to site 1 over 15 s. Exits 1 unless the
# `refuse` arm's leader went idle as the `joined` control's does (evicted, or at most 2 calls in the window; main's
# stayed resident and leading and called once every CALL_DEADLINE, 15 in 15 s), and both arms commit a second put.
#   bash tests/quietbed/run.sh             # build, serve, drive, stop
#   OUT=dir bash tests/quietbed/run.sh     # reuse a site build in dir/out
# Needs worker-build and tests/m0's wrangler. Run from the repository root. Takes about 3 minutes after the build.
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd); root=$(cd "$here/../.." && pwd)
P=${P:-8798}; W=${WRANGLER:-$root/tests/m0/node_modules/.bin/wrangler}
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
    s["script"] = f"quietbed-{s['id']}"
classes = [("MEMBER", "Member"), ("REGISTRY", "Registry"), ("SHARD", "GitShard"), ("HOOKSEQ", "HookSequencer"),
           ("SEARCH", "SearchShard"), ("INBOX", "Inbox"), ("LOGRELAY", "LogRelay"), ("BROKER", "RunnerBroker")]
for s in sites:
    me = s["id"]
    json.dump({
        "name": s["script"], "main": f"{b}/driver.mjs", "compatibility_date": "2026-10-01",
        "vars": {"SITE_ID": str(me), "GRAFTYARD_SITES": json.dumps(sites)},
        "durable_objects": {"bindings": [
            {"name": f"{n}_{t['id']}", "class_name": c, **({} if t["id"] == me else {"script_name": t["script"]})}
            for n, c in classes for t in sites]},
        "r2_buckets": [{"binding": f"BUCKET_{t['id']}", "bucket_name": f"quietbed-{t['id']}"} for t in sites],
        "queues": {"producers": [{"binding": "PRSYNC", "queue": f"quietbed-q{me}"}]},
        "migrations": [{"tag": "v1", "new_sqlite_classes": [c for _, c in classes]}],
    }, open(f"{b}/wrangler{me}.json", "w"))
PY
rm -rf "$B/state"
setsid "$W" dev --port $P -c "$B/wrangler0.json" -c "$B/wrangler1.json" -c "$B/wrangler2.json" --persist-to "$B/state" \
  >"$B/dev.log" 2>&1 & DEV=$!
trap 's=$?; kill -- -$DEV 2>/dev/null; wait $DEV 2>/dev/null; [ -z "${OUT:-}" ] && rm -rf "$B"; exit $s' EXIT
for _ in $(seq 120); do grep -q "Ready on" "$B/dev.log" && break; sleep 1; done
u() { curl -s -m 200 "http://127.0.0.1:$P$1"; }
# Both arms at once: each waits out its own 90 s.
u "/drive?chain=repo:900301&refuse=1" >"$B/refuse.json" & R=$!
u "/drive?chain=repo:900302" >"$B/joined.json" & J=$!
wait $R $J
r=$(cat "$B/refuse.json"); j=$(cat "$B/joined.json"); echo "refuse: $r"; echo "joined: $j"
python3 - "$r" "$j" <<'PY'
import json, sys
r, j = json.loads(sys.argv[1]), json.loads(sys.argv[2])
ok = True
def fail(m):
    global ok
    print("FAIL:", m); ok = False
for arm, x in (("refuse", r), ("joined", j)):
    if x.get("role") != "leader" or x.get("put") != 200 or x.get("put2") != 200 or not x.get("put2Done"):
        fail(f"{arm}: no leader, or a put did not commit: {x}")
if not r.get("site1Unjoined"):
    fail("the refuse arm's site-1 member joined")
if not j.get("evicted"):
    fail("the joined control's leader was not evicted while idle, so eviction cannot tell the arms apart")
# Quiet is the control's state: evicted while idle. A leader still resident must at least not call the peer.
if not r.get("evicted") and r.get("windowCalls", 99) > 2:
    fail(f"the leader stayed resident and called the refusing peer {r.get('windowCalls')} times in 15 s with no "
         "input and no alarm")
sys.exit(0 if ok else 1)
PY
ok=$?
n=$(grep -c 'out of bounds\|RuntimeError\|panicked' "$B/dev.log"); echo "wasm traps: $n"; [ "$n" = 0 ] || ok=1
[ $ok = 0 ] && echo PASS || { echo FAIL; exit 1; }
