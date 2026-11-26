#!/usr/bin/env bash
# BF-46 hosted on graft-m4a (build/BUILD-PLAN.md, M5-WP6; tests/m5/results/m5-wp6.md, "BF-46 hosted"): `Rotate` of
# `DeployToken`, `ReadToken` and `MintWrapKey` executed by `graft-m4a-ops` with both slots of each bound, while the
# edge mints installation tokens. Pass: no new script version per switch, zero failed calls, the old value unused
# after the switch. The install's real tokens are not rotated: each new slot holds a throwaway token minted here and
# revoked at teardown, and every slot goes back to 0.
#
#   bash tests/m5/bf46/row.sh <tag> <phase>
#
# Phases, in order:
#   setup     mint the throwaway tokens (deploy slots 0 and 1, read slots 0 and 1) and the kid-2 signing key; create
#             the four new store secrets; PATCH slot 0's deploy token; deploy graft-m4a-ops (test-faults, both slots, 1-minute
#             cron, OPS_ROTATE_PUBLISHED_MS); start the minting probe
#   rotate S N   sign and post `Rotate { S, N }` with the customer key (S: DeployToken, ReadToken or MintWrapKey)
#   watch     the group's entries from the start, the ops version, and the probe's counts (repeat)
#   logs      graft-ops' Rotate log lines from Workers Logs since setup
#   teardown  stop the probe; cron off; restore 4bbfd9e5's configuration (exec-ck-pending.json, no slots); revoke the
#             three tokens; delete the four new secrets; PATCH the slot-0 secret back to a revoked value
#   lists     tokens, secrets, schedules and the ops deployment, to show every one is gone
#
# Needs: CLOUDFLARE_ACCOUNT_ID and CLOUDFLARE_API_TOKEN (the install account), the graft-m4a env
# (tests/am1/lf13/env-m4a.sh), ~/.graft-m4a, G (the graftyard binary), and MINTKEY and RELVIEW (tests/m3/hosted/tools'
# `cargo build --release --bin mintkey --bin relview`), and NODE_MODULES, a tests/m0/node_modules with wrangler. PUBLISHED_MS sets MintWrapKey's publish wait (default 10 min). Every log goes through scrub; token values are never printed, read only from
# 0600 files, and sent only as request bodies or headers.
set -uo pipefail
cd "$(dirname "$0")/../../.."
TAG=${1:?tag}; PH=${2:?phase}
O=tests/m5/results/raw/bf46/$TAG; mkdir -p "$O"
W=$HOME/.graft-m4a; K=$W/bf46; mkdir -p "$K"; chmod 700 "$K"
STORE=${CF_SECRETS_STORE_ID:?}
OPS=graft-m4a-ops
G=${G:?the graftyard binary}
MINTKEY=${MINTKEY:?tests/m3/hosted/tools mintkey}
export RELVIEW=${RELVIEW:?tests/m3/hosted/tools relview}
B="https://api.cloudflare.com/client/v4/accounts/$CLOUDFLARE_ACCOUNT_ID"
api() { curl -s -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" "$B$1" "${@:2}"; }
scrub() {
  sed -e "s/$CLOUDFLARE_ACCOUNT_ID/<install-account>/g" -e "s/$CF_SECRETS_STORE_ID/<store>/g" -e "s/$CF_WORKERS_SUBDOMAIN/<install-subdomain>/g" \
      -e "s/$CF_ZONE_ID/<zone-id>/g" -e "s#$PWD/#<checkout>/#g" -e "s#$HOME/#~/#g"
}
N=$(ls "$O" | grep -c "^$PH" || true)
L="$O/$PH$( [ "$N" -gt 0 ] && echo "-$((N + 1))" ).log"
log() { { date -u +%FT%TZ; cat; } | scrub >"$L"; cat "$L"; }

# mint <name> <permission group id>...: an account token scoped to this account, written to $K/<name>.json (0600).
mint() {
  local name=$1; shift
  # A rerun after a cut-off setup keeps the token it already minted (m4abf46's first setup died after the mints).
  if jq -e .success "$K/$name.json" >/dev/null 2>&1; then
    jq -c --arg n "$name" '{mint: $n, kept: true, id: (.result.id // "" | .[0:8])}' "$K/$name.json"; return
  fi
  local groups; groups=$(printf '{"id":"%s"},' "$@"); groups="[${groups%,}]"
  ( umask 077
    jq -nc --arg n "$name" --arg a "com.cloudflare.api.account.$CLOUDFLARE_ACCOUNT_ID" --argjson g "$groups" \
      '{name: $n, policies: [{effect: "allow", resources: {($a): "*"}, permission_groups: $g}]}' |
      api /tokens -X POST -H 'content-type: application/json' --data-binary @- >"$K/$name.json" )
  jq -c --arg n "$name" '{mint: $n, ok: .success, id: (.result.id // "" | .[0:8]), errors: [.errors[]?.code]}' "$K/$name.json"
}
SCRIPTS_WRITE=e086da7e2179491d91ee5f35b3ca210a
CONTAINERS_WRITE=bdbcd690c763475a985e8641dddc09f7
R2_READ=b4992e1108244f5d8bfbd5744320c2e1
# secret_id <name>: the store secret's id, or nothing.
secret_id() { api "/secrets_store/stores/$STORE/secrets?per_page=100" | jq -r --arg n "$1" '.result[] | select(.name == $n) | .id'; }
# put_secret <name> <value file holding {"value": ...}>: create it, or PATCH it when it exists.
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
# value_of <token json>: {"value": <the token>} into a 0600 file, path on stdout.
value_of() { ( umask 077; jq -c '{value: .result.value}' "$K/$1.json" >"$K/$1.value" ); echo "$K/$1.value"; }
cronoff() {
  api "/workers/scripts/$OPS/schedules" -X PUT -H 'content-type: application/json' --data '[{"cron":"0 0 1 1 *"}]' >/dev/null
  sleep 2
  api "/workers/scripts/$OPS/schedules" -X PUT -H 'content-type: application/json' --data '[]' | jq -c '{schedules: .result.schedules}'
}
ops_version() { api "/workers/scripts/$OPS/deployments" | jq -c '.result.deployments[0] | {created_on, versions: [.versions[] | "\(.version_id[0:8]) \(.percentage)"]}'; }

case $PH in
setup)
  { git rev-parse --short HEAD
    ops_version
    # Slot 0 of DeployToken: graft-ops' own token for the row (the secret it already binds). Slot 1: a second token
    # with the same scope. ReadToken slot 1: R2 read only. Each is revoked at teardown.
    mint graft-m4a-bf46-deploy-0 $SCRIPTS_WRITE $CONTAINERS_WRITE
    mint graft-m4a-bf46-deploy-1 $SCRIPTS_WRITE $CONTAINERS_WRITE
    mint graft-m4a-bf46-read-0 $R2_READ
    mint graft-m4a-bf46-read-1 $R2_READ
    put_secret graft-m4a-ops-deploy-token "$(value_of graft-m4a-bf46-deploy-0)"
    put_secret graft-m4a-bf46-deploy-token-1 "$(value_of graft-m4a-bf46-deploy-1)"
    put_secret graft-m4a-bf46-read-token-1 "$(value_of graft-m4a-bf46-read-1)"
    put_secret graft-m4a-bf46-read-token-0 "$(value_of graft-m4a-bf46-read-0)"
    # MintWrapKey slot 1: kid 2, drawn by mintkey; slot 0 stays the install's kid 1 secret.
    if [ ! -s "$K/kid2.value" ]; then
      ( umask 077; "$MINTKEY" 2 "$K/kid2.text" >"$O/kid2-putvk.json"
        jq -Rsc '{value: .}' "$K/kid2.text" >"$K/kid2.value"; shred -u "$K/kid2.text" )
    fi
    jq -c '{kid2_putvk: {kid: .op.PutVerifyingKey.kid, key: .op.PutVerifyingKey.key}}' "$O/kid2-putvk.json" 2>/dev/null || cat "$O/kid2-putvk.json"
    put_secret graft-m4a-bf46-mint-wrap-key-1 "$K/kid2.value"
    jq '.slots = {"DEPLOY_TOKEN": ["graft-m4a-ops-deploy-token", "graft-m4a-bf46-deploy-token-1"],
                  "READ_TOKEN": ["graft-m4a-bf46-read-token-0", "graft-m4a-bf46-read-token-1"],
                  "MINT_WRAP_KEY": ["graft-m4a-mint-wrap-key", "graft-m4a-bf46-mint-wrap-key-1"]}
        | .cron = "* * * * *" | .rotate_published_ms = (env.PUBLISHED_MS // "600000" | tonumber) | del(.gate) | .gate = "pending"' \
      "$W/ops/exec-ck-pending.json" >"$W/ops/exec-bf46.json"
    jq -c '{test_faults, gate, cron, rotate_published_ms, step_dwell_ms, slots}' "$W/ops/exec-bf46.json"
    date -u +%s%3N >"$O/start.ms"
    # graft-ops is built from merged main (REF, default origin/main), not from this checkout's branch.
    git fetch -q origin main
    (cd /tmp && rm -rf bf46-deploy && git -C "$OLDPWD" worktree add -f --detach /tmp/bf46-deploy "${REF:-origin/main}" >/dev/null 2>&1)
    echo "graft-ops built from $(git -C /tmp/bf46-deploy rev-parse --short HEAD)"
    grep -q rotate_published_ms /tmp/bf46-deploy/workers/ops/deploy.mjs \
      && echo "OPS_ROTATE_PUBLISHED_MS: in this build" || echo "OPS_ROTATE_PUBLISHED_MS: not in this build; MintWrapKey waits the full hour"
    (cd /tmp/bf46-deploy && unset CARGO_TARGET_DIR && export PATH=$HOME/.cargo/bin:$PATH \
      && ln -sfn "${NODE_MODULES:?a tests/m0/node_modules with wrangler}" tests/m0/node_modules \
      && node workers/ops/deploy.mjs $OPS $STORE graft-m4a-mint-wrap-key - graft-m4a-secrets-wrap-key-b 1 \
           --executor "$W/ops/exec-bf46.json" 2>&1 | grep -E "Uploaded|Deployed|Current Version|error|Error" | tail -5)
    ops_version
    api "/workers/scripts/$OPS/schedules" | jq -c '{schedules: [.result.schedules[]?.cron]}'
    # The minting probe: one installation token through the edge every 10 s, while the rotations run.
    # A supervised service, so an orb or CLI restart does not end it mid-row.
    amp orb service start bf46-mint --command "cd $PWD && node tests/m5/bf46/mint.mjs $PWD/$O/mint.jsonl" 2>&1 | head -1
  } 2>&1 | log ;;
