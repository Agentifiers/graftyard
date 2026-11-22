#!/usr/bin/env bash
# The change feed end to end in local workerd, on the site script's own build (workers/site, test-faults): one script
# runs as three scripts, one per site, bound to each other's classes as deploy.mjs binds them, with a local EXPORT
# bucket on site 0, local PRSYNC queues and GRAFTYARD_EXPORT=1. `driver.mjs` joins one
# test-register group and commits a put; then the leader's report (`Host::report_commit` to the registry object
# `export`), the cron's round claim (`export_feed::kick`, through wrangler's /__scheduled) and the round's object in
# the bucket are each checked. A second group commits, then every member resets and it never elects again: the round
# must carry it by its probe (the sweep, `decisions/DECISIONS.md`, `export-host`), as m3fx49b's `release` was not. Exits 1 unless the round wrote one `export/` object and the state, a queued round followed
# it, and workerd logged no feed error or undecodable queue message. m3fx49's reports all failed on SQL no native test ran (`commit` as
# a column), and m3fx49b's queued next round arrived as `{}`; this is the binding-level check of that path, including
# the queued round that follows the cron's. Needs worker-build and tests/m0's wrangler. Run from the
# repository root.
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd); root=$(cd "$here/../.." && pwd)
P=${P:-8794}; W=${WRANGLER:-$root/tests/m0/node_modules/.bin/wrangler}
[ -x "$W" ] || { echo "no wrangler at $W (npm ci in tests/m0, or WRANGLER=)"; exit 1; }
B=$(mktemp -d); cp "$here/driver.mjs" "$B/"
(cd "$root/workers/site" && worker-build --release --out-dir "$B/out" --features test-faults) >"$B/build.log" 2>&1 \
  || { tail -20 "$B/build.log"; exit 1; }
python3 - "$B" "$root" <<'PY'
import json, sys
b, root = sys.argv[1], sys.argv[2]
sites = json.load(open(f"{root}/tests/m3/finish/sites4.json"))[:3]
for s in sites:
    s["script"] = f"exportbed-{s['id']}"
classes = [("MEMBER", "Member"), ("REGISTRY", "Registry"), ("SHARD", "GitShard"), ("HOOKSEQ", "HookSequencer"),
           ("SEARCH", "SearchShard"), ("INBOX", "Inbox"), ("LOGRELAY", "LogRelay"), ("BROKER", "RunnerBroker")]
# One script per site, as deploy.mjs builds them: each binds its own classes and the other sites' by `script_name`.
# Site 0 is the feed site: the EXPORT bucket and the driver's routes. wrangler dev runs the first config's script as
# the one it serves, and the others beside it.
for s in sites:
    me = s["id"]
    json.dump({
        "name": s["script"], "main": f"{b}/driver.mjs", "compatibility_date": "2026-10-01",
        "vars": {"SITE_ID": str(me), "GRAFTYARD_SITES": json.dumps(sites), "GRAFTYARD_EXPORT": "1"},
        "durable_objects": {"bindings": [
            {"name": f"{n}_{t['id']}", "class_name": c, **({} if t["id"] == me else {"script_name": t["script"]})}
            for n, c in classes for t in sites]},
        "r2_buckets": [{"binding": f"BUCKET_{t['id']}", "bucket_name": f"exportbed-{t['id']}"} for t in sites]
                      + ([{"binding": "EXPORT", "bucket_name": "exportbed-export"}] if me == 0 else []),
        "queues": {"producers": [{"binding": "PRSYNC", "queue": f"exportbed-q{me}"}],
                   "consumers": [{"queue": f"exportbed-q{me}", "max_batch_size": 10, "max_batch_timeout": 0}]},
        "triggers": {"crons": ["*/15 * * * *"]},
        "migrations": [{"tag": "v1", "new_sqlite_classes": [c for _, c in classes]}],
    }, open(f"{b}/wrangler{me}.json", "w"))
