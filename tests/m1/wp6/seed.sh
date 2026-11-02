#!/bin/bash
# Seeds the stand-in group of each bed repository with the identity rows its `deps` read: the owner (account 100) and
# a writer, a read-only user, and a revoked token. Tokens are made by mktok.py into $WP6_DIR/tokens.txt, one
# "<token> <stored hash>" per line, in that order. Usage: seed.sh <repo id>...
set -euo pipefail
: "${WP6_URL:?}" "${WP6_DIR:?}"
BED=$(cat "$WP6_DIR/bed.token")
mapfile -t T < "$WP6_DIR/tokens.txt"
h() { echo "${T[$1]}" | cut -d' ' -f2; }
for repo in "$@"; do
  body=$(cat <<JSON
{"repo": $repo, "owner": 100, "reads": [[32, 1]], "users": [
  {"account": 100, "token_hash": "$(h 0)"},
  {"account": 101, "token_hash": "$(h 1)", "write": true},
  {"account": 102, "token_hash": "$(h 2)"},
  {"account": 103, "token_hash": "$(h 3)", "write": true, "revoked": true}]}
JSON
)
  curl -sf -X POST -H "x-bed-token: $BED" --data "$body" "$WP6_URL/_bed/seed?repo=$repo"; echo
done
