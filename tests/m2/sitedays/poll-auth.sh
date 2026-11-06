#!/usr/bin/env bash
# Polls an edge once a second with a valid token (GraphQL and git info/refs) and every tenth second with a
# well-formed unknown token, logging one line per request: UTC time, probe, HTTP status, seconds, and the body's first 120 bytes.
# Usage: poll-auth.sh <edge https url> <owner/repo> <tokens.txt> <token line> <seconds> > log
set -u
E=$1; REPO=$2; TOKENS=$3; LINE=$4; FOR=$5
TOK=$(sed -n "${LINE}p" "$TOKENS" | cut -d" " -f1)
# A well-formed token no account holds: a valid checksum, so the edge reads its Tokens shard and finds no row.
BAD=$(python3 -c '
import random, zlib
A = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz"
s = "".join(random.choice(A) for _ in range(30)); v = zlib.crc32(s.encode()); t = ""
for _ in range(6): t = A[v % 62] + t; v //= 62
print("ghp_" + s + t)')
OWNER=${REPO%/*}; NAME=${REPO#*/}
Q="{\"query\":\"query{repository(owner:\\\"$OWNER\\\",name:\\\"$NAME\\\"){name}}\"}"
probe() {
  local what=$1; shift
  local out; out=$(mktemp)
  local r; r=$(curl -s -m 60 -o "$out" -w "%{http_code} %{time_total}" "$@")
  printf '%s %s %s %s\n' "$(date -u +%H:%M:%S)" "$what" "$r" "$(head -c 120 "$out" | tr -d '\000' | tr '\n' ' ')"
  rm -f "$out"
}
end=$(( $(date +%s) + FOR )); n=0
while [ "$(date +%s)" -lt "$end" ]; do
  probe gql-valid -H "Authorization: token $TOK" -d "$Q" "$E/api/graphql" &
  probe git-valid -u "x:$TOK" "$E/$REPO.git/info/refs?service=git-receive-pack" &
  [ $((n % 10)) -eq 0 ] && probe gql-unknown -H "Authorization: token $BAD" -d "$Q" "$E/api/graphql" &
  n=$((n + 1)); sleep 1
done
wait
