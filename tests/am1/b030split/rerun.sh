#!/bin/bash
# The B0-30 rerun on graft-m1, as briefed: 30 simulated days of journey 9 at 18x (about 40 h) with pushes on
# (load.mjs's default 1.6 pushes/s and 2.48 GB a simulated day), on a fresh repository seeded with the 4 GB metadata
# tier, no keeper. One step per service; each waits for the one before it.
#   amp orb service start b030-seed    --command 'bash tests/am1/b030split/rerun.sh seed'
#   amp orb service start b030-load    --command 'bash tests/am1/b030split/rerun.sh load'
#   amp orb service start b030-monitor --command 'bash tests/am1/b030split/rerun.sh monitor'
#   amp orb service start b030-cross   --command 'bash tests/am1/b030split/rerun.sh cross'
# STOP_USD (default 175) is the rerun's own spend, DO, Workers and containers, from the start time in
# tests/am1/raw/b030r2.start. To change it mid-run, write the new number to tests/am1/raw/b030-stop-usd.
cd "$(dirname "$0")/../../.."
export BED_TOKEN_FILE=$HOME/.graft-wp6/bed.token
R=tests/am1/raw
NAME=${NAME:-b0-30-r2}
seeded() { [ -s $R/b030-seed.json ] && grep -q "\"repo:$(jq .repo $R/b030-repo.json)\"" $R/b030-seed.json; }
case "$1" in
  seed)
    # After the rerun's site day (tests/am1/b030split/siteday.sh) has landed.
    until grep -q '^DONE' $R/b030split-siteday.log 2>/dev/null; do sleep 30; done
    grep -q '^exit 0' $R/b030split-siteday.log || { echo "site day failed" >> $R/b030r2-seed.log; exec sleep infinity; }
    [ -s $R/b030r2.start ] || date -u +%FT%TZ > $R/b030r2.start
    { grep -q "\"m3org/$NAME\"" $R/b030-repo.json 2>/dev/null || node tests/am1/b030/mkrepo.mjs "$NAME" || exit 1
      seeded || node tests/am1/b030/seed.mjs 4000000000
      echo "exit $?"; date -u +%FT%TZ; echo DONE; } >> $R/b030r2-seed.log 2>&1 ;;
  load)
    until seeded; do sleep 60; done
    node tests/am1/b030/load.mjs >> $R/b030r2-load.log 2>&1 ;;
  monitor)
    until seeded; do sleep 60; done
    STOP_USD=${STOP_USD:-175} B030_SINCE=$(cat $R/b030r2.start) MONITOR_S=900 node tests/am1/b030/monitor.mjs >> $R/b030r2-monitor.log 2>&1 ;;
  cross)
    until seeded; do sleep 60; done
    node tests/am1/b030/cross.mjs >> $R/b030r2-cross.log 2>&1 ;;
esac
exec sleep infinity
