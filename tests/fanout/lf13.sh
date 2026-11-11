#!/bin/bash
# LF-13, both modes, 3 runs each (tests/am1/rest/lf13.mjs), for one orb service: on graft-m2b, or on the install
# LF13_ENV names (tests/am1/lf13/env-m3c.sh for graft-m3c).
cd "$(dirname "$0")/../.."
source ${LF13_ENV:-tests/m3/rows/env.sh}
ST=tests/fanout/raw/lf13${LF13_TAG:-}.status
[ -e tests/fanout/raw/.lf13${LF13_TAG:-}.started ] && exec sleep infinity
touch tests/fanout/raw/.lf13${LF13_TAG:-}.started
id() { awk -v n="$1" '$1==n{print $2}' "$M3_STATE/am1-runs-repos.txt"; }
for mode in ${MODES:-gap install}; do
  r=$([ $mode = gap ] && echo lf13-g || echo lf13-i)
  for n in 1 2 3; do
    echo "$(date -u +%FT%TZ) lf13 $mode $n start" >> $ST
    timeout 900 node tests/am1/rest/lf13.mjs $mode --repo $r --chain "repo:$(id $r)" > tests/am1/raw/runs/flf13-$mode-$n.log 2>&1
    echo "$(date -u +%FT%TZ) lf13 $mode $n exit $?" >> $ST
    sleep 20
  done
done
echo DONE >> $ST
exec sleep infinity
