#!/usr/bin/env bash
# BF-46's `SshHostKeys` traffic (`tests/m5/bf46/ssh-row.sh`): stock OpenSSH and git through ssh.graftyard.dev every
# INTERVAL_S seconds until <stop file> exists. One ed25519 user key is added to m3-member for the run and deleted at
# the end. Each round writes one JSONL row:
#   - `kex`: a fresh `ssh-keyscan -t ed25519,ecdsa,rsa` of the host, the three SHA-256 fingerprints it served;
#   - `ls`: `git ls-remote` over SSH with an empty known_hosts each time (`StrictHostKeyChecking=accept-new`), its exit,
#     its ms, and the ed25519 fingerprint the session's key exchange used;
#   - `pinned`: the same with the known_hosts of the run's first session kept (`StrictHostKeyChecking=yes`), as a client
#     that learned slot A at the start; after the switch it fails unless it learned B (UpdateHostKeys) first;
#   - `push`: a commit pushed over SSH every PUSH_EVERY rounds, read back over HTTPS.
# The PAT reaches curl and git through files, never argv.
#   source tests/am1/lf13/env-m3f.sh; bash tests/m5/bf46/ssh-probe.sh <out.jsonl> <stop file>
set -uo pipefail
out=${1:?out}; stop=${2:?stop}
EDGE=${GRAFT_EDGE:?}; HOST=${SSH_HOST:-ssh.graftyard.dev}; IV=${INTERVAL_S:-20}; PE=${PUSH_EVERY:-6}
REPO=${PROBE_REPO:-m3-member/bf46-ssh}
umask 077
w=$(mktemp -d)
awk '$1=="member"{print $2}' "$M3_STATE/m3-tokens.txt" > "$w/pat"
printf 'Authorization: token %s\nUser-Agent: graftyard-bf46-ssh\ncontent-type: application/json\n' "$(cat "$w/pat")" > "$w/h"
api() { curl -sS -m 60 -o "$w/body" -w '%{http_code}' -H @"$w/h" -X "$1" "$EDGE$2" ${3:+--data-binary "$3"}; }
now() { date -u +%FT%T.%3NZ; }
ms() { echo $(( $(date +%s%N) / 1000000 )); }
ssh-keygen -q -t ed25519 -N "" -C "bf46-ssh" -f "$w/id"
code=$(api POST /api/v3/user/keys "{\"title\":\"bf46 ssh probe\",\"key\":\"$(cat "$w/id.pub")\"}")
kid=$(jq -r '.id // empty' "$w/body")
echo "{\"at\":\"$(now)\",\"kind\":\"setup\",\"key_add\":$code,\"key\":\"${kid}\"}" >> "$out"
[ "$code" = 201 ] || { rm -rf "$w"; exit 1; }
cleanup() { [ -n "$kid" ] && echo "{\"at\":\"$(now)\",\"kind\":\"teardown\",\"key_delete\":$(api DELETE "/api/v3/user/keys/$kid")}" >> "$out"; rm -rf "$w"; }
trap cleanup EXIT
code=$(api GET "/api/v3/repos/$REPO")
if [ "$code" != 200 ]; then
  code=$(api POST /api/v3/user/repos "{\"name\":\"${REPO#*/}\",\"private\":false,\"auto_init\":true}")
  echo "{\"at\":\"$(now)\",\"kind\":\"setup\",\"repo_create\":$code,\"repo\":$(jq '.id // null' "$w/body")}" >> "$out"
