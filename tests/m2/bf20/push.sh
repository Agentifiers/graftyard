#!/bin/bash
# push.sh EXPECT NEW
BED=$(cat ~/.graft-wp6/bed.token); W=$(sed -n 2p ~/.graft-bf20/tokens.txt | cut -d' ' -f1)
E=$1; [ "$E" = null ] && E=null || E="\"$E\""
curl -s -m 60 -X POST "https://graft-m1-edge.<workers-subdomain>.workers.dev/_bed/push?repo=${REPO:-196627}" -H "x-bed-token: $BED" -H "authorization: token $W" -d "{\"ref\":\"refs/heads/bf20/${REF:-probe}\",\"expect\":$E,\"new\":\"$2\"}"
