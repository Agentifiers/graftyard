#!/usr/bin/env bash
# BM-1's fault control on the test install: one site's member class fails every call, as Milestone 0's B0-03
# simulated region loss. Sets the plan through the git edge's test-faults route `/_bed/fault`, which writes it to every
# site's `Registry` object `faults`; each member isolate rereads it at most once a second
# (`workers/site/src/faults.rs`, `SiteFault`).
#
#   tests/m1/wp8/fault.sh on <site>     every call to and from <site>'s member class fails, from now until cleared
#   tests/m1/wp8/fault.sh off           clears it
#
# These are run.sh's `--fault-cmd` and `--restore-cmd`, and clients-fault.sh calls them for clients.sh. With
# FAULT_RATE=<r> below 1 (the gray fault), each call fails with chance r instead of every call. With
# FAULT_STATS=<file>, `on` also saves the members' stats so far to <file> and resets them.
set -u
HOST=${EDGE_HOST:-https://graft-m1-edge.<workers-subdomain>.workers.dev}
B=$(cat ~/.graft-wp6/bed.token)
case "${1:?on <site> | off}" in
on) q="site=${2:?site}${FAULT_RATE:+&rate=$FAULT_RATE}" ;;
off) q="off=1" ;;
*) echo "on <site> | off" >&2; exit 2 ;;
esac
echo "$(date -u +%FT%T.%3NZ) fault $*: $(curl -s -X POST -H "x-bed-token: $B" "$HOST/_bed/fault?$q")"
# With FAULT_STATS=<file>, the members' stats up to now are saved there and reset, so the stats read after the run
# are the fault's alone.
if [ -n "${FAULT_STATS:-}" ]; then
  node "$(dirname "$0")/observe.ts" --stats "$FAULT_STATS" --reset >/dev/null 2>&1
  echo "$(date -u +%FT%T.%3NZ) stats saved to $FAULT_STATS and reset"
fi
