#!/usr/bin/env bash
# F112's bed: what a write costs on a cold edge isolate, in local workerd on the edge's and the site's own builds
# (test-faults). One `wrangler dev` runs the three site scripts (`site.mjs`, each member call held CALL_MS per site
# as a stand-in for its region's round trip), EDGES edge scripts of the same edge build bound to them as
# tests/m1/integ/deploy-edge.mjs binds graft-m1-edge (FORMATS_ENABLED with 32.8, as graft-m1's edge has), and a front
# (`driver.mjs`) that routes `x-cw-edge: N` to edge script N. Edge 0 seeds (`seed.mjs`); every other edge script has
# served nothing, so its first request is a cold isolate's. Then `push.mjs` sends receive-pack's advertisement and
# POST on each cold edge, then again on it warm, and prints each request's server-timing and member calls.
# No git cache shard runs here, so a POST ends at validate (503); every stage before it is the product's.
#   bash tests/coldwrite/run.sh            # build, serve, seed, measure, stop
#   KEEP=1 bash tests/coldwrite/run.sh     # leave the bed up (prints its port and dir)
# Needs worker-build and tests/m0's wrangler. Run from the repository root.
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd); root=$(cd "$here/../.." && pwd)
P=${P:-8796}; W=${WRANGLER:-$root/tests/m0/node_modules/.bin/wrangler}; EDGES=${EDGES:-4}
CALL_MS=${CALL_MS:-60,90,150}
[ -x "$W" ] || { echo "no wrangler at $W"; exit 1; }
B=${B:-$(mktemp -d)}; cp "$here"/site.mjs "$here"/driver.mjs "$B/"
if [ ! -f "$B/site/index.js" ]; then
  (cd "$root/workers/site" && worker-build --release --out-dir "$B/site" --features test-faults) >"$B/site.log" 2>&1 \
    || { tail -20 "$B/site.log"; exit 1; }
fi
if [ ! -f "$B/edge/index.js" ]; then
  (cd "$root/workers/edge" && worker-build --release --out-dir "$B/edge" --features test-faults) >"$B/edge.log" 2>&1 \
    || { tail -20 "$B/edge.log"; exit 1; }
fi
ln -sfn "$B/site" "$B/out"
python3 - "$B" "$root" "$EDGES" "$CALL_MS" <<'PY'
import json, sys
b, root, edges, call_ms = sys.argv[1], sys.argv[2], int(sys.argv[3]), sys.argv[4].split(",")
sites = json.load(open(f"{root}/tests/m3/finish/sites4.json"))[:3]
for s in sites:
    s["script"] = f"cw-site-{s['id']}"
    s["bucket"] = f"cw-packs-{s['id']}"
genesis = json.load(open(f"{root}/tests/m1/wp7/genesis.json"))["placements"]
classes = [("MEMBER", "Member"), ("REGISTRY", "Registry"), ("SHARD", "GitShard"), ("HOOKSEQ", "HookSequencer"),
           ("SEARCH", "SearchShard"), ("INBOX", "Inbox"), ("LOGRELAY", "LogRelay"), ("BROKER", "RunnerBroker")]
buckets = [{"binding": f"BUCKET_{t['id']}", "bucket_name": t["bucket"]} for t in sites]
formats = "32.1,32.4,32.5,32.6,32.7,32.9,16.2,17.2,18.2,20.1,32.10,18.4,32.8,32.12,32.14,18.3"
json.dump({"name": "cw-front", "main": f"{b}/driver.mjs", "compatibility_date": "2026-10-01",
           "services": [{"binding": "SITE0", "service": "cw-site-0"}]
                       + [{"binding": f"EDGE{n}", "service": f"cw-edge-{n}"} for n in range(edges)]},
          open(f"{b}/w-front.json", "w"))
for s in sites:
    me = s["id"]
    json.dump({
        "name": s["script"], "main": f"{b}/site.mjs", "compatibility_date": "2026-10-01",
        "vars": {"SITE_ID": str(me), "GRAFTYARD_SITES": json.dumps(sites), "GRAFTYARD_GENESIS": json.dumps(genesis),
                 "CALL_MS": call_ms[me]},
        "durable_objects": {"bindings": [
            {"name": f"{n}_{t['id']}", "class_name": c, **({} if t["id"] == me else {"script_name": t["script"]})}
            for n, c in classes for t in sites]},
        "r2_buckets": buckets,
        "migrations": [{"tag": "v1", "new_sqlite_classes": [c for _, c in classes]}],
    }, open(f"{b}/w-site{me}.json", "w"))
for n in range(edges):
    json.dump({
        "name": f"cw-edge-{n}", "main": f"{b}/edge/index.js", "compatibility_date": "2026-10-01",
        "vars": {"PLAN": "pro", "SSH_OPTION": "off", "EDGE_CAP_BYTES": "80000000", "FORMATS_ENABLED": formats,
                 "PART_BYTES": "5242880", "HOST": "cw.example", "GRAFTYARD_SITES": json.dumps(sites),
                 "GRAFTYARD_GENESIS": json.dumps(genesis), "LOGINS": "{}"},
        "durable_objects": {"bindings": [{"name": f"{c}_{t['id']}", "class_name": k, "script_name": t["script"]}
                                         for c, k in classes for t in sites]},
        "r2_buckets": buckets,
    }, open(f"{b}/w-edge{n}.json", "w"))
open(f"{b}/.dev.vars", "w").write("BED_TOKEN=cw-bed\n")
PY
cfgs=(-c "$B/w-front.json" -c "$B/w-site0.json" -c "$B/w-site1.json" -c "$B/w-site2.json")
for n in $(seq 0 $((EDGES - 1))); do cfgs+=(-c "$B/w-edge$n.json"); done
cd "$B" && { setsid "$W" dev --port $P "${cfgs[@]}" --persist-to "$B/state" >"$B/dev.log" 2>&1 & DEV=$!; cd - >/dev/null; }
stop() { kill -- -$DEV 2>/dev/null; wait $DEV 2>/dev/null; }
[ -z "${KEEP:-}" ] && trap 's=$?; stop; exit $s' EXIT
for _ in $(seq 120); do grep -q "Ready on" "$B/dev.log" && break; sleep 1; done
grep -q "Ready on" "$B/dev.log" || { tail -30 "$B/dev.log"; exit 1; }
echo "bed up on $P in $B"
F=http://127.0.0.1:$P
node "$here/seed.mjs" "$F" cw-bed "$B" || { tail -30 "$B/dev.log"; exit 1; }
[ -n "${KEEP:-}" ] && { echo "KEEP: dev pid $DEV"; exit 0; }
node "$here/push.mjs" "$F" "$B" "$EDGES"
