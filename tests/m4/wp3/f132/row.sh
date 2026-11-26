#!/bin/bash
# F132 on graft-m4a (`decisions/OPEN-QUESTIONS.md`, F132; `decisions/DECISIONS.md`, "`reusable`"): a `push`
# run whose workflow calls another repository's reusable workflow, read as the edge's dispatch reads it
# (`reusable::called_file`). The push and checks are `push.mjs`'s; the plan and verdict are
# `tests/m4/results/m4-wp3-f132.md`.
#
#   bash tests/m4/wp3/f132/row.sh <tag> <step>
#
# Steps, in order (one per call, each logged to tests/m4/raw/wp3/f132/<tag>/<step>.log; the stop is $3, read by
# `state` before each step):
#   state      BF-74's (`tests/m4/wp3/bf74/row.sh state`) with ROW=f132
#   flag       BF-78's site day (`tests/m4/wp3/bf78/row.sh`, `flag`) from TREE, a worktree at a main with push-calls:
#              every live flag kept (GRAFTYARD_REUSABLE included) and GRAFTYARD_PUSH_TRIGGER=1 added
#   push       `push.mjs`: both repositories, one push, its called job, and the checks
#   off        the same site day without GRAFTYARD_PUSH_TRIGGER, so no other row's push starts runs
#   teardown   both repositories deleted
#
# Needs what BF-78's row needs: RUNNER (an unpacked actions/runner 2.338.2), target/release/examples/jitconfig, and
# TREE (with `worker-build --release --features test-faults` done in workers/site).
set -uo pipefail
unset HTTP_PROXY HTTPS_PROXY http_proxy https_proxy
TAG=${1:?tag}; STEP=${2:?step}
REPO=$(cd "$(dirname "$0")/../../../.." && pwd)
O=$REPO/tests/m4/raw/wp3/f132/$TAG; mkdir -p "$O"
export F132_REPO=${F132_REPO:-f132-$TAG}
scrub() { sed -e "s/$CLOUDFLARE_ACCOUNT_ID/<install-account>/g" -e "s/$CF_WORKERS_SUBDOMAIN/<sub>/g" -e "s#$HOME/#~/#g" -e "s#$REPO/#<checkout>/#g"; }
log() { { date -u +%FT%TZ; git -C "$REPO" rev-parse --short HEAD; cat; } | scrub >"$O/$STEP.log"; cat "$O/$STEP.log"; }
# BF-78's days write under tests/m4/raw/wp3/bf78/f132-<tag>/; their logs are moved here.
day() {
  local b=$REPO/tests/m4/raw/wp3/bf78/f132-$TAG
  bash "$REPO/tests/m4/wp3/bf78/row.sh" "f132-$TAG" "$1"; local rc=$?
  mkdir -p "$O/$1"; mv "$b/$1"/* "$b/$1.log" "$O/$1/" 2>/dev/null; rmdir "$b/$1" "$b" 2>/dev/null
  for f in $(grep -rlE "$CF_WORKERS_SUBDOMAIN|$CLOUDFLARE_ACCOUNT_ID" "$O/$1"); do scrub <"$f" >"$f.s" && mv "$f.s" "$f"; done
  echo "$1 day exit $rc"
}

case $STEP in
state) ROW=f132 bash "$REPO/tests/m4/wp3/bf74/row.sh" "$TAG" state ;;
flag) { [ -s "$O/cost.from" ] || date -u +%s >"$O/cost.from"; day flag; } 2>&1 | log ;;
off) { day off; } 2>&1 | log ;;
# The events name the edge's workers.dev URL, so the output is scrubbed in place and the runners' keys removed.
push) { node "$REPO/tests/m4/wp3/f132/push.mjs" "$O/push"; echo "push exit $?"
    rm -rf "$O"/push/runner-*/; for f in $(grep -rlE "$CF_WORKERS_SUBDOMAIN|$CLOUDFLARE_ACCOUNT_ID" "$O"); do scrub <"$f" >"$f.s" && mv "$f.s" "$f"; done
  } 2>&1 | log ;;
teardown) { node "$REPO/tests/m4/wp3/f132/push.mjs" delete "$O"; echo "delete exit $?"; } 2>&1 | log ;;
*) echo "step: state flag push off teardown"; exit 2 ;;
esac
