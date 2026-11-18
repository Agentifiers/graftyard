#!/usr/bin/env bash
# Before a BM-1 step: every watched group's leader on site <site> (the site the step then fails), so the fault moves
# leaders off it. Each leader elsewhere gets `/transfer {"to": <site>}` (Raft leadership transfer) through the
# M1-WP7 test edge's raw `/member` call, and the script waits up to 60 s for every group to lead there.
#
#   tests/m1/wp8/home.sh <site> [chains, default observe.ts's]
set -u
SITE=${1:?site}
CHAINS=${2:-repo:196608,repo:196610,names:174,names:987,names:938,names:806,authz:7000001,tokens:1563,tokens:1153}
EDGE=https://graft-m1-wp7-edge.${CF_WORKERS_SUBDOMAIN:?}.workers.dev
T=$(cat "${GRAFT_REPO:-$(cd "$(dirname "$0")/../../.." && pwd)}/tests/m0/.m0-token")
# A call with no body ("") is a GET on the member; one with a body a POST.
m() {
  local b=""
  [ -n "${4:-}" ] && b=",\"body\":$4"
  curl -s -m 15 -X POST -H "Authorization: Bearer $T" "$EDGE/member" -d "{\"chain\":\"$1\",\"site\":$2,\"path\":\"$3\"$b}"
}
leader() { # the site whose member says it leads, or none
  for s in 0 1 2; do
    m "$1" "$s" /status | python3 -c '
import json,sys
v=json.load(sys.stdin).get("v")
sys.exit(0 if isinstance(v, dict) and v.get("role") == "leader" else 1)' 2>/dev/null && { echo "$s"; return; }
  done
  echo none
}
end=$(( $(date +%s) + 60 ))
while :; do
  away=0
  for c in ${CHAINS//,/ }; do
    l=$(leader "$c")
    [ "$l" = "$SITE" ] && continue
    away=$((away + 1))
    if [ "$l" = none ]; then
      m "$c" "$SITE" /campaign '{}' >/dev/null
    else
      echo "$(date -u +%T) $c: leader $l, transfer to $SITE: $(m "$c" "$l" /transfer "{\"to\":$SITE}" | head -c 160)"
    fi
  done
  [ "$away" = 0 ] && { echo "$(date -u +%T) every leader on site $SITE"; exit 0; }
  [ "$(date +%s)" -ge "$end" ] && { echo "$(date -u +%T) $away groups still lead off site $SITE"; exit 1; }
  sleep 4
done
