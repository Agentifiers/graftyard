#!/bin/bash
# G11's hosted rehome on graft-m3f, for an orb service, through the egress proxy (README, "Harness outages on gate
# rows"): a fresh repository seeded from a bare clone (b034seed.mjs, plan from b034plan.py), its leader's terms traced
# (terms.mjs), then g11.mjs (cold target, planned rehome to site 2, merges around the hand-off, R29's idle check), the
# edge and sites' arrivals per second over the row (arrivals.py), and the end sweep.
#   bash tests/fanout/g11.sh <tag> <bare dir> <plan file>   -> tests/fanout/raw/g11-<tag>.status, .jsonl
set -uo pipefail
cd "$(dirname "$0")/../.."
source tests/am1/lf13/env-m3f.sh
source tests/fanout/srvclock.sh
export NODE_USE_ENV_PROXY=1
TAG=$1 BARE=$2 PLAN=$3 R=tests/fanout/raw
ST=$R/g11-$TAG.status
[ -e $R/.g11-$TAG.started ] && exec sleep infinity
[ -z "$(git status --porcelain --untracked-files=no)" ] || { echo "g11.sh: commit the driver changes first" >&2; exit 1; }
touch $R/.g11-$TAG.started
s() { echo "$(stamp) $*" >> "$ST"; }
s "start $(git rev-parse --short HEAD)"
s "client colo: $(curl -s -m 20 "$GRAFT_EDGE/cdn-cgi/trace" | grep -E '^colo=')"
python3 tests/fanout/teardown.py --sweep >> "$ST" 2>&1 && s "start sweep ok" || s "START SWEEP FAILED"
repo=f$TAG-g11
node tests/am1/runs/mkrepos.mjs "$repo" >> "$ST" 2>&1
rid=$(awk -v n="$repo" '$1==n{print $2}' "$M3_STATE/am1-runs-repos.txt")
[ -n "$rid" ] || { s "no repo"; echo DONE >> "$ST"; exec sleep infinity; }
s "repo $repo = $rid"
node tests/fanout/terms.mjs "repo:$rid" 3600 > $R/terms-$TAG-g11.jsonl 2>&1 & TERMS=$!
node tests/fanout/b034seed.mjs "$BARE" "$PLAN" "m3org/$repo" admin > $R/g11-seed-$TAG.log 2>&1
s "seed exit $?: $(tail -1 $R/g11-seed-$TAG.log)"
from=$(stamp)
node tests/fanout/g11.mjs "$repo" "$PLAN" "$BARE" 2 24 > $R/g11-$TAG.jsonl 2>&1
s "g11 exit $?: $(tail -1 $R/g11-$TAG.jsonl)"
to=$(stamp)
s "client colo: $(curl -s -m 20 "$GRAFT_EDGE/cdn-cgi/trace" | grep -E '^colo=')"
kill $TERMS 2>/dev/null
s "arrivals $from to $to: $(python3 tests/fanout/arrivals.py graft-m3f "$from" "$to" $R/arrivals-$TAG.jsonl 2>&1 | tail -1)"
if python3 tests/fanout/teardown.py --sweep >> "$ST" 2>&1; then s "sweep ok"; else s "SWEEP FAILED: shard containers still running"; fi
echo DONE >> "$ST"
exec sleep infinity
