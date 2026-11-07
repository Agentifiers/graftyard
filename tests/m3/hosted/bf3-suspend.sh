#!/usr/bin/env bash
# BF-3's suspension half over HTTPS (`build/BUILD-PLAN.md`, M3-WP3): a user's own repository, pushed by stock git
# with the user's PAT; then the user is suspended (`PutAccount { suspended: true }` on `account_shard`, as the
# operator) and every push with the same PAT must be refused with no ref moved, `n` times; then the suspension is
# lifted and a push is accepted again. Seeds the user, its `Placement` row, its `Authz` chain, the repository, and
# the PAT through the M1-WP7 tools edge as the operator (as setup.ts does). Tokens reach git through a credential
# helper reading a 0600 file, and curl through stdin.
#
#   tests/m3/hosted/bf3-suspend.sh <out dir> [n] [login]
set -uo pipefail
out=${1:?out}; n=${2:-100}; login=${3:-m3-suspend}
here=$(cd "$(dirname "$0")" && pwd)
EDGE=${GRAFT_EDGE:-https://graft-m1-edge.<workers-subdomain>.workers.dev}
TOOLS=${GRAFT_TOOLS_EDGE:-https://graft-m1-wp7-edge.<workers-subdomain>.workers.dev}
acct=${BF3_ACCOUNT:-7290001}
mkdir -p "$out"; out=$(cd "$out" && pwd); umask 077
work=$(mktemp -d); trap 'rm -rf "$work"' EXIT
tok=$work/tok
tools() { curl -s -m 300 -X POST -H @- --data "$2" "$TOOLS$1" <<<"authorization: Bearer $(cat "$here/../../m0/.m0-token")"; }
rid() { python3 -c 'import os,time; v=(int(time.time()*1000)<<80)|int.from_bytes(os.urandom(10),"big"); A="0123456789ABCDEFGHJKMNPQRSTVWXYZ"; print("".join(A[(v>>(5*i))&31] for i in range(25,-1,-1)))'; }
shard() { python3 -c 'import hashlib,struct,sys; t,v=sys.argv[1],sys.argv[2]; b=struct.pack(">q",int(v)) if t in("account","repo") else v.lower().encode(); print(int.from_bytes(hashlib.sha256(b"graftyard/names/"+t.encode()+b"/"+b).digest()[:4],"big")%1024)' "$1" "$2"; }
cmd() { # cmd CHAIN JSON-OP: commits as the operator, prints the outcome
  tools /create "{\"chain\":\"$1\"}" >/dev/null
  tools /commit "{\"chain\":\"$1\",\"cmd\":{\"cred\":{\"account\":null,\"key\":null},\"op\":$2},\"rid\":\"$(rid)\"}" | jq -c '.outcome // .'
}
as=$(shard account "$acct")
suspend() { cmd "names:$as" "{\"PutAccount\":{\"id\":$acct,\"suspended\":$1,\"sessions_not_before\":0}}"; }
{
  echo "seed $login=$acct"
  suspend false
  cmd "names:$(shard login "$login")" "{\"ClaimLogin\":{\"login\":\"$login\",\"kind\":\"User\",\"account\":$acct}}"
  cmd "names:$as" "{\"SetAccountLogin\":{\"account\":$acct,\"login\":\"$login\"}}"
  cmd "names:$as" '{"InitPlacement":{"owner":'$acct',"record":{"name":"Tri","sites":[0,1,2],"home":0,"generation":1,"moving_from":null,"at":5}}}'
  cmd "authz:$acct" '{"Open":{"kind":"User","enterprise":null}}'
  o=$(cmd "authz:$acct" '{"CreateRepo":{"name":"bf3"}}'); echo "$o"
  repo=$(jq -r '.ok.out.Repo.repo // empty' <<<"$o")
  [ -n "$repo" ] || repo=$(tools /read "{\"chain\":\"authz:$acct\",\"q\":{\"repo\":\"bf3\"}}" | jq -r '.value.repo.repo')
  cmd "names:$(shard repo "$repo")" "{\"RepoOwner\":{\"repo\":$repo,\"owner\":$acct}}"
  echo "repo $login/bf3=$repo"
} > "$out/seed.log" 2>&1
# A PAT for the user: newpat.mjs's shape, for an account it does not know.
python3 - "$acct" > "$work/pat.json" <<'EOF'
import hashlib, os, sys, zlib
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
import json
print(json.dumps({"text": "ghp_" + s + tail, "hash": list(h), "shard": int.from_bytes(h[:4], "big") % 3072}))
EOF
jq -r .text "$work/pat.json" > "$tok"
pshard=$(jq -r .shard "$work/pat.json")
echo "store $(cmd "tokens:$pshard" "{\"Store\":{\"hash\":$(jq -c .hash "$work/pat.json"),\"kind\":\"Pat\",\"account\":$acct,\"scopes\":1,\"expires\":null}}")" >> "$out/seed.log"
cat > "$work/cred.sh" <<'EOF'
#!/bin/sh
[ "$1" = get ] || exit 0
echo username=x
echo "password=$(cat "$GRAFT_TOKFILE")"
EOF
chmod +x "$work/cred.sh"
export GRAFT_TOKFILE=$tok GIT_TERMINAL_PROMPT=0
g() { git -c credential.helper= -c credential.helper="$work/cred.sh" -c user.name=bf3 -c user.email=bf3@x "$@"; }
url="$EDGE/$login/bf3.git"
mkdir "$work/r" && cd "$work/r" && git init -q -b main
remote_head() { g ls-remote "$url" refs/heads/main 2>/dev/null | cut -f1; }
push() { # push LABEL: ok, or refused <why> moved=<yes|no>
  local before rc after
  before=$(remote_head)
  echo "$1 $(date +%s%N)" >> log.txt; git add log.txt; g commit -qm "$1"
  g push -q "$url" HEAD:main 2>"$work/err"; rc=$?
  after=$(remote_head)
  if [ $rc -eq 0 ] && [ "$after" = "$(git rev-parse HEAD)" ]; then echo ok; else
    git reset -q --hard HEAD~1 2>/dev/null || true
    why=$(grep -oE "(401|403|404|503|Authentication failed|Permission denied|not found)" "$work/err" | head -1)
    echo "refused ${why:-rc$rc} moved=$([ "$after" = "$before" ] && echo no || echo yes)"
  fi
}
printf 'step\tresult\n' > "$out/bf3.tsv"
b=$(push before); printf 'push before suspension\t%s\n' "$b" >> "$out/bf3.tsv"
printf 'suspend\t%s\n' "$(suspend true)" >> "$out/bf3.tsv"
refused=0
for i in $(seq 1 "$n"); do
  r=$(push "suspended-$i"); printf 'push %s while suspended\t%s\n' "$i" "$r" >> "$out/bf3.tsv"
  case $r in "refused "*"moved=no") refused=$((refused+1));; esac
done
printf 'unsuspend\t%s\n' "$(suspend false)" >> "$out/bf3.tsv"
a=$(push after); printf 'push after unsuspension\t%s\n' "$a" >> "$out/bf3.tsv"
echo "before=[$b] suspended_pushes=$n refused_unmoved=$refused after=[$a]" | tee "$out/bf3.summary"
