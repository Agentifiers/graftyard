#!/usr/bin/env bash
# BM-1 on Cloudflare: "The git conformance suite and git clone / fetch / push --atomic from unmodified git 2.4x pass
# with one site's member class failing mid-run." Each step runs once and leaves a `done` marker, so the orb service
# that runs this resumes after a reboot at the first unfinished step (a step cut by a reboot reruns from its start).
#
#   tests/m1/wp8/bm1.sh <out root> [fault site, default 0] [fault after s, default 120]
#
# Steps, against graft-m1-edge on the real groups (test install graft-m1-site-a/b/c):
#   1. t5541 on git 2.56.0 with no fault: the t5541 #20 edge fix, against the m1-wp6 no-fault baseline.
#   2. clients.sh on git 2.39.5 and 2.51.0 (the graft-git239 / graft-git251 images), each with the fault.
#   3. git's own t/ HTTP suite (conformance/run.sh) on 2.51.0, 2.56.0, and 2.39.5, each with the fault.
# Before every step the fault is cleared, the slots' shard containers are torn down, so each starts as the no-fault
# baseline did, and every watched group's leader is moved to the site the step fails (home.sh), so the fault moves
# each of them; observe.ts samples the groups' leaders throughout, and the members' commit latency is read
# after each step.
set -u
ROOT=${1:?out root}
mkdir -p "$ROOT"
SITE=${2:-0}
AFTER=${3:-120}
SRC=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$SRC/../../.." && pwd)
# bash reads a script as it runs, so an edit to a helper mid-run would corrupt that run: every step runs a copy taken
# when this run started (conformance/run.sh too).
HERE=$ROOT/bin
export GRAFT_REPO=$REPO
mkdir -p "$HERE/conformance"
cp "$SRC"/*.sh "$SRC"/observe.ts "$HERE/"
cp -r "$REPO/tests/m1/wp6/conformance/." "$HERE/conformance/"
H=https://graft-m1-edge.<workers-subdomain>.workers.dev
B=$(cat ~/.graft-wp6/bed.token)
log() { echo "$(date -u +%FT%TZ) $*" | tee -a "$ROOT/bm1.log"; }

reset() {
  bash "$HERE/fault.sh" off >>"$ROOT/bm1.log" 2>&1
  for r in 196608 196610; do curl -s -X POST -H "x-bed-token: $B" "$H/_bed/kill?repo=$r" >>"$ROOT/bm1.log"; done
  echo >>"$ROOT/bm1.log"
  sleep 5
  # Every watched group led from the site the step fails, so the fault moves each leader (home.sh).
  bash "$HERE/home.sh" "$SITE" >>"$ROOT/bm1.log" 2>&1
}

conformance() { # name, versions, fault?
  local name=$1 versions=$2 fault=$3 out=$ROOT/$1
  [ -f "$out/done" ] && return 0
  rm -rf "$out"; mkdir -p "$out"
  reset
  node "$HERE/observe.ts" --stats "$out/stats-before.jsonl" --reset >/dev/null
  node "$HERE/observe.ts" --out "$out/observe.jsonl" &
  local obs=$!
  local f=()
  [ "$fault" = 1 ] && f=(--fault-cmd "FAULT_STATS=$out/stats-at-fault.jsonl bash $HERE/fault.sh on $SITE"
    --fault-after "$AFTER" --restore-cmd "bash $HERE/fault.sh off")
  local tests=()
  [ "$name" = t5541-nofault ] && tests=(--tests t5541-http-push-smart)
  log "$name: start ($versions, fault=$fault)"
  bash "$HERE/conformance/run.sh" "$H" --git "$versions" "${tests[@]}" --tokens ~/.graft-integ/tokens.txt \
    --slots wp6/main,wp6/bf16 --out "$out/run" "${f[@]}" >"$out/run.log" 2>&1
  kill "$obs" 2>/dev/null
  node "$HERE/observe.ts" --stats "$out/stats.jsonl" >/dev/null
  log "$name: end: $(tail -1 "$out/run.log")"
  touch "$out/done"
}

clients() { # name, image
  local name=$1 img=$2 out=$ROOT/$1
  [ -f "$out/done" ] && return 0
  rm -rf "$out"
  reset
  log "$name: start"
  bash "$HERE/clients-fault.sh" "$out" "$img" "$SITE" 60 >"$out.summary" 2>&1
  log "$name: end: $(head -1 "$out.summary")"
  touch "$out/done"
}

conformance t5541-nofault 2.56.0 0
clients clients-239 graft-git239
clients clients-251 graft-git251
# The two client runs again with every watched leader first moved to the faulted site (reset's home.sh, added after
# the first two ran with most leaders already on site 1).
clients clients-239-homed graft-git239
clients clients-251-homed graft-git251
conformance t-251 2.51.0 1
conformance t-256 2.56.0 1
conformance t-239 2.39.5 1
reset
log "all steps done"
touch "$ROOT/all.done"
# An orb service restarts a command that exits.
sleep infinity
