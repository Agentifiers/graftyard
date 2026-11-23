#!/bin/bash
# B0-32's gate row on graft-m3f under the host guard (`tests/fanout/hostguard.py`; `tests/am1/results/b033-pga.md`,
# "The host guard"): guardrow.sh's hold and re-place around rows-m3f.sh's single B0-32 row. While a1 sets up, the
# leader object's CPU per setup commit is read from Workers Logs, and a1 holds before load (`--hold-file`). A leader
# over the line, or one the guard could not judge, is re-placed on a fresh repository: at most two re-places. If the
# third placement is re-placed too, the set stops without loading; that is not a fail (orchestrator, after m3fsf2).
# Each placement's repository is retired and purged after its row, loaded or not. A pass on this row reads kept hosts
# only (`decisions/OPEN-QUESTIONS.md`, A44).
#   TAG=<prefix> RUN_FROM=<site day start, ISO> [RUN_STOP_USD=3] [STOP_USD=180] bash tests/fanout/guardrow-m3f.sh
#   -> tests/fanout/raw/guardrow-<prefix>.log; placements are <prefix>A, <prefix>B, <prefix>C, each with
#      rows-m3f.sh's tests/fanout/raw/rows-<prefix><R>.status.
# Run it with the install's env loaded (the account variables and TOOLS_TOKEN_FILE), as rows-m3f.sh needs, from a
# clean worktree.
set -uo pipefail
cd "$(dirname "$0")/../.."
source tests/am1/lf13/env-m3f.sh
P=${TAG:?} L=tests/fanout/raw/guardrow-$P.log
export RUN_FROM=${RUN_FROM:?the site day start, ISO} RUN_STOP_USD=${RUN_STOP_USD:-3} RUN_PREFIX=graft-m3f
export STOP_USD=${STOP_USD:-180} ACCT_FROM=${ACCT_FROM:-$(date -u +%F)T00:00:00Z}
log() { echo "$(date -u +%FT%TZ) $*" >> "$L"; }
log "start $(git rev-parse --short HEAD), run stop \$$RUN_STOP_USD on graft-m3f from $RUN_FROM"
# A site day resets every member, so the install's finished networks are retired again first, as before every m3f
# row. Its log goes to an untracked file: rows.sh refuses a tree with a changed tracked file.
RETIRE_LOG=tests/fanout/raw/reretire-graft-m3f-$P.log bash tests/fanout/reretire.sh tests/fanout/reretire/graft-m3f.txt \
  > tests/fanout/raw/reretire-$P.out 2>&1
grep -q "RERETIRE OK" tests/fanout/raw/reretire-$P.out || { log "RERETIRE FAILED, set stopped"; echo "GUARD${P}DONE" >> "$L"; exit 1; }
for R in A B C; do
  T=$P$R ST=tests/fanout/raw/rows-$P$R.status HOLD=tests/fanout/raw/.guard-$P$R.hold RF=$(date -u +%FT%TZ)
  rm -f "$HOLD"
  log "placement $R ($T) starts"
  TAG=$T ROWS=b0-32 A1_EXTRA="--hold-file $HOLD" bash tests/fanout/rows-m3f.sh &
  row=$!
  # The guard: ask hostguard.py every 20 s, once the leader's site is known (k.mjs's "leader S: k" line), until it
  # answers keep (exit 0) or re-place (exit 3). Unknown (exit 2) keeps asking while a1 holds. Fails closed: a1's own
  # HOLD_WAIT_MS (300 s) ends it before load with exit 4 when no verdict came.
  rid="" verdict="" replaced=0
  while [ -z "$verdict" ] && ! grep -q "^ALLDONE" "$ST" 2>/dev/null; do
    sleep 20
    [ -z "$rid" ] && rid=$(awk -v n="f$T-b032" '$1==n{print $2}' "$M3_STATE/am1-runs-repos.txt" 2>/dev/null)
    [ -z "$rid" ] && continue
    site=$(grep -oE "repo:$rid leader [0-9]+: k " "$ST" 2>/dev/null | tail -1 | awk '{print $3}' | tr -d :)
    [ -z "$site" ] && continue
    # The leader's object by name from the row's voter trace (vmem.mjs): after a re-placement a voter is `#2#f1`, not
    # `#1#f1` (m3fsf2).
    name=$(jq -rR --argjson s "$site" 'fromjson? | select(.site == $s and .name != null) | .name' \
      tests/fanout/raw/vmem-$T-b0-32.jsonl 2>/dev/null | tail -1)
    [ -z "$name" ] && continue
    svc=graft-m3f-site-$(echo abc | cut -c$((site + 1)))
    g=$(python3 -W ignore tests/fanout/hostguard.py "$svc" "$name" "$RF" 2>/dev/null); rc=$?
    case $rc in
      0) verdict=$g; echo go > "$HOLD"; log "placement $R repo $rid $g; load released" ;;
      3) verdict=$g; echo stop > "$HOLD"; replaced=1; log "placement $R repo $rid $g; GUARD: a1 ends before load" ;;
      2) ;;
      *) log "placement $R guard error (exit $rc), asking again" ;;
    esac
  done
  [ -z "$verdict" ] && { replaced=1; log "placement $R GUARD: no verdict, a1 ended before load (fails closed)"; }
  # rows-m3f.sh and the rows.sh it starts each end with `exec sleep infinity`; stop both once FINISHED is written.
  until grep -q "^FINISHED" "$ST" 2>/dev/null; do sleep 15; done
  pkill -P "$row" 2>/dev/null; kill "$row" 2>/dev/null
  log "placement $R done: $(grep -E ' b0-32 .* done ' "$ST" | tail -1)"
  # Retire and purge this placement's repository, loaded or not.
  [ -n "$rid" ] && { RETIRE_LOG=tests/fanout/raw/retirenets-$T.log node tests/fanout/retirenets.mjs "$rid" > /dev/null 2>&1
    log "placement $R retire and purge of $rid exit $?: $(tail -1 tests/fanout/raw/retirenets-$T.log 2>/dev/null)"; }
  grep -q "STOP before\|MEMSTOP" "$ST" && { log "placement $R hit a stop, set stopped"; break; }
  [ $replaced = 0 ] && break
  [ $R = C ] && log "third placement re-placed too: set stopped without load, not a fail"
done
echo "GUARD${P}DONE" >> "$L"
