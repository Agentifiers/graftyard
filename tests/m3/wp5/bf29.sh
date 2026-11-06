#!/bin/bash
# BF-29 hosted: a follower kept offline past the archive boundary on a real repository rejoins by InstallSnapshot.
#   bf29.sh REPO_ID DOWN_SITE [PUSHES]
# 1. Site 0 leads. DOWN_SITE's member is held down (`/bench_seed {"snap":"hold"}` keeps its snapshot work off, and the
#    edge's site fault makes its member class fail every call), so it misses every entry from here.
# 2. PUSHES single-ref pushes through the edge commit while it is down; the other follower snapshots both tiers
#    (forced), and the edge archives 1..=commit (`/_bed/archive`), after which the leader and the up follower truncate
#    their stored logs and the leader names an install source.
# 3. More pushes, then the fault is cleared. The leader's next round finds DOWN_SITE's next entry archived and sends
#    `InstallSnapshot`; the member loads the snapshot and archives in batches and installs.
# 4. Pass: the member reports one install, its commit reaches the leader's, and at the last index all three applied
#    every tier's `fold_digest` and the `request` rows at or below it are equal on all three members.
# Env: GRAFT_EDGE, GRAFT_TOOLS_EDGE, BED_TOKEN_FILE, BF20_HOME (tokens.txt). Log: $OUT (default /tmp/m3h/bf29).
set -uo pipefail
ID=${1:?repo id} D=${2:?down site} N=${3:-60}
OUT=${OUT:-/tmp/m3h/bf29}; mkdir -p "$OUT"
HERE=$(cd "$(dirname "$0")" && pwd)
M=$HERE/../../m2/bf20/m.sh
export CH=repo:$ID REPO=$ID
BED=$(cat "$BED_TOKEN_FILE")
EDGE=${GRAFT_EDGE:?}
UP=$(( D == 1 ? 2 : 1 ))
log() { echo "$(date -u +%FT%TZ) $*" | tee -a "$OUT/bf29.log"; }
bed() { curl -sS -m 900 -X POST -H "x-bed-token: $BED" "$EDGE/_bed/$1${2:-}"; }
st() { $M "$1" /status | python3 -c "import json,sys; v=json.load(sys.stdin)['v']; print(v.get('role'), v.get('commit'), v.get('applied'), v.get('base'), v.get('last'))"; }
# n real git pushes through the edge, each one commit on `main` (a promotion needs the refs' objects, so the edge's
# `/_bed/push` of made-up ids will not do).
GIT=$OUT/git; WRITER_TOKEN=$(sed -n 2p "$BF20_HOME/tokens.txt" | cut -d' ' -f1)
NAME=$(python3 -c "import json; print({v: k for k, v in json.load(open('$BF20_HOME/repos.json')).items()}[$ID])")
URL=https://x:$WRITER_TOKEN@${EDGE#https://}/m2b/$NAME.git
if [ ! -d "$GIT" ]; then
  git init -q -b main "$GIT" && git -C "$GIT" -c user.name=bf29 -c user.email=bf29@m2b commit -q --allow-empty -m root
  git -C "$GIT" fetch -q "$URL" main 2>/dev/null && git -C "$GIT" reset -q --hard FETCH_HEAD
