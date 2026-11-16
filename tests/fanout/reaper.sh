#!/bin/bash
# Every 60 s, tears down the graft-m2b shard containers of networks FIRST..LAST (tests/fanout/teardown.py), which the
# site's */15 cron wakes again (lm1-memory, "What wakes idle shards"). One orb service; stop it when done.
#   bash tests/fanout/reaper.sh FIRST LAST [FIRST LAST ...], e.g. 131100 131217   -> tests/fanout/raw/reaper.log
cd "$(dirname "$0")/../.."
source tests/m3/rows/env.sh
while true; do
  out=$(python3 tests/fanout/teardown.py "$@" 2>&1 | tail -1)
  case "$out" in *'"torn_down": 0'*) ;; *) echo "$(date -u +%FT%TZ) $out" >> tests/fanout/raw/reaper.log ;; esac
  sleep 60
done
