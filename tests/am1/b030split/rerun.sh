#!/bin/bash
# The B0-30 rerun on graft-m1 (third run, b030r3, m3org/b0-30-r3, from 2026-11-16; set B030_RUN=b030r2 and
# NAME=b0-30-r2 for the second run's files), as briefed: 30 simulated days of journey 9 at 18x (about 40 h) with pushes on
# (load.mjs's default 1.6 pushes/s and 2.48 GB a simulated day), on a fresh repository seeded with the 4 GB metadata
# tier, no keeper. One step per service; each waits for the one before it.
#   amp orb service start b030-seed    --command 'bash tests/am1/b030split/rerun.sh seed'
#   amp orb service start b030-load    --command 'bash tests/am1/b030split/rerun.sh load'
#   amp orb service start b030-monitor --command 'bash tests/am1/b030split/rerun.sh monitor'
#   amp orb service start b030-cross   --command 'bash tests/am1/b030split/rerun.sh cross'
#   amp orb service start b030-watch   --command 'bash tests/am1/b030split/rerun.sh watch'
#   amp orb service start b030-drain   --command 'bash tests/am1/b030split/rerun.sh drain'
# The short run (b030r4, operator 2026-11-16, option A; b030-split.md, "Proposal: a shorter B0-30 run from the split
# onward") adds two services and seeds 5.5 GB: B030_RUN=b030r4 NAME=b0-30-r4 SEED_BYTES=5500000000.
#   amp orb service start b030-force   --command 'bash tests/am1/b030split/rerun.sh force'
#   amp orb service start b030-acct    --command 'bash tests/am1/b030split/rerun.sh acct'
# The second short run (b030r5, operator 2026-11-17): B030_RUN=b030r5 NAME=b0-30-r5, and force and acct take
# SEVER_AT=split END_AT=install and PREFIX=graft-m1 RUN_USD=60 from the environment:
#   amp orb service start b030r5-siteday --command 'RUN=b030r5 bash tests/am1/b030split/siteday-r4.sh'
#   amp orb service start b030-force --command 'B030_RUN=b030r5 NAME=b0-30-r5 SEVER_AT=split END_AT=install bash tests/am1/b030split/rerun.sh force'
#   amp orb service start b030-acct  --command 'B030_RUN=b030r5 PREFIX=graft-m1 RUN_USD=60 bash tests/am1/b030split/rerun.sh acct'
# The third short run (b030r6, operator 2026-11-17, from main with G21 and G22): as b030r5, with force's install cap of
# 120 min after the sever lifts (INSTALL_CAP_MIN), and the teardown by generation scan plus purge (retire-run.mjs).
# Generic state files of the run before go to tests/am1/raw/r5-generic/ first.
#   amp orb service start b030r6-siteday --command 'RUN=b030r6 bash tests/am1/b030split/siteday-r4.sh'
#   E='B030_RUN=b030r6 NAME=b0-30-r6'; seed, load, cross, watch as above with $E;
#   amp orb service start b030-force --command "$E SEVER_AT=split END_AT=install INSTALL_CAP_MIN=120 bash tests/am1/b030split/rerun.sh force"
#   amp orb service start b030-acct  --command "$E PREFIX=graft-m1 RUN_USD=60 DAY_USD=170 bash tests/am1/b030split/rerun.sh acct"
#   then node tests/am1/b030split/configs.mjs repo:<id> (and repo-meta:<id>) for the create-time config line.
# STOP_USD (default 190) is the old per-install meter's line, for diagnosis only since 2026-11-16; the stop is acctstop.py's. It was the rerun's own spend, DO, Workers and containers, from the start time in
# tests/am1/raw/$B030_RUN.start. To change it mid-run, write the new number to tests/am1/raw/b030-stop-usd.
cd "$(dirname "$0")/../../.."
export BED_TOKEN_FILE=$HOME/.graft-wp6/bed.token
R=tests/am1/raw
NAME=${NAME:-b0-30-r4}
RUN=${B030_RUN:-b030r4}
SEED_BYTES=${SEED_BYTES:-5500000000}
export B030_RUN=$RUN
# Seeded: the seed of the rerun's own repository is done (b030-repo.json names m3org/$NAME, and b030-seed.json names it).
seeded() { grep -q "\"m3org/$NAME\"" $R/b030-repo.json 2>/dev/null && [ -s $R/b030-seed.json ] \
  && grep -q "\"repo:$(jq .repo $R/b030-repo.json)\"" $R/b030-seed.json; }
case "$1" in
  seed)
    # After the run's site day (tests/am1/b030split/siteday-r4.sh) has landed.
    until grep -q '^DONE' $R/$RUN-siteday.log 2>/dev/null; do sleep 30; done
    grep -q '^exit 0' $R/$RUN-siteday.log || { echo "site day failed" >> $R/$RUN-seed.log; exec sleep infinity; }
    [ -s $R/$RUN.start ] || date -u +%FT%TZ > $R/$RUN.start
    { grep -q "\"m3org/$NAME\"" $R/b030-repo.json 2>/dev/null || node tests/am1/b030/mkrepo.mjs "$NAME" || exit 1
      seeded || node tests/am1/b030/seed.mjs $SEED_BYTES
      echo "exit $?"; date -u +%FT%TZ; echo DONE; } >> $R/$RUN-seed.log 2>&1 ;;
  load)
    until seeded; do sleep 60; done
    # No keepHome: handing leadership back to site 0 was the first run's aid for a promotion that worked only on a warm
    # home shard (b0-30.md, "Blocked on", 7), which hot-shards-2 removed; like the keeper, it would steer leadership.
    B030_HOME=none node tests/am1/b030/load.mjs >> $R/$RUN-load.log 2>&1 ;;
  drain)
    # After load.mjs stops at day 30: every push its schedule still owed, at its concurrency (operator, 2026-11-15).
    node tests/am1/b030/drain.mjs >> $R/$RUN-drain.log 2>&1 ;;
  monitor)
    until seeded; do sleep 60; done
    B030_ALL_CONTAINERS=1 STOP_USD=${STOP_USD:-190} B030_SINCE=$(cat $R/$RUN.start) MONITOR_S=900 node tests/am1/b030/monitor.mjs >> $R/$RUN-monitor.log 2>&1 ;;
  cross)
    until seeded; do sleep 60; done
    node tests/am1/b030/cross.mjs >> $R/$RUN-cross.log 2>&1 ;;
  watch)
    until seeded; do sleep 60; done
    node tests/am1/b030split/watch.mjs >> $R/$RUN-watch.log 2>&1 ;;
  force)
    until seeded; do sleep 60; done
    # b030r5: SEVER_AT=split END_AT=install (operator, 2026-11-17).
    node tests/am1/b030split/force.mjs >> $R/$RUN-force.log 2>&1 ;;
  acct)
    # The stop rule on the account meter: $150 for the run from the site day, or $170 for the account's UTC day.
    until [ -s $R/$RUN.start ]; do sleep 30; done
    # b030r5: PREFIX=graft-m1 RUN_USD=60 (operator, 2026-11-17).
    python3 tests/am1/b030split/acctstop.py >> $R/$RUN-acct.log 2>&1 ;;
esac
exec sleep infinity
