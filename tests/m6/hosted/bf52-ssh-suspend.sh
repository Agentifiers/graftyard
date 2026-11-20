#!/usr/bin/env bash
# BF-52's suspension half hosted over SSH (build/BUILD-PLAN.md, M6-WP2): a new user (seeded through the M1-WP7 tools
# edge as the operator, as tests/m3/hosted/bf3-suspend.sh seeds one) with a PAT and an SSH key added by `POST
# /user/keys`; a repository it owns; a push over SSH accepted; then the user suspended (`PutAccount { suspended: true }`
# on `account_shard`) and `n` pushes over SSH with the same key, each of which must fail with no ref moved; then the
# suspension lifted and a push accepted again. The key stays listed through all of it, so the refusal is the leader's
# `Account` row check, not the key lookup.
#   HOST=<ssh host> tests/m6/hosted/bf52-ssh-suspend.sh <n> [login] [account id]
set -uo pipefail
n=${1:-20}; login=${2:-m6-ssh-suspend}; acct=${3:-7290061}
here=$(cd "$(dirname "$0")" && pwd)
EDGE=${GRAFT_EDGE:?}; TOOLS=${GRAFT_TOOLS_EDGE:?}; HOST=${HOST:?}
umask 077
w=$(mktemp -d); trap 'rm -rf "$w"' EXIT
log() { echo "$(date -u +%FT%T.%3NZ) $*"; }
tools() { curl -s -m 300 -X POST -H @- --data "$2" "$TOOLS$1" <<<"authorization: Bearer $(cat "$here/../../m0/.m0-token")"; }
rid() { python3 -c 'import os,time; v=(int(time.time()*1000)<<80)|int.from_bytes(os.urandom(10),"big"); A="0123456789ABCDEFGHJKMNPQRSTVWXYZ"; print("".join(A[(v>>(5*i))&31] for i in range(25,-1,-1)))'; }
shard() { python3 -c 'import hashlib,struct,sys; t,v=sys.argv[1],sys.argv[2]; b=struct.pack(">q",int(v)) if t in("account","repo") else v.lower().encode(); print(int.from_bytes(hashlib.sha256(b"graftyard/names/"+t.encode()+b"/"+b).digest()[:4],"big")%1024)' "$1" "$2"; }
cmd() {
  tools /create "{\"chain\":\"$1\"}" >/dev/null
  tools /commit "{\"chain\":\"$1\",\"cmd\":{\"cred\":{\"account\":null,\"key\":null},\"op\":$2},\"rid\":\"$(rid)\"}" | jq -c '.outcome // .'
}
as=$(shard account "$acct")
suspend() { cmd "names:$as" "{\"PutAccount\":{\"id\":$acct,\"suspended\":$1,\"sessions_not_before\":0}}"; }
{
  suspend false
  cmd "names:$(shard login "$login")" "{\"ClaimLogin\":{\"login\":\"$login\",\"kind\":\"User\",\"account\":$acct}}"
  cmd "names:$as" "{\"SetAccountLogin\":{\"account\":$acct,\"login\":\"$login\"}}"
  cmd "names:$as" '{"InitPlacement":{"owner":'$acct',"record":{"name":"Tri","sites":[0,1,2],"home":0,"generation":1,"moving_from":null,"at":5}}}'
  cmd "authz:$acct" '{"Open":{"kind":"User","enterprise":null}}'
  # `identity:` goes through the test edge's bed-token route, which finds it under the owner's `Placement` row.
  curl -s -m 120 -X POST -H @- --data "{\"chain\":\"identity:$acct\",\"cmd\":{\"cred\":{\"account\":null,\"key\":null},\"op\":{\"Create\":{\"login\":\"$login\",\"kind\":\"User\",\"created\":$(date +%s%3N)}}}}" \
    "$EDGE/_bed/identity" <<<"x-bed-token: $(cat "${BED_TOKEN_FILE:?}")" | jq -c '.outcome // .'
} > "$w/seed.log" 2>&1
python3 - > "$w/pat.json" <<'EOF'
import hashlib, os, zlib, json
A = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz"
s = ""
while len(s) < 30:
    for b in os.urandom(32):
        if b < 248 and len(s) < 30:
            s += A[b % 62]
