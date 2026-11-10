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
source tests/m3/rows/env.sh
TAG=$1
ST=tests/fanout/raw/rows-$TAG.status
[ -e tests/fanout/raw/.rows-$TAG.started ] && exec sleep infinity
touch tests/fanout/raw/.rows-$TAG.started
K=${K:-4}
ROWS=${ROWS:-"b0-29 b0-32 b0-34 b0-31 b0-33 l0-12 b0-35"}
id() { awk -v n="$1" '$1==n{print $2}' "$M3_STATE/am1-runs-repos.txt"; }
for row in $ROWS; do
  # The $15 stop: no new row once the window has cost $12. With T0_FILE the window starts there and counts the
  # m2b shard containers too (tests/fanout/cost.sh).
  c=$(bash tests/fanout/cost.sh 2>/dev/null); echo "$(date -u +%FT%TZ) cost $c" >> "$ST"
  usd=$(echo "$c" | python3 -c 'import json,sys; print(json.loads(sys.stdin.read()).get("usd", 99))' 2>/dev/null || echo 99)
  if python3 -c "import sys; sys.exit(0 if float('$usd') >= 12 else 1)"; then echo "STOP at \$$usd" >> "$ST"; break; fi
  repo="f$TAG-${row//-/}"
  node tests/am1/runs/mkrepos.mjs "$repo" >> "$ST" 2>&1
  rid=$(id "$repo")
  [ -n "$rid" ] || { echo "$row: no repo" >> "$ST"; continue; }
  [ "$row" = b0-31 ] && node tests/am1/runs/transfer.mjs "repo:$rid" 2 "$repo" >> "$ST" 2>&1
  # B0-34 leaves k to `fanout` itself (in band, from NetworkLoad); the others start at K.
  if [ "$row" = b0-34 ]; then node tests/fanout/k.mjs "repo:$rid" >> "$ST" 2>&1; else node tests/fanout/k.mjs "repo:$rid" "$K" >> "$ST" 2>&1; fi
  case $row in
    b0-29) flags="--seconds 600" ;;
    b0-31) flags="--seconds 600" ;;
    b0-32) flags="--seconds 180" ;;
    b0-33) flags="--seconds 300" ;;
    b0-34) flags="--seconds 600 --kill-shard-at 300" ;;
    b0-35) flags="--seconds 600" ;;
    l0-12) flags="--seconds 600 --kill-every 60" ;;
  esac
  echo "$(date -u +%FT%TZ) $row $repo $rid start" >> "$ST"
  bash tests/am1/runs/a1.sh "$row" "$repo" "$rid" $flags
  node tests/fanout/k.mjs "repo:$rid" >> "$ST" 2>&1
  echo "$(date -u +%FT%TZ) $row $repo $rid done $(grep -E '^exit' tests/am1/raw/runs/a1-$row-$repo.log)" >> "$ST"
  # A durable_object shard container never sleeps on its own: the row's own shards are torn down after it.
  [ -n "${TEARDOWN:-}" ] && python3 tests/fanout/teardown.py "$rid" "$rid" > /dev/null 2>&1 && echo "$(date -u +%FT%TZ) torn down $rid" >> "$ST"
done
# LF-13, both modes, 3 runs each (tests/am1/rest/lf13.mjs): gap on lf13-g, install on lf13-i, which has both tiers'
# snapshots for the archive alarm. Each run's last row goes to tests/am1/raw/lf13-<mode>.jsonl.
if [ -z "${NO_LF13:-}" ]; then
  for mode in gap install; do
    r=$([ $mode = gap ] && echo lf13-g || echo lf13-i)
    for n in 1 2 3; do
      echo "$(date -u +%FT%TZ) lf13 $mode $n start" >> "$ST"
      timeout 900 node tests/am1/rest/lf13.mjs $mode --repo $r --chain "repo:$(id $r)" > tests/am1/raw/runs/flf13-$mode-$n.log 2>&1
      echo "$(date -u +%FT%TZ) lf13 $mode $n exit $?" >> "$ST"
    done
  done
fi
echo ALLDONE >> "$ST"
exec sleep infinity
