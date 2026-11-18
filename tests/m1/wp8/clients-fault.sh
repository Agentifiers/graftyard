#!/usr/bin/env bash
# BM-1, part one under the fault: tests/m1/wp6/clients.sh (SHARED's git-http and git-protocol-v2 oracle rows),
# unchanged, with one site's member class failing from --fault-after seconds into the run to its end, as run.sh does
# for part two. The observer (observe.ts) samples the repository groups' leaders throughout, and the members' commit
# latency is read at the end.
#
#   tests/m1/wp8/clients-fault.sh <out dir> <git image or "host"> [fault site, default 0] [fault after s, default 60]
#
# The tokens are ~/.graft-integ/tokens.txt (line 2 writer, 3 read-only, 4 revoked); the repository is wp6/main.
set -u
OUT=${1:?out dir} IMG=${2:?image or host} SITE=${3:-0} AFTER=${4:-60}
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=${GRAFT_REPO:-$(cd "$HERE/../../.." && pwd)}
HOST=graft-m1-edge.${CF_WORKERS_SUBDOMAIN:?}.workers.dev
tok() { sed -n "${1}p" ~/.graft-integ/tokens.txt | cut -d' ' -f1; }
mkdir -p "$OUT"
node "$HERE/observe.ts" --stats "$OUT/stats-before.jsonl" --reset >/dev/null
node "$HERE/observe.ts" --out "$OUT/observe.jsonl" &
obs=$!
(sleep "$AFTER" && FAULT_STATS="$OUT/stats-at-fault.jsonl" bash "$HERE/fault.sh" on "$SITE" >>"$OUT/fault.log" 2>&1) &
fault=$!
echo "$(date -u +%FT%T.%3NZ) clients start ($IMG, fault on site $SITE after ${AFTER}s)" >>"$OUT/fault.log"
if [ "$IMG" = host ]; then
  bash "$ROOT/tests/m1/wp6/clients.sh" "$HOST" wp6/main "$(tok 2)" "$(tok 3)" "$(tok 4)" >"$OUT/clients.log" 2>&1
else
  docker run --rm --network host -e HTTPS_PROXY -v "$ROOT/tests/m1/wp6:/wp6:ro" "$IMG" \
    bash /wp6/clients.sh "$HOST" wp6/main "$(tok 2)" "$(tok 3)" "$(tok 4)" >"$OUT/clients.log" 2>&1
fi
echo "$(date -u +%FT%T.%3NZ) clients end: exit $?" >>"$OUT/fault.log"
kill "$fault" 2>/dev/null
bash "$HERE/fault.sh" off >>"$OUT/fault.log" 2>&1
sleep 5
kill "$obs" 2>/dev/null
node "$HERE/observe.ts" --stats "$OUT/stats.jsonl" >/dev/null
grep -cE '^PASS' "$OUT/clients.log" | sed 's/^/pass /'
grep -E '^FAIL' "$OUT/clients.log"
