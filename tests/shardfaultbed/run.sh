#!/usr/bin/env bash
# BM-6's container half of BM-1's site fault in local workerd (`build/BUILD-PLAN.md`, M6-WP4: "one site's member and
# container classes fail for a day"): a `GitShard` on the faulted site answers 503 while the plan names its whole
# site, and only then. Three site scripts, the site's own build (workers/site, test-faults), bound to each other's
# classes as quietbed binds them. The driver (`driver.mjs`) sets the plan on every site's `faults` registry and reads a
# shard's `/status` on sites 0 and 1 and a member's `/ping` on site 0 under four plans. Exits 1 unless site 0's shard
# answers 503 under the whole-site plan, while site 1's answers 200 and site 0's member fails too, and 200 under a
# `group_only` plan, a chain-scoped plan and no plan. On main the shard answers 200 under every plan: only the member
# class honoured the fault.
#   bash tests/shardfaultbed/run.sh            # build, serve, drive, stop
#   OUT=dir bash tests/shardfaultbed/run.sh    # reuse a site build in dir/out
# Needs worker-build and tests/m0's wrangler. Run from the repository root. Takes under a minute after the build.
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd); root=$(cd "$here/../.." && pwd)
P=${P:-8799}; W=${WRANGLER:-$root/tests/m0/node_modules/.bin/wrangler}
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
    s["script"] = f"shardfaultbed-{s['id']}"
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
        "r2_buckets": [{"binding": f"BUCKET_{t['id']}", "bucket_name": f"shardfaultbed-{t['id']}"} for t in sites],
        "queues": {"producers": [{"binding": "PRSYNC", "queue": f"shardfaultbed-q{me}"}]},
        "migrations": [{"tag": "v1", "new_sqlite_classes": [c for _, c in classes]}],
    }, open(f"{b}/wrangler{me}.json", "w"))
PY
rm -rf "$B/state"
setsid "$W" dev --port $P -c "$B/wrangler0.json" -c "$B/wrangler1.json" -c "$B/wrangler2.json" --persist-to "$B/state" \
  >"$B/dev.log" 2>&1 & DEV=$!
trap 's=$?; kill -- -$DEV 2>/dev/null; wait $DEV 2>/dev/null; [ -z "${OUT:-}" ] && rm -rf "$B"; exit $s' EXIT
for _ in $(seq 120); do grep -q "Ready on" "$B/dev.log" && break; sleep 1; done
r=$(curl -s -m 120 "http://127.0.0.1:$P/drive"); echo "drive: $r"
python3 - "$r" <<'PY'
import json, sys
try:
    r = json.loads(sys.argv[1])
except ValueError:
    print("FAIL: no drive answer"); sys.exit(1)
want = {"whole": (503, 200, 503), "group": (200, 200, 200), "chains": (200, 200, 200), "off": (200, 200, 200)}
ok = True
for arm, (s0, s1, p0) in want.items():
    got = r.get(arm, {})
    if (got.get("shard0"), got.get("shard1"), got.get("ping0")) != (s0, s1, p0):
        print(f"FAIL {arm}: want shard0 {s0}, shard1 {s1}, ping0 {p0}; got {got}"); ok = False
sys.exit(0 if ok else 1)
PY
ok=$?
n=$(grep -c 'out of bounds\|RuntimeError\|panicked' "$B/dev.log"); echo "wasm traps: $n"; [ "$n" = 0 ] || ok=1
[ $ok = 0 ] && echo PASS || { echo FAIL; exit 1; }
