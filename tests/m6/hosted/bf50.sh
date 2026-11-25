#!/usr/bin/env bash
# BF-50 on graft-m6a (`build/BUILD-PLAN.md`, M6-WP1; `tests/m6/results/m6-wp1.md`, "BF-50"). Helpers sourced by the
# row driver and run by hand: the Release view (relview over the bench's `/view` pages), customer-signed entries,
# the bench Worker's routes, and the install's spend. Secrets stay in ~/.graft-m6a (0600), read only from files.
#   source tests/m6/hosted/bf50.sh; view 0 | jq .count
set -uo pipefail
ROOT=${ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)}
WRANGLER=${WRANGLER:-$ROOT/tests/m0/node_modules/.bin/wrangler}
W=${W:-$HOME/.graft-m6a}
G=${G:-$ROOT/target/release/graftyard}
RELVIEW=${RELVIEW:-$ROOT/tests/m3/hosted/tools/target/release/relview}
EDGE=${EDGE:-https://graft-m6a-edge.${CF_WORKERS_SUBDOMAIN:?}.workers.dev}
BENCH=${BENCH:-https://graft-m6a-bf50.${CF_WORKERS_SUBDOMAIN:?}.workers.dev}
K=$W/bf50; mkdir -p "$K" && chmod 700 "$K"
CFAPI="https://api.cloudflare.com/client/v4/accounts/${CLOUDFLARE_ACCOUNT_ID:?}"
api() { curl -s -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" "$CFAPI$1" "${@:2}"; }
scrub() { sed -e "s/$CLOUDFLARE_ACCOUNT_ID/<install-account>/g" -e "s/$CF_WORKERS_SUBDOMAIN/<install-subdomain>/g" \
  -e "s/$CF_ZONE_ID/<zone-id>/g" -e "s#$HOME/#~/#g"; }
F=$(grep -E '^(install|customer_key) ' "$W/install.toml" | sed -E 's/.*= *"([^"]*)"/\1/')
# bench <path> [json body]: one bench call.
bench() {
  if [ $# -ge 2 ]; then curl -s -X POST -H "authorization: Bearer $(cat "$W/bf50.token")" "$BENCH$1" --data-binary "$2"
  else curl -s -H "authorization: Bearer $(cat "$W/bf50.token")" "$BENCH$1"; fi
}
# pages [skip site]: the longest site's `/view` pages into $K/pages.json.
pages() {
  bench /view "{\"sites\":[0,1,2,3],\"skip\":${1:-null}}" | jq -c '[.[] | select(.error == null)] | max_by([.pages[].entries[]] | length) | .pages' >"$K/pages.json"
}
# view [from]: relview's record of the group from position <from>.
view() { pages && "$RELVIEW" $F "${1:-0}" <"$K/pages.json"; }
# customer <spec json> <label>: one entry bound to the group's last digest, signed by the customer key, posted.
customer() {
  local v; v=$(view 0)
  echo "$v" | jq -r .keys_entry | python3 -c "import sys; sys.stdout.buffer.write(bytes.fromhex(sys.stdin.read().strip()))" >"$K/keys.entry"
  echo "$1" >"$K/spec.json"
  "$G" --config "$W/install.toml" release entry "$K/spec.json" --out "$K/$2.unsigned" \
    --prev "$(echo "$v" | jq -r .prev)" --keys "$K/keys.entry" >/dev/null &&
    "$G" sign "$K/$2.unsigned" --out "$K/$2.signed" --key "$W/customer.key" >/dev/null &&
    echo "$2: $("$G" --config "$W/install.toml" release post "$K/$2.signed" --edge "$EDGE" 2>&1 | tr '\n' ' ')"
}
# spend [from ISO]: the install's spend since then (default: the install's start).
spend() { INSTALL=graft-m6a python3 "$ROOT/tests/fanout/m2bcost.py" "${1:-$(date -u -d @"$(cat "$W/t0")" +%FT%TZ)}" "$(date -u +%FT%TZ)" 2>/dev/null | tail -n 1; }
# versions: each graft-m6a script's version ids, newest first.
versions() {
  for s in $(api "/workers/scripts" | jq -r '.result[].id | select(startswith("graft-m6a"))'); do
    api "/workers/scripts/$s/versions?per_page=50" | jq -c --arg s "$s" '{script: $s, versions: [.result.items[]?.id[0:8]]}'
  done
}
# graft-ops' setup, as tests/m5/bf46/row.sh and bf47/row.sh do it. STORE is the account's Secrets Store.
STORE=${STORE:-bfaa0c1573de4775b1b92a12fb89acb4}
OPS=${OPS:-graft-m6a-ops}
SCRIPTS_WRITE=e086da7e2179491d91ee5f35b3ca210a
CONTAINERS_WRITE=bdbcd690c763475a985e8641dddc09f7
# mint <name> <permission group id>...: an account token into $K/<name>.json (0600).
mint() {
  local name=$1; shift
  local groups; groups=$(printf '{"id":"%s"},' "$@"); groups="[${groups%,}]"
  ( umask 077
    jq -nc --arg n "$name" --arg a "com.cloudflare.api.account.$CLOUDFLARE_ACCOUNT_ID" --argjson g "$groups" \
      '{name: $n, policies: [{effect: "allow", resources: {($a): "*"}, permission_groups: $g}]}' |
      api /tokens -X POST -H 'content-type: application/json' --data-binary @- >"$K/$name.json" )
  jq -c --arg n "$name" '{mint: $n, ok: .success, id: (.result.id // "" | .[0:8]), errors: [.errors[]?.code]}' "$K/$name.json"
}
# revoke <name>: delete the account token of that name.
revoke() {
  local id; id=$(api "/tokens?per_page=50" | jq -r --arg n "$1" '.result[] | select(.name == $n) | .id')
  [ -n "$id" ] && api "/tokens/$id" -X DELETE | jq -c --arg n "$1" '{revoke: $n, ok: .success, errors: [.errors[]?.code]}' \
    || echo "{\"revoke\": \"$1\", \"found\": false}"
}
secret_id() { api "/secrets_store/stores/$STORE/secrets?per_page=100" | jq -r --arg n "$1" '.result[] | select(.name == $n) | .id'; }
# put_secret <name> <file holding {"value": ...}>: create the store secret, or PATCH it when it exists.
put_secret() {
  local id; id=$(secret_id "$1")
  if [ -n "$id" ]; then
    api "/secrets_store/stores/$STORE/secrets/$id" -X PATCH -H 'content-type: application/json' --data-binary @"$2" |
      jq -c --arg n "$1" '{patch: $n, ok: .success, errors: [.errors[]?.code]}'
  else
    jq -c --arg n "$1" '[{name: $n, value: .value, scopes: ["workers"]}]' "$2" |
      api "/secrets_store/stores/$STORE/secrets" -X POST -H 'content-type: application/json' --data-binary @- |
      jq -c --arg n "$1" '{create: $n, ok: .success, errors: [.errors[]?.code]}'
  fi
}
cronoff() {
  api "/workers/scripts/$OPS/schedules" -X PUT -H 'content-type: application/json' --data '[{"cron":"0 0 1 1 *"}]' >/dev/null
  sleep 2
  api "/workers/scripts/$OPS/schedules" -X PUT -H 'content-type: application/json' --data '[]' | jq -c '{ops_schedules: .result.schedules}'
}