rotate)
  S=${3:?secret}; NS=${4:?slot}
  { a=$(node tests/m5/bf46/view.mjs prev); echo "prev $(echo "$a" | jq -r .prev | cut -c1-12)"
    echo "{\"Intent\": {\"ttl_s\": 21600, \"intent\": {\"Rotate\": {\"secret\": \"$S\", \"slot\": $NS}}}}" >"$K/rot.json"
    "$G" --config "$W/install.toml" release entry "$K/rot.json" --out "$K/rot.unsigned" --prev "$(echo "$a" | jq -r .prev)" >/dev/null
    "$G" sign "$K/rot.unsigned" --out "$K/rot.signed" --key "$W/customer.key" >/dev/null
    "$G" --config "$W/install.toml" release post "$K/rot.signed" --edge "$GRAFT_EDGE"
    echo "Rotate $S to slot $NS posted"
  } 2>&1 | tee -a "$O/answers.txt" | log ;;
watch)
  { ops_version
    node tests/m5/bf46/view.mjs entries "$(cat "$O/start.ms")"
    python3 - "$O/mint.jsonl" <<'PY'
import collections, json, sys
rows = [json.loads(l) for l in open(sys.argv[1])]
by = collections.Counter((r["status"], r.get("kid")) for r in rows)
print(json.dumps({"mints": len(rows), "by_status_kid": {f"{s} kid {k}": n for (s, k), n in sorted(by.items(), key=str)},
                  "last": rows[-1] if rows else None}))
PY
  } 2>&1 | log ;;
