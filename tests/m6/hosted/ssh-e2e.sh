#!/usr/bin/env bash
# B0-24d end to end (build/BUILD-PLAN.md, M6-WP2): native git over SSH through a Spectrum Worker app on a Pro zone,
# with the stock OpenSSH client and stock git, no cloudflared. Steps, each logged with a UTC time:
#   1. a new ed25519 key, added to m3-member with `POST /user/keys` (HTTPS, the member's PAT);
#   2. a new public repository `m3-member/ssh-<tag>` with a README (`POST /user/repos`, auto_init);
#   3. `git clone git@<host>:m3-member/ssh-<tag>.git`, then a commit and `git push`; the pushed commit is read back
#      over HTTPS (`ls-remote`), so the push went through the edge's write path to the group;
#   4. a session held for HOLD_S seconds: `ssh -o ServerAliveInterval=30 git@<host> git-upload-pack ...` reads the v2
#      advertisement, then sends one ls-refs request every 60 s on the same connection and reads each answer;
#   5. the key removed with `DELETE /user/keys/{id}`; a clone and a push with it must then fail with "Permission denied".
# Prints one line per step; the PAT reaches curl and git through files, never argv.
#   HOST=<ssh hostname> HOLD_S=600 tests/m6/hosted/ssh-e2e.sh <state dir> <tag> > tests/m6/raw/<tag>.log
set -uo pipefail
state=${1:?state}; tag=${2:?tag}
EDGE=${GRAFT_EDGE:?}; HOST=${HOST:?}; HOLD=${HOLD_S:-600}
umask 077
w=$(mktemp -d); trap 'rm -rf "$w"' EXIT
log() { echo "$(date -u +%FT%T.%3NZ) $*"; }
awk '$1=="member"{print $2}' "$state/m3-tokens.txt" > "$w/pat"
printf 'Authorization: token %s\nUser-Agent: graftyard-ssh-e2e\ncontent-type: application/json\n' "$(cat "$w/pat")" > "$w/h"
api() { curl -sS -o "$w/body" -w '%{http_code}' -H @"$w/h" -X "$1" "$EDGE$2" ${3:+--data-binary "$3"}; }
repo="ssh-$tag"
ssh-keygen -q -t ed25519 -N "" -C "ssh-e2e-$tag" -f "$w/id"
SSHC="ssh -i $w/id -o IdentitiesOnly=yes -o StrictHostKeyChecking=accept-new -o UserKnownHostsFile=$w/known -o BatchMode=yes -o ConnectTimeout=30"
export GIT_SSH_COMMAND="$SSHC" GIT_TERMINAL_PROMPT=0
g() { git -c user.name=ssh-e2e -c user.email=ssh-e2e@x "$@"; }

code=$(api POST /api/v3/user/keys "{\"title\":\"ssh-e2e $tag\",\"key\":\"$(cat "$w/id.pub")\"}")
id=$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1])).get("id",""))' "$w/body")
log "key add: $code id $id fingerprint $(ssh-keygen -lf "$w/id.pub" | awk '{print $2}')"
[ "$code" = 201 ] || { cat "$w/body"; exit 1; }
code=$(api POST /api/v3/user/repos "{\"name\":\"$repo\",\"private\":false,\"auto_init\":true}")
rid=$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1])).get("id",""))' "$w/body")
log "repo create: $code m3-member/$repo id $rid"
[ "$code" = 201 ] || { cat "$w/body"; exit 1; }

