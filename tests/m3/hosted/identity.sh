#!/usr/bin/env bash
# The identity REST routes journey 4 uses, called once each as m3-admin, each reply kept for validate.py
# (rest-cases.py): `GET /user`, `GET /users/{u}` for a user and the org, `GET /orgs/{org}/memberships/{u}`, an
# unknown user's 404, an installation token's `GET /user` (403), and `DELETE /applications/{gh}/token` for a fresh
# PAT (204). Tokens reach curl on stdin.
#
#   tests/m3/hosted/identity.sh <state dir> <out dir>
set -uo pipefail
state=${1:?state}; out=${2:?out}
here=$(cd "$(dirname "$0")" && pwd)
EDGE=${GRAFT_EDGE:-https://graft-m1-edge.<workers-subdomain>.workers.dev}
mkdir -p "$out/identity"; umask 077
work=$(mktemp -d); trap 'rm -rf "$work"' EXIT
awk '$1=="admin"{print $2}' "$state/m3-tokens.txt" > "$work/admin"
call() { # call NAME METHOD PATH OPENAPI TOKENFILE
  local s
  s=$(curl -s -m 60 -o "$work/body" -w '%{http_code}' -X "$2" -H @- "$EDGE/api/v3$3" <<<"authorization: token $(cat "$5")")
  jq -n --arg m "$2" --arg p "$4" --argjson s "$s" --slurpfile b <(cat "$work/body" 2>/dev/null | jq -c . 2>/dev/null || echo null) \
    '{method: $m, path: $p, status: $s, body: ($b[0] // null)}' > "$out/identity/$1.json"
  echo "$1 $s"
}
call user GET /user /user "$work/admin"
call user-named GET /users/m3-member /users/{username} "$work/admin"
call org-as-user GET /users/m3org /users/{username} "$work/admin"
call user-unknown GET /users/no-such-user-m3 /users/{username} "$work/admin"
call membership GET /orgs/m3org/memberships/m3-member /orgs/{org}/memberships/{username} "$work/admin"
node "$here/ghs.mjs" "$state" > "$work/ghs"
call user-as-installation GET /user /user "$work/ghs"
node "$here/newpat.mjs" "$state" member > "$work/pat"
s=$(node "$here/revoke.mjs" "$work/pat" 178c6fc778ccc68e1d6a)
jq -n --argjson s "$s" '{method: "DELETE", path: "/applications/{client_id}/token", status: $s, body: null}' > "$out/identity/app-token-delete.json"
echo "app-token-delete $s"
