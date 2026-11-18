#!/bin/bash
# m.sh SITE PATH [JSONBODY]  -> member call through wp7 edge
TOK=$(cat /home/user/workspace/repo/tests/m0/.m0-token)
REQ=$(python3 -c "
import json,sys
b={'chain':'${CH:-repo:196627}','site':int(sys.argv[1]),'path':sys.argv[2],'deadline':${DL:-30000}}
if len(sys.argv)>3 and sys.argv[3]: b['body']=json.loads(sys.argv[3])
print(json.dumps(b))" "$@")
for retry in 1 2 3 4 5 6; do OUT=$(curl -s -m ${TO:-60} -X POST ${GRAFT_TOOLS_EDGE:-https://graft-m1-wp7-edge.${CF_WORKERS_SUBDOMAIN:?}.workers.dev}/member -H @<(printf 'authorization: Bearer %s\n' "$TOK") -d "$REQ")
  [ -n "$OUT" ] && break; sleep 5
done
printf "%s" "$OUT"
