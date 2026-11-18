#!/bin/bash
# push.sh EXPECT NEW
# Headers go to curl from a builtin printf through a pipe, so no token is on a command line `ps` shows.
BED=$(cat ${BED_TOKEN_FILE:-~/.graft-wp6/bed.token}); W=$(sed -n 2p ${BF20_HOME:-~/.graft-bf20}/tokens.txt | cut -d' ' -f1)
E=$1; [ "$E" = null ] && E=null || E="\"$E\""
curl -s -m 60 -X POST "${GRAFT_EDGE:-https://graft-m1-edge.${CF_WORKERS_SUBDOMAIN:?}.workers.dev}/_bed/push?repo=${REPO:-196627}" -H @<(printf 'x-bed-token: %s\nauthorization: token %s\n' "$BED" "$W") -d "{\"ref\":\"refs/heads/bf20/${REF:-probe}\",\"expect\":$E,\"new\":\"$2\"}"