fi
push() {
  for k in $(seq 1 "$1"); do
    echo "$2-$k $(date +%s%N)" > "$GIT/f-$2"
    git -C "$GIT" add -A && git -C "$GIT" -c user.name=bf29 -c user.email=bf29@m2b commit -q -m "$2-$k"
    for t in 1 2 3; do timeout 120 git -C "$GIT" push -q "$URL" HEAD:refs/heads/main > "$OUT/push.last" 2>&1 && break; sleep 2; done
  done
}
RUN=$(date +%s)
log "run $RUN repo $ID down $D up $UP pushes $N"
for s in 0 1 2; do log "status $s: $(st $s)"; done
for t in 1 2 3 4 5; do $M 0 /campaign '{}' | grep -q '"verdict":"\(leader\|resumed\|won\)"' && break; sleep 3; done
log "campaign 0: $(st 0)"
push 3 warm
log "fault on site $D: $(bed fault "?site=$D")"
sleep 3
push "$N" a
C1=$(st 0 | cut -d' ' -f2); log "after first pushes: leader commit $C1; down: $(st $D)"
# The up follower snapshots both tiers at its applied index.
A1=$(st $UP | cut -d' ' -f3)
for t in 0 1; do log "force tier $t on $UP: $($M $UP /bench_seed "{\"snap\":\"release\"}") $($M $UP /bench_seed "{\"force\":$t}")"; done
# Both tiers' snapshots committed past the first pushes, as the leader's fold records them (a push wakes the writer,
# whose alarm runs only after it applies an entry).
for k in $(seq 1 60); do
  F=$($M 0 /fold '{}'); echo "$F" > "$OUT/fold-leader.json"
  DONE=$(echo "$F" | python3 -c "
import json,sys; v=json.load(sys.stdin)['v']
print(sum(1 for t in ('core','metadata') if any(s['to'] >= $A1 for s in v[t]['snapshots'])))")
  [ "$DONE" = 2 ] && break; push 1 wake$k; sleep 5
done
log "snapshots on the leader: $(echo "$F" | python3 -c "import json,sys; v=json.load(sys.stdin)['v']; print([(t, [s['to'] for s in v[t]['snapshots']]) for t in ('core','metadata')])")"
TO=$(st 0 | cut -d' ' -f2)
A=$(bed archive "?repo=$ID&from=1&to=$TO"); echo "$A" > "$OUT/archive.json"
log "archive 1..$TO: $(echo "$A" | head -c 600)"
sleep 15
push 10 b
for s in 0 $UP; do log "after archive $s: $(st $s) truncatedTo $($M $s /stats | python3 -c "import json,sys; print(json.load(sys.stdin)['v']['truncatedTo'])")"; done
log "fault off: $(bed fault '?off=1')"
T0=$(date +%s)
for k in $(seq 1 120); do
  push 1 c$k
  R=$($M $D /fold '{"audit":{}}' 2>/dev/null)
  S=$(st $D); L=$(st 0)
  echo "$(date -u +%T) down=$S leader=$L" >> "$OUT/wait.log"
  # Caught up within the push just sent, and its log starts past the archive: it installed.
  [ "$(echo "$S" | cut -d' ' -f2)" -ge $(( $(echo "$L" | cut -d' ' -f2) - 1 )) ] && [ "$(echo "$S" | cut -d' ' -f4)" -ge "$TO" ] && break
  sleep 5
done
log "rejoined after $(( $(date +%s) - T0 )) s: down $(st $D) leader $(st 0)"
$M $D /stats > "$OUT/stats-down.json"
log "installs on $D: $(python3 -c "import json; print(json.load(open('$OUT/stats-down.json'))['v']['installs'])") alarms: $(python3 -c "import json; print(json.load(open('$OUT/stats-down.json'))['v']['alarms'][-5:])")"
$M $D /fold '{"audit":{}}' > "$OUT/load-report.json"
log "load report: $(python3 -c "import json; v=json.load(open('$OUT/load-report.json'))['v']['last']; print({k: v.get(k) for k in ['kind','installed','batches','batchMaxMs','bytes','rows','entries','ms','memMaxMb','requestRows','core','metadata']})")"
sleep 5
AT=$(for s in 0 1 2; do st $s | cut -d' ' -f3; done | sort -n | head -1)
log "compare at $AT"
for s in 0 1 2; do
  $M $s /fold "{\"at\":$AT}" > "$OUT/fold-$s.json"
  $M $s /fold "{\"requestsAt\":$AT}" > "$OUT/req-$s.json"
  log "site $s: $(python3 -c "
import json; v=json.load(open('$OUT/fold-$s.json'))['v']; r=json.load(open('$OUT/req-$s.json'))['v']
print('core', v['core']['at'], 'metadata', v['metadata']['at'], 'requests', r['rows'], r['digest'][:12])")"
done
python3 - "$OUT" <<'EOF'
import json, sys
o = sys.argv[1]
f = [json.load(open(f"{o}/fold-{s}.json"))["v"] for s in range(3)]
r = [json.load(open(f"{o}/req-{s}.json"))["v"] for s in range(3)]
same = lambda xs: len(set(xs)) == 1 and None not in xs
ok = same([x["core"]["at"] for x in f]) and same([x["metadata"]["at"] for x in f]) and same([x["digest"] for x in r])
print("VERDICT", "PASS" if ok else "FAIL")
EOF
