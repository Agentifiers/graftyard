#!/bin/bash
# Every 15 s, the voters' status of each row's repository as rows.sh starts it, into tests/fanout/raw/lead-<tag>.log.
#   bash tests/fanout/leadwatch.sh <rows tag>     (one orb service; stop it after the rows)
cd "$(dirname "$0")/../.."
source tests/m3/rows/env.sh
ST=tests/fanout/raw/rows-$1.status
while true; do
  id=$(grep -oE " [0-9]+ start" "$ST" | tail -1 | awk '{print $1}')
  [ -n "$id" ] && timeout 40 node tests/fanout/lead.mjs "repo:$id" >> tests/fanout/raw/lead-$1.log 2>&1
  sleep 15
done
