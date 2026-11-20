#!/bin/bash
# B0-35's trace saver on a test-faults build: waits for b035.sh to name its repository, then runs tests/nr3/tsave.mjs
# on that chain every 30 s, with ISOLATES=1 and LEARNERS=1, until 60 s after the row's done line, into
# tests/fanout/raw/tsave-$TAG.jsonl. The done file is this row's own and is removed first: m59's saver, copied from
# m58's, read m58's leftover done file and stopped after one pull.
#   TAG=<tag> bash tests/m4/wp4/b035tsave.sh
set -uo pipefail
cd "$(dirname "$0")/../../.."
source tests/am1/lf13/env-m4a.sh >/dev/null 2>&1
TAG=${TAG:?}
ST=tests/m4/raw/wp4/b035-$TAG.status
DONE=$(mktemp -u "/tmp/b035tsave-$TAG.XXXXXX.done")
until id=$(grep -oE " [0-9]+ start$" "$ST" 2>/dev/null | tail -1 | awk '{print $1}'); [ -n "$id" ]; do sleep 10; done
mkdir -p tests/fanout/raw
( until grep -q " done " "$ST" 2>/dev/null; do sleep 15; done; sleep 60; touch "$DONE" ) &
DONE_FILE=$DONE EVERY=30000 ISOLATES=1 LEARNERS=1 node tests/nr3/tsave.mjs "repo:$id" \
  > "tests/fanout/raw/tsave-$TAG.jsonl" 2> "/tmp/b035tsave-$TAG.err"
rm -f "$DONE"
exec sleep infinity
