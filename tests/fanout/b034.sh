#!/bin/bash
# B0-34 as written on graft-m2b, for an orb service: the networks (b034setup.mjs, once), their seeds (b034seed.mjs,
# resumable; the monorepo is 5 GB of torvalds/linux, the others git/git), then a1.mjs b0-34 --b034 with the home shard
# killed at 300 s, the second network hot from 420 s and the hostile owner's three from 600 s, over 900 s. The repos
# are kept between rounds. Before the run, a static check that no request path can call the Cloudflare API.
#   EDGE_LOG=tests/am1/raw/runs/fanout-mNN-edge.log [SEED_ONLY=1] [LEARNERS=...] [PROCS_S=3600] B034_DIR=$HOME/b034 \
#     bash tests/fanout/b034.sh <tag>
# Needs $B034_DIR/linux.git and $B034_DIR/git.git (bare clones). Output: tests/fanout/raw/b034-<tag>.status.
set -uo pipefail
cd "$(dirname "$0")/../.."
source tests/m3/rows/env.sh
TAG=$1 D=${B034_DIR:-$HOME/b034}
ST=tests/fanout/raw/b034-$TAG.status
[ -e tests/fanout/raw/.b034-$TAG.started ] && exec sleep infinity
touch tests/fanout/raw/.b034-$TAG.started
s() { echo "$(date -u +%FT%TZ) $*" >> "$ST"; }
# No Cloudflare API call from a request path (B0-34's fail line): the only `api.cloudflare.com` in a request-serving
# crate would be a bug; the deploy and ops tools are not request paths.
n=$(rg -l "api\.cloudflare\.com" workers/edge/src workers/site/src crates --type rust | wc -l); s "api.cloudflare.com in request paths: $n files"
# Round 24c: the m36 site day's edge deploy stopped while compiling, and the rows ran on the previous edge. EDGE_LOG
# names the site day's edge log (tests/am1/raw/runs/fanout<TAG>-edge.log); the row stops unless that version serves.
if [ -n "${EDGE_LOG:-}" ]; then
  python3 tests/fanout/edgever.py "$EDGE_LOG" >> "$ST" 2>&1 || { s "b0-34 stopped: the edge is not the site day's"; echo DONE >> "$ST"; exec sleep infinity; }
else
  s "b0-34 stopped: EDGE_LOG (the site day's edge deploy log) is not set"; echo DONE >> "$ST"; exec sleep infinity
fi
node tests/fanout/b034setup.mjs >> "$ST" 2>&1 || { s "setup failed"; exec sleep infinity; }
J=$M3_STATE/b034.json
[ -s "$D/plan-git.txt" ] || python3 tests/fanout/b034plan.py "$D/git.git" "$D/plan-git.txt" 1e12 60e6 0 >> "$ST" 2>&1
# 30 MB steps: the edge buffers a push body in a growing Vec, so one over 64 MB reserved 128 MB and the isolate
# ran out of memory (rows m20, seed1: 6 of 6 tries of a 60 MB step).
[ -s "$D/plan.txt" ] || python3 tests/fanout/b034plan.py "$D/linux.git" "$D/plan.txt" 5e9 30e6 >> "$ST" 2>&1
seed() { node tests/fanout/b034seed.mjs "$1" "$2" "$3" "$4" > "tests/fanout/raw/b034-seed-${3//\//_}.log" 2>&1; s "seed $3 exit $?"; }
q() { python3 -c "import json,sys; d=json.load(open('$J')); print(eval(sys.argv[1]))" "$1"; }
seed "$D/git.git" "$D/plan-git.txt" "$(q "d['second']['repo']")" member
for i in 0 1 2; do seed "$D/git.git" "$D/plan-git.txt" "$(q "d['hostile'][$i]['repo']")" hostile; done
seed "$D/linux.git" "$D/plan.txt" "$(q "d['mono']['repo']")" admin
mono=$(q "d['mono']['repo'].split('/')[1]"); chain=$(q "d['mono']['chain']")
# SEED_ONLY=1: stop after the seeds (a later tag runs the row; the seeds and the repos are kept).
[ -n "${SEED_ONLY:-}" ] && { echo DONE >> "$ST"; exec sleep infinity; }
python3 - "$J" "$D" > tests/fanout/raw/b034-$TAG.json <<'PY'
import json, sys
d = json.load(open(sys.argv[1])); D = sys.argv[2]
fut = [l for l in open(f"{D}/plan.txt") if l.startswith("# future ")][0].split()[2:]
print(json.dumps({"bare": f"{D}/linux.git", "future": fut, "second": d["second"], "secondAt": 420,
                  "hostile": d["hostile"], "hostileAt": 600, "hotPushPerS": 1, "hotClonePerS": 3}))
