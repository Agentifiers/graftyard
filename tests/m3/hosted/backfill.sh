#!/usr/bin/env bash
# The `AccountLogin` backfill for the graft-m1 install's accounts seeded before F50 (tests/m1/integ/seed.ts):
# for each account, claims its login if the seed never did (writer and reader got only an `Account` row), then reads
# the login's shard and the account's `account_shard` as `dep` folds and commits what `finish::account_login` plans
# (tools/src/bin/backfill.rs). Rerunnable: a second run plans nothing. Every command goes through the M1-WP7 tools edge as the
# operator, with its bearer token on stdin.
#
#   tests/m3/hosted/backfill.sh <out dir>
set -euo pipefail
out=${1:?out dir}
mkdir -p "$out"
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../.." && pwd)
EDGE=${GRAFT_TOOLS_EDGE:-https://graft-m1-wp7-edge.${CF_WORKERS_SUBDOMAIN:?}.workers.dev}
tools() { curl -s -m 120 -X POST -H @- -d "$2" "$EDGE$1" <<<"authorization: Bearer $(cat "$root/tests/m0/.m0-token")"; }
shard() { node -e '
const {createHash}=require("crypto"); const [k,v]=process.argv.slice(1);
const b=k==="account"?(()=>{const x=Buffer.alloc(8);x.writeBigInt64BE(BigInt(v));return x})():Buffer.from(v.toLowerCase());
console.log(createHash("sha256").update(`graftyard/names/${k}/`).update(b).digest().readUInt32BE(0)%1024)' "$1" "$2"; }
rid() { python3 -c 'import os,time; v=(int(time.time()*1000)<<80)|int.from_bytes(os.urandom(10),"big"); A="0123456789ABCDEFGHJKMNPQRSTVWXYZ"; print("".join(A[(v>>(5*i))&31] for i in range(25,-1,-1)))'; }
plan=$CARGO_TARGET_DIR/debug/backfill
(cd "$here/tools" && cargo build -q --bin backfill)

# account login (tests/m1/integ/seed.ts: owner, writer, reader, revoked).
while read -r acct login; do
  ls=$(shard login "$login"); as=$(shard account "$acct")
  row=$(tools /read "{\"chain\":\"names:$ls\",\"q\":{\"login\":\"$login\"}}")
  if [ "$(jq -c '.value.login' <<<"$row")" = null ]; then
    tools /create "{\"chain\":\"names:$ls\"}" >/dev/null
    claim="{\"cred\":{\"account\":null,\"key\":null},\"op\":{\"ClaimLogin\":{\"login\":\"$login\",\"kind\":\"User\",\"account\":$acct}}}"
    echo "$acct $login: ClaimLogin on names:$ls $(tools /commit "{\"chain\":\"names:$ls\",\"cmd\":$claim,\"rid\":\"$(rid)\"}" | jq -c '.outcome // .')"
  fi
  for pass in 1 2; do
    tools /read "{\"chain\":\"names:$ls\",\"q\":{\"dep\":true}}" | jq -r '.value // ""' > "$out/names-$ls.hex"
    tools /read "{\"chain\":\"names:$as\",\"q\":{\"dep\":true}}" | jq -r '.value // ""' > "$out/names-$as.hex"
    step=$("$plan" "$acct" "$login" "$out/names-$ls.hex" "$out/names-$as.hex")
    echo "$acct $login pass $pass: plan $step"
    [ "$step" = null ] && break
    body=$(jq -c --arg rid "$(rid)" '{chain, cmd, rid: $rid}' <<<"$step")
    echo "$acct $login: commit $(tools /commit "$body" | jq -c '.outcome // .')"
  done
  echo "$acct $login: row $(tools /read "{\"chain\":\"names:$as\",\"q\":{\"accountLogins\":[$acct]}}" | jq -c '.value.accountLogins')"
done <<'EOF'
7000001 wp6
7000002 wp6-writer
7000003 wp6-reader
7000004 wp6-revoked
EOF