t=$(date +%s%N); out=$(g clone "git@$HOST:m3-member/$repo.git" "$w/c" 2>&1); rc=$?
log "clone over ssh: rc $rc in $(( ($(date +%s%N) - t) / 1000000 )) ms; $(echo "$out" | tr '\n' ' ' | cut -c1-300)"
[ $rc = 0 ] || exit 1
log "host key learned: $(ssh-keygen -lf "$w/known" | awk '{print $2, $4}' | tr '\n' ' ')"
cd "$w/c" || exit 1
head -c 200000 /dev/urandom | base64 > data.txt; echo "ssh push $tag" > ssh.txt
g add -A && g commit -qm "push over ssh $tag"
t=$(date +%s%N); out=$(g push origin HEAD:main 2>&1); rc=$?
log "push over ssh: rc $rc in $(( ($(date +%s%N) - t) / 1000000 )) ms; $(echo "$out" | tr '\n' ' ' | cut -c1-300)"
printf '#!/bin/sh\n[ "$1" = get ] || exit 0\necho username=x\necho "password=$(cat %s)"\n' "$w/pat" > "$w/cred"; chmod +x "$w/cred"
remote=$(git -c credential.helper= -c credential.helper="$w/cred" ls-remote "$EDGE/m3-member/$repo.git" refs/heads/main | cut -f1)
log "main over https after the push: $remote; local $(git rev-parse HEAD); $([ "$remote" = "$(git rev-parse HEAD)" ] && echo MATCH || echo MISMATCH)"
t=$(date +%s%N); out=$(g fetch origin 2>&1); rc=$?
log "fetch over ssh: rc $rc in $(( ($(date +%s%N) - t) / 1000000 )) ms"
# A second push on a delta of the first, and a branch create and delete.
echo more >> ssh.txt; g commit -qam "second $tag"
out=$(g push origin HEAD:main HEAD:refs/heads/side 2>&1); rc=$?; log "second push (main + new branch): rc $rc"
out=$(g push origin :side 2>&1); rc=$?; log "branch delete: rc $rc"

# The held session: one SSH connection, protocol v2 upload-pack, an ls-refs every 60 s.
python3 - "$HOST" "$w/id" "$w/known" "$repo" "$HOLD" <<'PY'
import subprocess, sys, time, datetime
host, key, known, repo, hold = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4], int(sys.argv[5])
def log(s): print(datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.%f")[:-3] + "Z " + s, flush=True)
p = subprocess.Popen(["ssh", "-i", key, "-o", "IdentitiesOnly=yes", "-o", f"UserKnownHostsFile={known}", "-o", "BatchMode=yes",
                      "-o", "ServerAliveInterval=30", "-o", "SetEnv=GIT_PROTOCOL=version=2", f"git@{host}",
                      f"git-upload-pack 'm3-member/{repo}.git'"], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
def pkt():
    n = p.stdout.read(4)
    if len(n) < 4: raise EOFError("closed")
    n = int(n, 16)
    return b"" if n <= 2 else p.stdout.read(n - 4)
def section():
    lines = []
    while (l := pkt()) != b"": lines.append(l)
    return lines
start = time.time()
adv = section(); log(f"held session: advertisement {len(adv)} lines, first {adv[0][:20]!r}")
def line(s): return b"%04x%s" % (len(s) + 4, s)
n = ok = 0
while time.time() - start < hold:
    n += 1
    req = line(b"command=ls-refs\n") + line(b"object-format=sha1\n") + b"0001" + line(b"ref-prefix refs/heads/\n") + b"0000"
    try:
        p.stdin.write(req); p.stdin.flush()
        refs = section(); ok += 1
    except Exception as e:
        log(f"held session: ls-refs {n} failed after {time.time() - start:.0f} s: {e}"); break
    if n == 1 or n % 5 == 0: log(f"held session: ls-refs {n} ok at {time.time() - start:.0f} s, {len(refs)} refs")
    time.sleep(min(60, max(0, hold - (time.time() - start))))
p.stdin.write(b"0000"); p.stdin.flush(); p.stdin.close(); rc = p.wait(30)
log(f"held session: {time.time() - start:.0f} s, {ok} of {n} ls-refs answered, ssh exit {rc}; {'PASS' if ok == n and time.time() - start >= hold else 'FAIL'}")
PY

code=$(api DELETE "/api/v3/user/keys/$id"); log "key delete: $code"
out=$(g clone "git@$HOST:m3-member/$repo.git" "$w/c2" 2>&1); rc=$?
log "clone after delete: rc $rc; $(echo "$out" | grep -oE 'Permission denied[^.]*' | head -1)"
echo late >> ssh.txt; g commit -qam "after delete $tag"; before=$remote
out=$(g push origin HEAD:main 2>&1); rc=$?
after=$(git -c credential.helper= -c credential.helper="$w/cred" ls-remote "$EDGE/m3-member/$repo.git" refs/heads/main | cut -f1)
log "push after delete: rc $rc; $(echo "$out" | grep -oE 'Permission denied[^.]*' | head -1); main $( [ "$after" = "$(git rev-parse HEAD)" ] && echo MOVED || echo unchanged)"
log "repo id $rid"
