#!/bin/bash
# merge-p99 smokes on graft-m3f, for one orb service: each "<name>:<agents>" in SMOKES gets a fresh repository
# p<name>, 8 learners (enam 4, weur 2, apac 2), k = 4, and one B0-29-shaped a1.mjs run of SECONDS_ (default 300), as
# LM-1's round 8e smokes (tests/fanout/smoke.sh) ran them on graft-m2b. Its shard containers are torn down after.
#   SMOKES="s500:500" TAG=base bash tests/am1/mp99/smoke.sh   -> tests/am1/raw/mp99-$TAG.status
cd "$(dirname "$0")/../../.."
source tests/am1/lf13/env-m3f.sh
ST=tests/am1/raw/mp99-$TAG.status
mkdir -p tests/am1/raw/runs
[ -e tests/am1/raw/.mp99-$TAG.started ] && exec sleep infinity
touch tests/am1/raw/.mp99-$TAG.started
id() { awk -v n="$1" '$1==n{print $2}' "$M3_STATE/am1-runs-repos.txt"; }
for s in $SMOKES; do
  IFS=: read -r name agents <<< "$s"
  repo="p$name"
  node tests/am1/runs/mkrepos.mjs "$repo" >> "$ST" 2>&1
  rid=$(id "$repo")
  [ -n "$rid" ] || { echo "$name: no repo" >> "$ST"; continue; }
  node tests/fanout/learners.mjs "repo:$rid" "${LEARNERS:-enam:4000,weur:2000,apac:2000}" >> "$ST" 2>&1
  node tests/fanout/k.mjs "repo:$rid" 4 >> "$ST" 2>&1
  echo "$(date -u +%FT%TZ) $name $repo $rid start" >> "$ST"
  { date -u +%FT%TZ; git rev-parse --short HEAD
    node tests/am1/rest/a1.mjs b0-29 --repo "$repo" --chain "repo:$rid" --agents "$agents" --seconds "${SECONDS_:-300}"
    echo "exit $?"; date -u +%FT%TZ; } > "tests/am1/raw/runs/mp99-$TAG-$name.log" 2>&1
  echo "$(date -u +%FT%TZ) $name $repo $rid done $(grep -E '^exit' "tests/am1/raw/runs/mp99-$TAG-$name.log")" >> "$ST"
  python3 tests/fanout/teardown.py "$rid" "$rid" > /dev/null 2>&1 && echo "$(date -u +%FT%TZ) torn down $rid" >> "$ST"
done
echo "$(date -u +%FT%TZ) cost $(python3 tests/fanout/m2bcost.py "$(date -u -d @"$(cat ~/.graft-m3f/t0)" +%FT%TZ 2>/dev/null || cat ~/.graft-m3f/t0)" "$(date -u +%FT%TZ)" 2>/dev/null)" >> "$ST"
echo DONE >> "$ST"
exec sleep infinity
