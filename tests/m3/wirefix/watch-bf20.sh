#!/usr/bin/env bash
# Every minute until killed: a `git ls-remote` of m2b/bf20's main through graft-m2b-edge (so the group serves a request
# and its leader resumes or is elected), then each member's role, term, commit and leader from the tools edge. One line
# per minute to stdout. The token reaches git through its environment, never argv.
#   bash tests/m3/wirefix/watch-bf20.sh >> <log>
set -u
E=https://graft-m2b-edge.<workers-subdomain>.workers.dev
TOOLS=https://graft-m2b-tools-edge.<workers-subdomain>.workers.dev
TOK=$(sed -n 2p "$HOME/.graft-m2b/tokens.txt" | cut -d" " -f1)
export GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=http.extraHeader GIT_CONFIG_VALUE_0="Authorization: Basic $(printf 'x:%s' "$TOK" | base64 -w0)"
unset TOK
export GIT_TERMINAL_PROMPT=0
H=$(mktemp); trap 'rm -f $H' EXIT
(umask 077; printf 'authorization: Bearer %s\n' "$(cat /home/user/workspace/repo/tests/m0/.m0-token)" > $H)
while true; do
  t0=$(date +%s%N)
  ls=$(git -c protocol.version=2 ls-remote "$E/m2b/bf20.git" refs/heads/main 2>&1 | cut -c1-12)
  ms=$(( ($(date +%s%N) - t0) / 1000000 ))
  line="$(date -u +%FT%TZ) ls-remote=${ls:-none} ${ms}ms"
  for s in 0 1 2; do
    r=$(curl -s -m 30 -X POST -H @$H "$TOOLS/member" \
      -d "{\"chain\":\"repo:65536\",\"site\":$s,\"path\":\"/status\",\"deadline\":20000}" | python3 -c "
import json,sys
try:
  d=json.load(sys.stdin)['v']; print(f\"{d['site']}:{d['role']}/t{d['term']}/c{d['commit']}/L{d['leader']}/led{d['ledTerm']}\")
except Exception: print('$s:err')")
    line="$line $r"
  done
  echo "$line"
  sleep 60
done