fi
printf '#!/bin/sh\n[ "$1" = get ] || exit 0\necho username=x\necho "password=$(cat %s)"\n' "$w/pat" > "$w/cred"; chmod +x "$w/cred"
# `UpdateHostKeys=yes` as a stock client has it: OpenSSH turns it on by default only without a `UserKnownHostsFile`
# option, which every session here sets. Without it, m3fs1's round 43 cloned on slot A and pushed to a front that had
# just switched to B, and strict checking refused the push.
SSHC="ssh -i $w/id -o IdentitiesOnly=yes -o BatchMode=yes -o ConnectTimeout=30 -o LogLevel=DEBUG1 -o UpdateHostKeys=yes"
g() { git -c user.name=bf46 -c user.email=bf46@x "$@"; }
# The ed25519 key the session's key exchange used, from ssh's own debug line.
kexfp() { grep -oE 'Server host key: ssh-ed25519 SHA256:[A-Za-z0-9+/]+' "$1" | head -1 | awk '{print $NF}'; }
n=0
while [ ! -e "$stop" ]; do
  n=$((n + 1)); t0=$(ms)
  # `ssh -E` appends, so each round starts its logs empty: before this, `kexfp` read the run's first key exchange in
  # every round (m3fs1 and m3fs2's `ls.fp` and `pinned.fp`; their `kex` scans and exit codes are unaffected).
  : > "$w/ls.err"; : > "$w/pin.err"; : > "$w/push.err"
  scan=$(ssh-keyscan -T 20 -t ed25519,ecdsa,rsa "$HOST" 2>/dev/null | ssh-keygen -lf - 2>/dev/null | awk '{print $2}' | sort | paste -sd, -)
  : > "$w/fresh"
  t=$(ms); GIT_SSH_COMMAND="$SSHC -o StrictHostKeyChecking=accept-new -o UserKnownHostsFile=$w/fresh -E $w/ls.err" \
    git ls-remote "git@$HOST:$REPO.git" HEAD > "$w/ls.out" 2>/dev/null; rc=$?; lms=$(( $(ms) - t ))
  fp=$(kexfp "$w/ls.err")
  [ -s "$w/pinned" ] || cp "$w/fresh" "$w/pinned"
  t=$(ms); GIT_SSH_COMMAND="$SSHC -o StrictHostKeyChecking=yes -o UpdateHostKeys=yes -o UserKnownHostsFile=$w/pinned -E $w/pin.err" \
    git ls-remote "git@$HOST:$REPO.git" HEAD > /dev/null 2>&1; prc=$?; pms=$(( $(ms) - t ))
  pfp=$(kexfp "$w/pin.err"); learned=$(ssh-keygen -lf "$w/pinned" 2>/dev/null | awk '{print $2}' | sort | paste -sd, -)
  push=null
  if [ $((n % PE)) = 1 ]; then
    rm -rf "$w/c"; t=$(ms)
    GIT_SSH_COMMAND="$SSHC -o StrictHostKeyChecking=accept-new -o UserKnownHostsFile=$w/fresh -E $w/push.err" \
      g clone -q "git@$HOST:$REPO.git" "$w/c" 2>/dev/null; crc=$?
    prc2=1; match=false
    if [ $crc = 0 ]; then
      (cd "$w/c" && echo "$n $(now)" >> probe.txt && g add probe.txt && g commit -qm "bf46 ssh probe $n" &&
        GIT_SSH_COMMAND="$SSHC -o StrictHostKeyChecking=accept-new -o UserKnownHostsFile=$w/fresh -E $w/push.err" \
          g push -q origin HEAD:main 2>/dev/null); prc2=$?
      head=$(git -C "$w/c" rev-parse HEAD)
      remote=$(git -c credential.helper= -c credential.helper="$w/cred" ls-remote "$EDGE/$REPO.git" refs/heads/main | cut -f1)
      [ "$remote" = "$head" ] && match=true
    fi
    push="{\"clone\":$crc,\"push\":$prc2,\"https_match\":$match,\"ms\":$(( $(ms) - t ))}"
  fi
  echo "{\"at\":\"$(now)\",\"kind\":\"round\",\"n\":$n,\"kex\":\"$scan\",\"ls\":{\"rc\":$rc,\"ms\":$lms,\"fp\":\"$fp\"},\"pinned\":{\"rc\":$prc,\"ms\":$pms,\"fp\":\"$pfp\",\"learned\":\"$learned\"},\"push\":$push}" >> "$out"
  sleep $(( IV - ($(ms) - t0) / 1000 > 0 ? IV - ($(ms) - t0) / 1000 : 0 ))
done