PY
# The warm step (round 18): every site's shard of the monorepo caught up and ls-refs listing main before the window,
# at most WARM_MIN minutes (default 45); its time is not the row's. Past the limit the row is not run. It runs before
# learners.mjs and k.mjs, whose leader reads found an idle group with no leader (round 17b); the catch-ups wake it.
# Round 20's drain (stored-fetch): before the cold warm, the archive's promote-ahead promotes the network's thin stored
# pushes under the pack-bytes trigger, timed on its own (b034drain.mjs), at most DRAIN_MIN minutes in all (default 90).
# Then every shard is torn down (teardown.py --sweep), so the warm is cold against the promoted packs.
# Round 22: the drain's watcher reads only members, which a group does not count as clients, so repo:131296 idled out
# to followers 3 minutes in and its archive alarm stopped. One edge read a minute keeps it led through the drain.
if [ -n "${DRAIN:-}" ]; then
  node tests/fanout/keepled.mjs "$(q "d['mono']['repo']")" "${DRAIN_MIN:-90}" > tests/fanout/raw/keepled-$TAG.jsonl 2>&1 &
  KEEP=$!
  s "b0-34 drain start"
  node tests/fanout/b034drain.mjs "$chain" "$(q "d['mono']['repo']")" "${DRAIN_MIN:-90}" > tests/fanout/raw/b034drain-$TAG.jsonl 2>&1
  w=$?; kill $KEEP 2>/dev/null; s "b0-34 drain exit $w $(tail -1 tests/fanout/raw/b034drain-$TAG.jsonl)"
  [ $w -eq 0 ] || { echo DONE >> "$ST"; exec sleep infinity; }
  python3 tests/fanout/teardown.py --sweep >> "$ST" 2>&1; s "b0-34 shards torn down before the cold warm"
  # Round 23: the warm must rehydrate cold, so no shard of the network may run when it starts (exit 2 otherwise).
  python3 tests/fanout/netimages.py "${chain#repo:}" --none >> "$ST" 2>&1 || { s "b0-34 a shard of the network runs before the cold warm"; echo DONE >> "$ST"; exec sleep infinity; }
fi
s "b0-34 warm start"
# Round 25's probes: each shard's git processes (RSS, nice) and the network's kept packs, every 15 s from the warm to
# the row's end (tests/fanout/shardprocs.mjs). A shard at its memory cap is recorded as a finding, not chased.
node tests/fanout/shardprocs.mjs "${chain#repo:}" "${PROCS_S:-3600}" 15 > tests/fanout/raw/shardprocs-$TAG.jsonl 2>&1 &
node tests/fanout/b034warm.mjs "${chain#repo:}" "$(q "d['mono']['repo']")" "${WARM_MIN:-45}" > tests/fanout/raw/b034warm-$TAG.jsonl 2>&1
w=$?; s "b0-34 warm exit $w $(tail -1 tests/fanout/raw/b034warm-$TAG.jsonl)"
# Round 23: every shard the warm started runs the expected image (IMG, a sha256: prefix), recorded beside the rehydrates.
[ -n "${IMG:-}" ] && { python3 tests/fanout/netimages.py "${chain#repo:}" --image "$IMG" >> "$ST" 2>&1; s "b0-34 warm images exit $?"; }
[ $w -eq 0 ] || { echo DONE >> "$ST"; exec sleep infinity; }
# (b): after the cold warm, each running shard's kept packs against its rehydrate's idx_used (recorded, not a stop).
sleep 20; python3 - tests/fanout/raw/shardprocs-$TAG.jsonl >> "$ST" <<'PY'
import json, sys
last = {}
for l in open(sys.argv[1]):
    try: d = json.loads(l)
    except ValueError: continue
    if d.get("packs"): last[d["site"]] = d["packs"]
print("b0-34 kept packs after the warm: " + json.dumps({s: {"kept": p["kept"], "count": p["count"]} for s, p in sorted(last.items())}))
PY
[ -n "${LEARNERS:-}" ] && node tests/fanout/learners.mjs "$chain" "$LEARNERS" >> "$ST" 2>&1
node tests/fanout/k.mjs "$chain" >> "$ST" 2>&1
s "b0-34 $mono $chain start"
# The row's memory guard: 2 s samples of every voter, a1.mjs stopped past 120 MiB (round 17's instruments).
node tests/fanout/memguard.mjs "$chain" 1100 2 120 > tests/fanout/raw/memguard-b034$TAG.jsonl 2> tests/fanout/raw/memguard-b034$TAG.err &
node tests/fanout/terms.mjs "$chain" 1100 > tests/fanout/raw/terms-b034$TAG.jsonl 2>&1 &
bash tests/am1/runs/a1.sh b0-34 "$mono" "${chain#repo:}" --seconds 900 --kill-shard-at 300 --b034 tests/fanout/raw/b034-$TAG.json
s "b0-34 done $(grep -E '^exit' tests/am1/raw/runs/a1-b0-34-$mono.log)"
node tests/fanout/k.mjs "$chain" >> "$ST" 2>&1
echo DONE >> "$ST"
exec sleep infinity