PY
setsid "$W" dev --port $P -c "$B/wrangler0.json" -c "$B/wrangler1.json" -c "$B/wrangler2.json" --test-scheduled --persist-to "$B/state" >"$B/dev.log" 2>&1 & DEV=$!
trap 's=$?; kill -- -$DEV 2>/dev/null; wait $DEV 2>/dev/null; rm -rf "$B"; exit $s' EXIT
for _ in $(seq 120); do grep -q "Ready on" "$B/dev.log" && break; sleep 1; done
u() { curl -s -m 60 "http://127.0.0.1:$P$1"; }
ok=1
d=$(u "/drive?chain=repo:900001"); echo "drive: $d"
grep -q '"role":"leader"' <<<"$d" && grep -q '"put":200' <<<"$d" || { echo "FAIL: no leader or the put failed"; ok=0; }
# A second group whose leader never reports: it commits, then every member resets at once, before the leader's alarm
# (DRAIN_CHECK_MS, 2 s) could report, and nothing calls it again, so it never elects. `repo:900099` is in
# `MemberRegistry` shard 0, the sweep's first slot (site 0's), so the first round probes it (`export::sweep_slots`).
d2=$(u "/drive?chain=repo:900099"); a2=$(u "/abort?chain=repo:900099"); echo "drive idle: $d2 abort: $a2"
grep -q '"put":200' <<<"$d2" || { echo "FAIL: the idle group's put failed"; ok=0; }
# The leader reports from its alarm, beside its tick: give it a few ticks.
sleep 5
u "/__scheduled?cron=*/15+*+*+*+*" >/dev/null; sleep 5
objs=$(u "/objects?prefix=export/"); state=$(u "/objects?prefix=export-state")
roles=$(u "/roles?chain=repo:900099"); batches=$(u "/batches")
echo "objects: $objs"; echo "state: $state"; echo "idle roles: $roles"; echo "batches: $batches"
python3 - "$objs" "$roles" "$batches" <<'PY' || ok=0
import json, sys
o = [x for x in json.loads(sys.argv[1]) if x["key"].startswith("export/")]
roles, batches = json.loads(sys.argv[2]), json.loads(sys.argv[3])
print(f"export objects: {len(o)}")
ok = len(o) == 1 and o[0]["size"] > 0
# The idle group stayed leaderless, and the round carried it anyway, each entry once, to its highest member commit.
if any(r["role"] == "leader" for r in roles):
    print("FAIL: repo:900099 elected a leader, so the control does not hold"); ok = False
top = max(r["commit"] or 0 for r in roles)
got = [(e[1], e[2]) for b in batches for e in b["entries"] if e[0] == "repo:900099"]
print(f"repo:900099 commit {top}, carried {got}")
n = got[0][1] if len(got) == 1 and got[0][0] == 1 else 0
if not 2 <= n <= top:
    print("FAIL: the leaderless chain is not in the feed exactly once, 1..commit"); ok = False
sys.exit(0 if ok else 1)
PY
# The round probed: `Release` and `Ids` have no group here, so only the sweep slot's chains answer.
grep -qE "export feed .*probed=[1-9]" "$B/dev.log" || { echo "FAIL: the round probed no chain"; ok=0; }
[ "$state" != "[]" ] || { echo "FAIL: no feed state written"; ok=0; }
# The round sends the next one as a queue message delayed FEED_EVERY_MS (10 s); with no new commit it logs `idle`.
# m3fx49b's message arrived as `{}` and the consumer read it as a PrSync batch, so no round followed the cron's.
# Each queued round is due FEED_EVERY_MS after the last one's claim (`export::next_round_delay_s`, R37), so idle rounds,
# which take well under a second here, start about 10 s apart: timed as their log lines appear.
end=$(( $(date +%s) + 35 )); seen=0; times=()
while [ "$(date +%s)" -lt "$end" ]; do
  n=$(grep -c "export feed idle" "$B/dev.log")
  while [ "$n" -gt "$seen" ]; do seen=$((seen + 1)); times+=("$(date +%s.%N)"); done
  sleep 0.2
done
[ "$seen" -ge 1 ] || { echo "FAIL: no queued round followed the cron's"; ok=0; }
if [ "$seen" -ge 2 ]; then
  gaps=$(python3 -c "import sys; t=[float(x) for x in sys.argv[1:]]; print(' '.join(f'{b-a:.1f}' for a,b in zip(t,t[1:])))" "${times[@]}")
  echo "idle rounds $seen, gaps s: $gaps"
  python3 -c "import sys; sys.exit(0 if all(9.0 <= float(g) <= 12.5 for g in sys.argv[1:]) else 1)" $gaps \
    || { echo "FAIL: queued rounds are not FEED_EVERY_MS apart"; ok=0; }
else
  echo "FAIL: fewer than two queued rounds in 35 s"; ok=0
fi
grep -E "export (feed|report)|does not decode" "$B/dev.log" | sed 's/^/log: /' | head -8
if grep -qE "export report .*Error|round claim failed|export feed round failed|does not decode" "$B/dev.log"; then
  echo "FAIL: the feed logged an error"; ok=0
fi
n=$(grep -c 'out of bounds\|RuntimeError' "$B/dev.log"); echo "wasm traps: $n"; [ "$n" = 0 ] || ok=0
[ $ok = 1 ] && echo PASS || { echo FAIL; exit 1; }