logs)
  python3 tests/m5/bf46/logs.py "$OPS" "$(cat "$O/start.ms")" "$(date -u +%s%3N)" 2>&1 | log ;;
teardown)
  { amp orb service stop bf46-mint 2>&1 | tail -1
    cronoff
    (cd /tmp/bf46-deploy && unset CARGO_TARGET_DIR && export PATH=$HOME/.cargo/bin:$PATH && git checkout -q --detach ad8fd52f \
      && node workers/ops/deploy.mjs $OPS $STORE graft-m4a-mint-wrap-key - graft-m4a-secrets-wrap-key-b 1 \
           --executor "$W/ops/exec-ck-pending.json" 2>&1 | grep -E "Current Version|error|Error" | tail -3)
    cronoff
    ops_version
    for n in graft-m4a-bf46-deploy-0 graft-m4a-bf46-deploy-1 graft-m4a-bf46-read-0 graft-m4a-bf46-read-1; do
      id=$(api "/tokens?per_page=50" | jq -r --arg n "$n" '.result[] | select(.name == $n) | .id')
      [ -n "$id" ] && api "/tokens/$id" -X DELETE | jq -c --arg n "$n" '{revoke: $n, ok: .success, errors: [.errors[]?.code]}'
    done
    for n in graft-m4a-bf46-deploy-token-1 graft-m4a-bf46-read-token-0 graft-m4a-bf46-read-token-1 graft-m4a-bf46-mint-wrap-key-1; do
      id=$(secret_id "$n")
      [ -n "$id" ] && api "/secrets_store/stores/$STORE/secrets/$id" -X DELETE | jq -c --arg n "$n" '{delete: $n, ok: .success, errors: [.errors[]?.code]}'
    done
    for f in "$K"/*.json "$K"/*.value; do [ -f "$f" ] && shred -u "$f"; done
    git worktree remove --force /tmp/bf46-deploy 2>/dev/null; echo "files shredded, deploy worktree removed"
  } 2>&1 | log ;;
lists)
  { api "/tokens?per_page=50" | jq -c '{bf46_tokens: [.result[] | select(.name | startswith("graft-m4a-bf46")) | .name]}'
    api "/secrets_store/stores/$STORE/secrets?per_page=100" | jq -c '{bf46_secrets: [.result[] | select(.name | startswith("graft-m4a-bf46")) | .name], store_count: (.result | length)}'
    api "/workers/scripts/$OPS/schedules" | jq -c '{schedules: [.result.schedules[]?.cron]}'
    ops_version
    node tests/m5/bf46/view.mjs slots
  } 2>&1 | log ;;
*) echo "phase: setup rotate watch logs teardown lists"; exit 2 ;;
esac
