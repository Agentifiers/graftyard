#!/bin/bash
# The fanout reruns on graft-m2b, one orb service, each row on a fresh repository (tests/am1/runs/mkrepos.mjs):
# B0-29, B0-32, B0-34 (home shard killed at 300 s), B0-31 (1 MiB pushes), B0-33 (5,000 agents), L0-12 (leader kill
# every 60 s), each at journey 9's 500 agents unless noted, then B0-35 (500 open PRs). Before each load row the
# network's k is set to K (default 4) through the leader's test-faults `/bench_seed {"fanout"}`, as a step of `fanout`
# would commit it, so the pool shards are named before load starts; FANOUT_K=1 runs a row on one shard.
#   [ROWS="b0-29 ..."] [K=4] bash tests/fanout/rows.sh <tag>
# Output: tests/am1/raw/runs/a1-<row>-<repo>.log, tests/fanout/raw/rows-<tag>.status.
set -uo pipefail
cd "$(dirname "$0")/../.."
source ${ROWS_ENV:-tests/m3/rows/env.sh}
source tests/fanout/srvclock.sh
TAG=$1
ST=tests/fanout/raw/rows-$TAG.status
[ -e tests/fanout/raw/.rows-$TAG.started ] && exec sleep infinity
# A run's state lives in its worktree: m3fr7 ran from /tmp, and an orb restart cleared it mid-row with the raw traces and
# uncommitted driver changes (`tests/am1/results/lm1-rows-m3f.md`, Rows m3fr7). So: no /tmp, and no uncommitted change
# to a tracked file, so the run's drivers are a commit.
case "$PWD" in /tmp/*) echo "rows.sh: run from a worktree under /home/user/workspace, not $PWD" >&2; exit 1 ;; esac
[ -z "$(git status --porcelain --untracked-files=no)" ] || { echo "rows.sh: commit the driver changes first" >&2; exit 1; }
touch tests/fanout/raw/.rows-$TAG.started
echo "$(stamp) start $(git rev-parse --short HEAD) $PWD" >> "$ST"
# A restart's leftovers: an earlier run cut off before its teardown leaves shard containers billing (m3fr7: 7 found at
# 04:26Z). The sweep tears down every running shard and is a no-op on an install with none.
if python3 tests/fanout/teardown.py --sweep >> "$ST" 2>&1; then echo "$(stamp) start sweep ok" >> "$ST"
else echo "$(stamp) START SWEEP FAILED: shard containers still running, see above" >> "$ST"; exit 1; fi
# The cost stop between phases, within a row as well as before it (tests/fanout/costok.sh). ROW_USD="row:usd ..." is
# each row's expected cost: a row starts only if the spend plus it stays under STOP_USD, so no row is started that the
# stop would cut (m3fr6: L0-12 with its stall and refusals cost $12.19, B0-31 $2.24).
export ST STOP_USD=${STOP_USD:-12}
row_usd() { for p in ${ROW_USD:-}; do [ "${p%%:*}" = "$1" ] && { echo "${p#*:}"; return; }; done; echo 0; }
costok() { NEXT_USD=${2:-0} bash tests/fanout/costok.sh "$1"; }
K=${K:-4}
ROWS=${ROWS:-"b0-29 b0-32 b0-34 b0-31 b0-33 l0-12 b0-35"}
id() { awk -v n="$1" '$1==n{print $2}' "$M3_STATE/am1-runs-repos.txt"; }
tracers=()
# TRACE=1: vmem.mjs writes MEMSTOP once a voter's counted high-water passes MEM_STOP_MIB (120 MiB); the row in
# progress is stopped and no later row runs.
MEMSTOP=tests/fanout/raw/.memstop-$TAG
for row in $ROWS; do
  [ -e "$MEMSTOP" ] && { echo "$(stamp) MEMSTOP: $(cat $MEMSTOP)" >> "$ST"; break; }
  # No new row once the window (T0_FILE, tests/fanout/cost.sh) plus the row's ROW_USD reaches STOP_USD.
  costok "$row" "$(row_usd "$row")" || break
  repo="f$TAG-${row//-/}"
  # TRACE=1: the group's per-second leader trace (terms.mjs) and the leader's admission (admit.mjs) beside the row,
  # each waiting for the repository's id; stopped when the row ends.
  [ ${#tracers[@]} -gt 0 ] && kill "${tracers[@]}" 2>/dev/null; tracers=()
  if [ -n "${TRACE:-}" ]; then
    node tests/fanout/terms.mjs "$repo" 10800 > tests/fanout/raw/terms-$TAG-$row.jsonl 2>&1 & tracers+=($!)
    node tests/fanout/admit.mjs "$repo" 10800 > tests/fanout/raw/admit-$TAG-$row.jsonl 2>&1 & tracers+=($!)
    # Every voter's memory split every 2 s (vmem.mjs), for L0-12's memory resets.
    MEM_STOP_FILE=$MEMSTOP node tests/fanout/vmem.mjs "$repo" 10800 > tests/fanout/raw/vmem-$TAG-$row.jsonl 2>&1 & tracers+=($!)
  fi
  node tests/am1/runs/mkrepos.mjs "$repo" >> "$ST" 2>&1
  rid=$(id "$repo")
  [ -n "$rid" ] || { echo "$row: no repo" >> "$ST"; continue; }
  # B0-31: the group homed on apac (its config's `home`), so the hand-off back to `home` keeps the leader there.
  if [ "$row" = b0-31 ]; then
    node tests/am1/runs/transfer.mjs "repo:$rid" 2 "$repo" >> "$ST" 2>&1
    tail -1 "$ST" | grep -q "^rehomed on 2" || { echo "$(stamp) b0-31: not rehomed on apac, row skipped" >> "$ST"; continue; }
    # The new home's shard catches up before the merges start: on m3fr7 it was cold, its rehydrate ran 69 s under the
    # row's load, and all 5 MergeQueue merges in that window answered `warming` (`tests/fanout/shardwarm.mjs`).
    node tests/fanout/shardwarm.mjs "$rid" 2 "${WARM_MIN:-15}" > tests/fanout/raw/shardwarm-$TAG-$row.jsonl 2>&1
    w=$?; echo "$(stamp) b0-31 warm exit $w: $(tail -1 tests/fanout/raw/shardwarm-$TAG-$row.jsonl)" >> "$ST"
    [ $w -eq 0 ] || { echo "$(stamp) b0-31: apac shard not warm, row skipped" >> "$ST"; continue; }
  fi
  # Learners for reads (design/DESIGN.md, "Follower read"), before k.mjs, whose commit tells them the leader.
  # A row whose learners did not place is not the row as run before (m3fr12's B0-32 ran on its two followers alone).
  if [ -n "${LEARNERS:-}" ] && ! node tests/fanout/learners.mjs "repo:$rid" "$LEARNERS" >> "$ST" 2>&1; then
    echo "$(stamp) $row: LEARNERS NOT PLACED, row skipped" >> "$ST"; continue
  fi
  # B0-34 leaves k to `fanout` itself (in band, from NetworkLoad); the others start at K.
  if [ "$row" = b0-34 ]; then node tests/fanout/k.mjs "repo:$rid" >> "$ST" 2>&1; else node tests/fanout/k.mjs "repo:$rid" "$K" >> "$ST" 2>&1
    # The ledgers cap a forced k: a row that did not get K is not the row as written, so it is skipped.
    tail -1 "$ST" | grep -q ": k $K at " || { echo "$(stamp) $row: k $K not granted, row skipped" >> "$ST"; continue; }
  fi
  case $row in
    b0-29) flags="--seconds 600 --open-prs ${B029_PRS:-500}" ;;
    b0-31) flags="--seconds 600" ;;
    b0-32) flags="--seconds 180" ;;
    b0-33) flags="--seconds 300" ;;
    b0-34) flags="--seconds 600 --kill-shard-at 300" ;;
    b0-35) flags="--seconds 600" ;;
    l0-12) flags="--seconds 600 --kill-every 60" ;;
  esac
  echo "$(stamp) $row $repo $rid start" >> "$ST"
  # TRACE=1: every voter's trace rows each minute (tests/nr3/tsave.mjs), kept across its isolate's resets.
  if [ -n "${TRACE:-}" ]; then
    rm -f tests/fanout/raw/.tsave-$TAG-$row.done
    DONE_FILE=tests/fanout/raw/.tsave-$TAG-$row.done node tests/nr3/tsave.mjs "repo:$rid" \
      > tests/fanout/raw/tsave-$TAG-$row.jsonl 2>&1 & tracers+=($!)
  fi
  if [ -n "${TRACE:-}" ]; then
    ( while [ ! -e "$MEMSTOP" ]; do sleep 5; done; pkill -f "a1.mjs $row --repo $repo " ) & tracers+=($!)
  fi
  bash tests/am1/runs/a1.sh "$row" "$repo" "$rid" $flags
  [ -e "$MEMSTOP" ] && echo "$(stamp) $row stopped: MEMSTOP $(cat $MEMSTOP)" >> "$ST"
  [ -n "${TRACE:-}" ] && touch tests/fanout/raw/.tsave-$TAG-$row.done
  # Back to k = 1, which releases the network's pool leases: a lease is only renewed or released by its own network,
  # and the ledger shifts every lease past a call gap of LEDGER_GAP, so a network torn down at k > 1 kept its owner's
  # pool share and every later row of the same owner was granted no slot (rows m21: k 1 on every row).
  node tests/fanout/k.mjs "repo:$rid" 1 >> "$ST" 2>&1
  echo "$(stamp) $row $repo $rid done $(grep -E '^exit' tests/am1/raw/runs/a1-$row-$repo.log)" >> "$ST"
  # L0-12's other parts on the same repository: the 30-minute stall with every member call blackholed, then the idle
  # 60 s lease and the overlapping Exclusive acquire 10 minutes later (stall.mjs), then 100 conflicting pushes and 100
  # REST ref writes per actor kind under AgentsExclusive, the install's two Apps included (rows.mjs l012). Six kinds
  # take about 21 minutes each, so 7,200 s cut m3fr1's last kind.
  # L012_REPLAY_ONLY=1: the replay alone (the storm reproducer); the stall and refusals are left out.
  if [ "$row" = l0-12 ] && [ -z "${L012_REPLAY_ONLY:-}" ] && costok "l0-12 stall"; then
    # The replay's own requests drain first, at most 120 s (held feed polls, member reads, admission queue); on m3fr9
    # the stall's first task got 503 4 s after the replay. The stall runs either way and counts every 503 it sees.
    node tests/fanout/drain.mjs "$rid" 120 > tests/fanout/raw/drain-$TAG-$row.jsonl 2>&1
    echo "$(stamp) l0-12 drain exit $?: $(tail -1 tests/fanout/raw/drain-$TAG-$row.jsonl)" >> "$ST"
    STALL_REPO=$repo STALL_REPO_ID=$rid timeout 3600 node tests/am1/hosted/stall.mjs "${STALL_MIN:-30}" 20 \
      > tests/am1/raw/runs/l012-stall-$repo.log 2>&1
    echo "$(stamp) l0-12 stall exit $?" >> "$ST"
    # rows.mjs runs costok.sh before each refusal kind and stops before one past the stop, naming the kinds not run.
    ROWS_COST_CMD="bash tests/fanout/costok.sh" ROWS_REPO=$repo ROWS_INSTALL_APPS=1 timeout 10800 node tests/am1/hosted/rows.mjs l012 100 \
      > tests/am1/raw/runs/l012-refusals-$repo.log 2>&1
    echo "$(stamp) l0-12 refusals exit $?" >> "$ST"
  fi
  # A durable_object shard container never sleeps on its own: the row's own shards are torn down after it.
  if [ -n "${TEARDOWN:-}" ]; then
    if python3 tests/fanout/teardown.py "$rid" "$rid" > /dev/null 2>&1; then echo "$(stamp) torn down $rid" >> "$ST"
    else echo "$(stamp) TEARDOWN FAILED for $rid; the end sweep retries" >> "$ST"; fi
  fi
  # tsave's last save runs after the done file; give it its minute before the tracers stop.
  [ -n "${TRACE:-}" ] && sleep 70
  [ ${#tracers[@]} -gt 0 ] && kill "${tracers[@]}" 2>/dev/null
done
[ ${#tracers[@]} -gt 0 ] && kill "${tracers[@]}" 2>/dev/null
# LF-13, both modes, 3 runs each (tests/am1/rest/lf13.mjs): gap on lf13-g, install on lf13-i, which has both tiers'
# snapshots for the archive alarm. Each run's last row goes to tests/am1/raw/lf13-<mode>.jsonl.
if [ -z "${NO_LF13:-}" ]; then
  for mode in gap install; do
    r=$([ $mode = gap ] && echo lf13-g || echo lf13-i)
    for n in 1 2 3; do
      echo "$(stamp) lf13 $mode $n start" >> "$ST"
      timeout 900 node tests/am1/rest/lf13.mjs $mode --repo $r --chain "repo:$(id $r)" > tests/am1/raw/runs/flf13-$mode-$n.log 2>&1
      echo "$(stamp) lf13 $mode $n exit $?" >> "$ST"
    done
  done
fi
# Every running shard container on the install, not only this run's rows (teardown.py --sweep): exit 2 names any left.
if python3 tests/fanout/teardown.py --sweep >> "$ST" 2>&1; then echo "$(stamp) sweep ok" >> "$ST"
else echo "$(stamp) SWEEP FAILED: shard containers still running, see above" >> "$ST"; fi
echo ALLDONE >> "$ST"
exec sleep infinity
