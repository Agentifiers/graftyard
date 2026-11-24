#!/usr/bin/env bash
# F115's storage-side question in local workerd (`decisions/OPEN-QUESTIONS.md`, F115): does a repository leader's
# per-entry SQLite read and write cost grow with its stored log? Three site scripts, the site's own build
# (workers/site, test-faults), bound as quietbed binds them, on real local SQLite. One register group grows its log
# through `/commit` (PAR puts in flight, each of a BYTES-character value) to each of STOPS stored entries. At each stop
# it times, from outside, three rounds each of:
# - `grow`: GROW more puts, each committed on three members (append, apply, the `request` row and its retention
#   delete), as ms an entry;
# - `entries`: K leader `/entries` reads of one stored entry each, spread over the whole stored log, as ms a read;
# - `request`: K `/outcome_by_request` lookups of a committed request row, spread over all of them, as ms a lookup;
# - `status`: K `/status` calls, the per-call cost the others share.
# Prints one JSON line per stop and round. A cost that tracks the stored log rises from stop to stop; a flat one
# does not.
#   bash tests/f115bed/run.sh                  # build, serve, measure, stop
#   OUT=dir bash tests/f115bed/run.sh          # reuse a site build in dir/out
#   STOPS="1000 10000 40000" BYTES=4096 bash tests/f115bed/run.sh
# Needs worker-build and tests/m0's wrangler. Run from the repository root. About 90 minutes after the build at the
# default stops, most of it growing the log: about 0.12 s an entry on three local members.
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd); root=$(cd "$here/../.." && pwd)
P=${P:-8799}; W=${WRANGLER:-$root/tests/m0/node_modules/.bin/wrangler}
STOPS=${STOPS:-"1000 10000 40000"}; BYTES=${BYTES:-4096}; GROW=${GROW:-300}; K=${K:-300}; PAR=${PAR:-1}
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
    s["script"] = f"f115bed-{s['id']}"
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
        "r2_buckets": [{"binding": f"BUCKET_{t['id']}", "bucket_name": f"f115bed-{t['id']}"} for t in sites],
        "queues": {"producers": [{"binding": "PRSYNC", "queue": f"f115bed-q{me}"}]},
        "migrations": [{"tag": "v1", "new_sqlite_classes": [c for _, c in classes]}],
    }, open(f"{b}/wrangler{me}.json", "w"))
PY
rm -rf "$B/state"
setsid "$W" dev --port $P -c "$B/wrangler0.json" -c "$B/wrangler1.json" -c "$B/wrangler2.json" --persist-to "$B/state" \
  >"$B/dev.log" 2>&1 & DEV=$!
trap 's=$?; kill -- -$DEV 2>/dev/null; wait $DEV 2>/dev/null; [ -z "${OUT:-}" ] && rm -rf "$B"; exit $s' EXIT
for _ in $(seq 120); do grep -q "Ready on" "$B/dev.log" && break; sleep 1; done
C=repo:900401
u() { curl -s -m 3000 "http://127.0.0.1:$P$1"; }
# ms for one driver route, and its answer.
timed() { local t0 t1 a; t0=$(date +%s%N); a=$(u "$1"); t1=$(date +%s%N); echo "$(( (t1 - t0) / 1000000 )) $a"; }
echo "setup: $(u "/setup?chain=$C")"
n=0
for stop in $STOPS; do
  # Grow until the stored log holds the stop's entries, PAR puts at a time, in batches of 500 puts.
  hi=$(u "/grow?chain=$C&from=$n&n=0" | python3 -c 'import json,sys; print(json.load(sys.stdin)["commit"])')
  while [ "$hi" -lt "$stop" ]; do
    g=$(u "/grow?chain=$C&from=$n&n=500&bytes=$BYTES&par=$PAR"); n=$((n + 500))
    grep -q '"ok":500,' <<<"$g" || { echo "FAIL: grow to $n puts: $g"; exit 1; }
    hi=$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["commit"])' "$g")
  done
  echo "grown: $n puts, $hi entries, $(du -sh "$B/state" | cut -f1) on disk"
  for round in 1 2 3; do
    read -r gms ga <<<"$(timed "/grow?chain=$C&from=$n&n=$GROW&bytes=$BYTES")"; n=$((n + GROW))
    read -r ems ea <<<"$(timed "/entries?chain=$C&k=$K&lo=2&hi=$hi")"
    read -r rms ra <<<"$(timed "/request?chain=$C&k=$K&lo=0&hi=$((n - 1))")"
    read -r sms sa <<<"$(timed "/status?chain=$C&k=$K")"
    python3 - "$stop" "$round" "$n" "$hi" "$GROW" "$K" "$gms" "$ems" "$rms" "$sms" "$ga" "$ea" "$ra" "$sa" <<'PY'
import json, sys
stop, rnd, n, hi, grow, k, gms, ems, rms, sms = map(int, sys.argv[1:11])
ga, ea, ra, sa = (json.loads(x) for x in sys.argv[11:15])
print(json.dumps({"stop": stop, "round": rnd, "puts": n, "stored": hi,
    "growMsPerEntry": round(gms / grow, 3), "entriesMsPerRead": round(ems / k, 3),
    "requestMsPerLookup": round(rms / k, 3), "statusMsPerCall": round(sms / k, 3),
    "ok": [ga["ok"], ea["ok"], ra["ok"], sa["ok"]]}))
PY
  done
done
n=$(grep -c 'out of bounds\|RuntimeError\|panicked' "$B/dev.log"); echo "wasm traps: $n"
du -sh "$B/state" | sed 's/^/state on disk: /'
