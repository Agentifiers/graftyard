#!/usr/bin/env bash
# `git ls-remote` (protocol v0 and v2, upload-pack and receive-pack) and a `git clone` of m2b/bf20 through graft-m2b-edge,
# each timed, with its ref count. The token (line 2 of ~/.graft-m2b/tokens.txt, as tests/m2/sitedays/shards.sh) reaches
# git through its environment, never argv.
#   bash tests/m3/wirefix/git-bf20.sh [repo]   (default bf20)
set -u
R=${1:-bf20}
E=https://graft-m2b-edge.${CF_WORKERS_SUBDOMAIN:?}.workers.dev
TOK=$(sed -n 2p "$HOME/.graft-m2b/tokens.txt" | cut -d" " -f1)
export GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=http.extraHeader GIT_CONFIG_VALUE_0="Authorization: Basic $(printf 'x:%s' "$TOK" | base64 -w0)"
unset TOK
export GIT_TERMINAL_PROMPT=0
W=$(mktemp -d); trap 'rm -rf $W' EXIT
(umask 077; printf '%s\n' "$GIT_CONFIG_VALUE_0" > $W/hdr)
run() {
  local name=$1; shift
  local t0=$(date +%s%N)
  "$@" > $W/out 2> $W/err; local rc=$?
  local t1=$(date +%s%N)
  printf '%s %s rc=%s %sms lines=%s bytes=%s %s\n' "$(date -u +%T)" "$name" "$rc" "$(( (t1 - t0) / 1000000 ))" \
    "$(wc -l < $W/out)" "$(wc -c < $W/out)" "$(tail -c 300 $W/err | tr '\n' ' ')"
  head -3 $W/out | cut -c1-120
}
run ls-remote-v0 git -c protocol.version=0 ls-remote "$E/m2b/$R.git"
run ls-remote-v2 git -c protocol.version=2 ls-remote "$E/m2b/$R.git"
run ls-remote-v2-main git -c protocol.version=2 ls-remote "$E/m2b/$R.git" refs/heads/main
run receive-pack-adv curl -s -m 600 -o $W/adv -w '%{http_code} %{size_download}\n' -H @$W/hdr "$E/m2b/$R.git/info/refs?service=git-receive-pack"
[ -f $W/adv ] && echo "receive-pack advertisement: $(wc -c < $W/adv) bytes, $(grep -a -c 'refs/' $W/adv) pkt lines with refs/"
run clone git clone -q --no-checkout "$E/m2b/$R.git" $W/c
[ -d $W/c ] && echo "clone: $(git -C $W/c for-each-ref | wc -l) refs, HEAD $(git -C $W/c rev-parse --short HEAD 2>/dev/null), objects $(git -C $W/c count-objects -v | tr '\n' ' ')"
