#!/usr/bin/env bash
# Backfills what M3 gives every account to the Milestone 1 seed accounts (tests/m1/integ/seed.ts), which predate it:
# the owner's `Placement` row on `account_shard`, a copy of the install account's record as `after_signup` writes, and
# the user's `Identity` chain's `Created`. Without them the hosted `Audit` consumer cannot list their stored PATs
# (`finish::credential` owes an `AddCredential` on an `Identity` chain with no record) and retries every 30 s.
# Rerunnable: a second run is refused `RecordExists` and `AlreadyCreated`, which count as done.
#
#   tests/m3/hosted/m1-identity.sh
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd)
TOOLS=${GRAFT_TOOLS_EDGE:-https://graft-m1-wp7-edge.${CF_WORKERS_SUBDOMAIN:?}.workers.dev}
EDGE=${GRAFT_EDGE:-https://graft-m1-edge.${CF_WORKERS_SUBDOMAIN:?}.workers.dev}
tools() { curl -s -m 300 -X POST -H @- --data "$2" "$TOOLS$1" <<<"authorization: Bearer $(cat "$here/../../m0/.m0-token")"; }
bed() { curl -s -m 300 -X POST -H @- --data "$1" "$EDGE/_bed/identity" <<<"x-bed-token: $(cat "$HOME/.graft-wp6/bed.token")"; }
rid() { python3 -c 'import os,time; v=(int(time.time()*1000)<<80)|int.from_bytes(os.urandom(10),"big"); A="0123456789ABCDEFGHJKMNPQRSTVWXYZ"; print("".join(A[(v>>(5*i))&31] for i in range(25,-1,-1)))'; }
shard() { python3 -c 'import hashlib,struct,sys; print(int.from_bytes(hashlib.sha256(b"graftyard/names/account/"+struct.pack(">q",int(sys.argv[1]))).digest()[:4],"big")%1024)' "$1"; }
refusal() { jq -r 'if .ok then "ok" else (.refused.err // tostring) end' | cut -d' ' -f1 | cut -d'(' -f1; }
OP='{"account":null,"key":null}'
TRI='{"name":"Tri","sites":[0,1,2],"home":0,"generation":1,"moving_from":null,"at":5}'
while read -r acct login; do
  as=$(shard "$acct")
  tools /create "{\"chain\":\"names:$as\"}" >/dev/null
  p=$(tools /commit "{\"chain\":\"names:$as\",\"cmd\":{\"cred\":$OP,\"op\":{\"InitPlacement\":{\"owner\":$acct,\"record\":$TRI}}},\"rid\":\"$(rid)\"}" | jq -c '.outcome' | refusal)
  c=$(bed "{\"chain\":\"identity:$acct\",\"cmd\":{\"cred\":$OP,\"op\":{\"Create\":{\"login\":\"$login\",\"kind\":\"User\",\"created\":$(date +%s%3N)}}}}" | jq -c '.outcome' | refusal)
  echo "$acct $login: InitPlacement on names:$as $p; Create on identity:$acct $c"
done <<'EOF'
7000001 wp6
7000002 wp6-writer
7000003 wp6-reader
7000004 wp6-revoked
EOF