v = zlib.crc32(s.encode()); tail = ""
for _ in range(6):
    tail = A[v % 62] + tail; v //= 62
h = hashlib.sha256(b"graftyard/token/" + s.encode()).digest()
print(json.dumps({"text": "ghp_" + s + tail, "hash": list(h), "shard": int.from_bytes(h[:4], "big") % 3072}))
EOF
jq -r .text "$w/pat.json" > "$w/pat"
cmd "tokens:$(jq -r .shard "$w/pat.json")" "{\"Store\":{\"hash\":$(jq -c .hash "$w/pat.json"),\"kind\":\"Pat\",\"account\":$acct,\"scopes\":1,\"expires\":null}}" >> "$w/seed.log"
printf 'Authorization: token %s\nUser-Agent: graftyard-bf52\ncontent-type: application/json\n' "$(cat "$w/pat")" > "$w/h"
api() { curl -sS -o "$w/body" -w '%{http_code}' -H @"$w/h" -X "$1" "$EDGE$2" ${3:+--data-binary "$3"}; }
ssh-keygen -q -t ed25519 -N "" -C bf52 -f "$w/id"
log "seed: $(grep -c '"ok"' "$w/seed.log") of $(wc -l < "$w/seed.log") ok"
log "key add: $(api POST /api/v3/user/keys "{\"title\":\"bf52\",\"key\":\"$(cat "$w/id.pub")\"}")"
repo="bf52-$(date +%s)"
log "repo create: $(api POST /api/v3/user/repos "{\"name\":\"$repo\",\"private\":true,\"auto_init\":true}") $login/$repo"
export GIT_SSH_COMMAND="ssh -i $w/id -o IdentitiesOnly=yes -o StrictHostKeyChecking=accept-new -o UserKnownHostsFile=$w/known -o BatchMode=yes -o ConnectTimeout=30" GIT_TERMINAL_PROMPT=0
g() { git -c user.name=bf52 -c user.email=bf52@x "$@"; }
printf '#!/bin/sh\n[ "$1" = get ] || exit 0\necho username=x\necho "password=$(cat %s)"\n' "$w/pat" > "$w/cred"; chmod +x "$w/cred"
head_https() { git -c credential.helper= -c credential.helper="$w/cred" ls-remote "$EDGE/$login/$repo.git" refs/heads/main | cut -f1; }
g clone -q "git@$HOST:$login/$repo.git" "$w/r" 2>"$w/err" || { log "clone failed: $(tr '\n' ' ' < "$w/err")"; exit 1; }
cd "$w/r" || exit 1
push() {
  local before rc after
  before=$(head_https)
  echo "$1 $(date +%s%N)" >> log.txt; g add log.txt; g commit -qm "$1"
  g push -q origin HEAD:main 2>"$w/err"; rc=$?
  after=$(head_https)
  if [ $rc -eq 0 ] && [ "$after" = "$(git rev-parse HEAD)" ]; then echo ok; else
    git reset -q --hard HEAD~1
    echo "refused rc$rc moved=$([ "$after" = "$before" ] && echo no || echo yes) $(grep -oE '(401|403|Permission denied|suspended|Unauthorized|Bad credentials)' "$w/err" | head -1)"
  fi
}
log "push before suspension: $(push before)"
log "suspend: $(suspend true)"
refused=0; moved=0
for i in $(seq 1 "$n"); do
  r=$(push "suspended $i"); case "$r" in refused*moved=no*) refused=$((refused+1));; esac; case "$r" in *moved=yes*|ok) moved=$((moved+1));; esac
  [ "$i" = 1 ] && log "push 1 while suspended: $r"
done
log "while suspended: $refused of $n pushes over ssh refused with no ref moved, $moved accepted or moved"
log "unsuspend: $(suspend false)"
log "push after unsuspend: $(push after)"
