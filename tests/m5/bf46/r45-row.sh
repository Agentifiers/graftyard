#!/bin/bash
# BF-46's R45 rows on graft-m4a (`build/BUILD-PLAN.md`, M5-WP6; `tests/m5/results/m5-wp6.md`, "BF-46 hosted, R45's
# rows on graft-m4a"; the lead's order, 2026-11-24): `Rotate` of McpClientSecret, SecretsWrapKey, McpAppKey and
# InsightsAppKey, executed by a test-faults `graft-m4a-ops` that binds both slots of each, with live traffic where a
# reader exists:
#   - McpClientSecret: `graft-mcp` (both slots bound, its own `Release` view, on its workers.dev origin, so nothing in
#     the graftyard.dev zone changes) redeems a host code with the slot's secret on every OAuth login; the App's record
#     lists both hashes before the switch, then only the new one (the old hash taken off), and a login after that still
#     succeeds;
#   - SecretsWrapKey: hook creates (`POST /repos/{o}/{r}/hooks`, which `graft-ops` `/wrap`s) and the pings they sign
#     (`HookSequencer` `/unwrap`s), at a receiver that checks `X-Hub-Signature-256`. With `wrap-slot-var` the active
#     slot before any `Rotate` is `SECRETS_WRAP_KEY_CURRENT`'s, 1, so the row runs `Rotate` to 0 and back to 1; slot 0 is
#     a fresh row secret;
#   - McpAppKey and InsightsAppKey: no reader (R45's ruling), so the probe and the switch only.
# Pass, per secret: no new script version across its rotation; zero failed calls; the old value unused after the
# switch. One step per call, each logged to tests/m5/results/raw/bf46r45/<tag>/<step>.log through `scrub`.
#
#   bash tests/m5/bf46/r45-row.sh <tag> <step> [args]
#
# Needs `source tests/am1/lf13/env-m4a.sh`, CLOUDFLARE_*, CF_WORKERS_SUBDOMAIN, ~/.graft-m4a with apps/apps.json and
# wp3/app-ids (AM2-WP3's), G (the graftyard binary) and RELVIEW. Every new secret value is drawn here into a 0600 file,
# sent only as a request body, and shredded at teardown; no step prints one. Only hashes and public halves are logged.
set -uo pipefail
cd "$(dirname "$0")/../../.."
TAG=${1:?tag}; STEP=${2:?step}
O=tests/m5/results/raw/bf46r45/$TAG; mkdir -p "$O"
W=$HOME/.graft-m4a; K=$W/r45; mkdir -p "$K"; chmod 700 "$K"
STORE=${CF_SECRETS_STORE_ID:?}
OPS=graft-m4a-ops; OPS_PARKED=${OPS_PARKED:-ac4cd9a5}
EDGE_NAME=graft-m4a-edge MCP_NAME=graft-m4a-mcp AUX_NAME=graft-m4a-mcp-aux SINK=graft-m4a-r45-sink J4SINK=graft-m4-hooksink
MCPO=https://$MCP_NAME.$CF_WORKERS_SUBDOMAIN.workers.dev
B="https://api.cloudflare.com/client/v4/accounts/$CLOUDFLARE_ACCOUNT_ID"; VAPI=$B
api() { curl -s -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" "$1" "${@:2}"; }
scrub() {
  sed -e "s/$CLOUDFLARE_ACCOUNT_ID/<install-account>/g" -e "s/$CF_SECRETS_STORE_ID/<store>/g" -e "s/$CF_WORKERS_SUBDOMAIN/<sub>/g" \
      -e "s#$PWD/#<checkout>/#g" -e "s#$HOME/#~/#g"
}
N=$(ls "$O" | grep -c "^$STEP" || true)
L="$O/$STEP$( [ "$N" -gt 0 ] && echo "-$((N + 1))" ).log"
log() { { date -u +%FT%TZ; git rev-parse --short HEAD; cat; } | scrub >"$L"; cat "$L"; }
secret_id() { api "$B/secrets_store/stores/$STORE/secrets?per_page=100" | jq -r --arg n "$1" '.result[] | select(.name == $n) | .id'; }
# put_secret <name> <file holding {"value": ...}>: create it, or PATCH it when it exists.
put_secret() {
  local id; id=$(secret_id "$1")
  if [ -n "$id" ]; then
    api "$B/secrets_store/stores/$STORE/secrets/$id" -X PATCH -H 'content-type: application/json' --data-binary @"$2" |
      jq -c --arg n "$1" '{patch: $n, ok: .success, errors: [.errors[]?.code]}'
  else
    jq -c --arg n "$1" '[{name: $n, value: .value, comment: "BF-46 R45 row", scopes: ["workers"]}]' "$2" |
      api "$B/secrets_store/stores/$STORE/secrets" -X POST -H 'content-type: application/json' --data-binary @- |
      jq -c --arg n "$1" '{create: $n, ok: .success, errors: [.errors[]?.code]}'
  fi
}
reset_cron() {
  api "$B/workers/scripts/$1/schedules" -X PUT -H 'content-type: application/json' --data '[{"cron":"0 0 1 1 *"}]' >/dev/null
  sleep 2
  api "$B/workers/scripts/$1/schedules" -X PUT -H 'content-type: application/json' --data '[]' | jq -c --arg n "$1" '{($n): .result.schedules}'
}
deployment() { api "$B/workers/scripts/$1/deployments" | jq -c --arg n "$1" '.result.deployments[0] | {script: $n, created_on, versions: [.versions[]? | "\(.version_id[0:8]) \(.percentage)"]}'; }
nversions() { api "$B/workers/scripts/$1/versions?per_page=100" | jq -r '.result.items | length'; }
cost() { PREFIX=graft-m4a python3 tests/fanout/acctcost.py "${1:-$(date -u +%Y-%m-%dT00:00:00Z)}" | head -1 | jq -c '{prefix_usd}'; }
read -r _ MCP_APP _ INS_APP <"$W/wp3/app-ids"
SCRIPTS="graft-m4a-site-a graft-m4a-site-b graft-m4a-site-c $EDGE_NAME $OPS $MCP_NAME"

# The App record commands: `apps init --app-ids` from apps.json (AM2-WP3's public keys and secret hash), with the
# record's keys and client secrets replaced as the row needs. Prints the PutApp command for <slug> on stdout.
#   putapp <slug> <keys json> <client secrets json>
putapp() {
  local d=$K/apps; rm -rf "$d"; mkdir -p "$d"; cp "$W/apps/apps.json" "$d/"
  "$G" apps init --app-ids "$MCP_APP,$INS_APP" --out "$d" >/dev/null
    jq -c --arg s "$1" --argjson k "$2" --argjson c "$3" \
    '.[] | select(.op == "PutApp (\($s))") | (if $k == null then . else .cmd.op.PutApp.record.keys = $k | .cmd.op.PutApp.record.client_secrets = $c end) | {chain, cmd}' \
    "$d/apps-register.request.json"
}
# The record's bytes `sha256` of a client secret file, as `[n, ...]`.
hash_of() { python3 -c 'import hashlib,json,sys; print(json.dumps(list(hashlib.sha256(json.load(open(sys.argv[1]))["value"].encode()).digest())))' "$1"; }
# One AppKey from an RSA private key file (PKCS#8 PEM in {"value"}): `{id, public: [DER SPKI bytes], created}`.
appkey() {
  python3 - "$1" "$2" <<'PY'
import json, subprocess, sys, time
pem = json.load(open(sys.argv[1]))["value"].encode()
der = subprocess.run(["openssl", "pkey", "-pubout", "-outform", "DER"], input=pem, capture_output=True, check=True).stdout
print(json.dumps({"id": int(sys.argv[2]), "public": list(der), "created": int(time.time() * 1000)}))
PY
}
# draw_key <file>: a fresh RSA 2048 private key as {"value": PEM}, 0600.
draw_key() { ( umask 077; openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 2>/dev/null | jq -Rsc '{value: .}' >"$1" ); }
draw_hex() { ( umask 077; python3 -c 'import os; print(os.urandom(32).hex())' | jq -Rc '{value: .}' >"$1" ); }
commit() { node tests/m5/bf46/r45.mjs commit "$1"; }
record() { node tests/m5/bf46/r45.mjs record "$1"; }

case $STEP in
state)
  # The orb's clock ran 28 s behind Cloudflare in r1 (raw/bf46r45/r1/clock.txt): each state step records the offset
  # from Cloudflare's Date header, and the results judge phases on Cloudflare time.
  { t=$(date +%s); d=$(curl -sI https://api.cloudflare.com/client/v4/ | tr -d '\r' | sed -n 's/^[Dd]ate: //p')
    echo "orb minus Cloudflare: $(( t - $(date -d "$d" +%s) )) s (Date header, 1 s resolution)"
    node tests/m5/bf46/view.mjs slots
    for n in $SCRIPTS; do deployment "$n"; echo "  versions $(nversions "$n")"; done
    api "$B/workers/scripts/$OPS/schedules" | jq -c '{ops_schedules: [.result.schedules[]?.cron]}'
    api "$B/secrets_store/stores/$STORE/secrets?per_page=100" | jq -c '{store: [.result[] | select(.name | startswith("graft-m4a")) | .name]}'
    record "$MCP_APP"; record "$INS_APP"
    echo "install day: $(cost)"
  } 2>&1 | log ;;
secrets)
  # Every slot is drawn here: AM2-WP3's App keys and client secret were deleted at its teardown, so the Apps' records
  # are rewritten to list the row's values (`apps`). SecretsWrapKey's slot 0 is a fresh `WrapKeySlot` naming 0 under a
  # row name; slot 1 stays graft-m4a's own `graft-m4a-secrets-wrap-key-b`, which the parked graft-ops binds.
  { for f in mcp-cs-a mcp-cs-b mcp-seal wrap-a; do [ -s "$K/$f.value" ] || draw_hex "$K/$f.value"; done
    for f in mcp-key-a mcp-key-b ins-key-a ins-key-b; do [ -s "$K/$f.value" ] || draw_key "$K/$f.value"; done
    ( umask 077; python3 -c 'import json,sys; v=json.load(open(sys.argv[1]))["value"]; print(json.dumps({"value": json.dumps({"slot": 0, "key": v})}))' \
      "$K/wrap-a.value" >"$K/wrap-a.slot" )
    put_secret graft-m4a-MCP_CLIENT_SECRET_A "$K/mcp-cs-a.value"
    put_secret graft-m4a-MCP_CLIENT_SECRET_B "$K/mcp-cs-b.value"
    put_secret graft-m4a-MCP_SEAL_KEY_A "$K/mcp-seal.value"
    put_secret graft-m4a-r45-MCP_APP_KEY_A "$K/mcp-key-a.value"
    put_secret graft-m4a-r45-MCP_APP_KEY_B "$K/mcp-key-b.value"
    put_secret graft-m4a-r45-INSIGHTS_APP_KEY_A "$K/ins-key-a.value"
    put_secret graft-m4a-r45-INSIGHTS_APP_KEY_B "$K/ins-key-b.value"
    put_secret graft-m4a-r45-secrets-wrap-key-a "$K/wrap-a.slot"
    for f in mcp-cs-a mcp-cs-b; do echo "$f sha256 $(hash_of "$K/$f.value" | python3 -c 'import json,sys; print(bytes(json.load(sys.stdin)).hex()[:16])')..."; done
  } 2>&1 | log ;;
apps)
  # ARG: a (records list slot A only), ab (both slots: before each switch), b (slot B only: the old value taken off),
  # orig (AM2-WP3's records from apps.json, at teardown).
  WHICH=${3:?a|ab|b|orig}
  if [ "$WHICH" = orig ]; then
    { putapp graftyard-mcp null null >"$K/put-mcp.json"; putapp graftyard-insights null null >"$K/put-ins.json"
      commit "$K/put-mcp.json"; commit "$K/put-ins.json"; record "$MCP_APP"; record "$INS_APP"
    } 2>&1 | log; exit
  fi
  { cs() { case $1 in a) echo "[$(hash_of "$K/mcp-cs-a.value")]";; b) echo "[$(hash_of "$K/mcp-cs-b.value")]";;
             ab) echo "[$(hash_of "$K/mcp-cs-a.value"),$(hash_of "$K/mcp-cs-b.value")]";; esac; }
    ks() { case $2 in a) echo "[$(appkey "$K/$1-a.value" 1)]";; b) echo "[$(appkey "$K/$1-b.value" 2)]";;
             ab) echo "[$(appkey "$K/$1-a.value" 1),$(appkey "$K/$1-b.value" 2)]";; esac; }
    putapp graftyard-mcp "$(ks mcp-key "$WHICH")" "$(cs "$WHICH")" >"$K/put-mcp.json"
    putapp graftyard-insights "$(ks ins-key "$WHICH")" '[]' >"$K/put-ins.json"
    commit "$K/put-mcp.json"; commit "$K/put-ins.json"
    record "$MCP_APP"; record "$INS_APP"
  } 2>&1 | log ;;
token)
  { ( umask 077
      jq -nc --arg a "com.cloudflare.api.account.$CLOUDFLARE_ACCOUNT_ID" \
        '{name: "graft-m4a-r45-deploy", policies: [{effect: "allow", resources: {($a): "*"},
          permission_groups: [{id: "e086da7e2179491d91ee5f35b3ca210a"}, {id: "bdbcd690c763475a985e8641dddc09f7"}]}]}' |
        api "$B/tokens" -X POST -H 'content-type: application/json' --data-binary @- >"$K/deploy.json"
      jq -c '{value: .result.value}' "$K/deploy.json" >"$K/deploy.value" )
    jq -c '{mint: "graft-m4a-r45-deploy", ok: .success, id: (.result.id // "" | .[0:8]), errors: [.errors[]?.code]}' "$K/deploy.json"
    put_secret graft-m4a-ops-deploy-token "$K/deploy.value"
  } 2>&1 | log ;;
ops)
  # graft-ops from this checkout, test-faults, both slots of each R45 secret bound, 1-minute cron. SecretsWrapKey binds
  # the row's slot 0 and graft-m4a's slot 1 (the parked version's only one), with SECRETS_WRAP_KEY_CURRENT 1.
  { jq '.slots = {"MCP_CLIENT_SECRET": ["graft-m4a-MCP_CLIENT_SECRET_A", "graft-m4a-MCP_CLIENT_SECRET_B"],
                  "MCP_APP_KEY": ["graft-m4a-r45-MCP_APP_KEY_A", "graft-m4a-r45-MCP_APP_KEY_B"],
                  "INSIGHTS_APP_KEY": ["graft-m4a-r45-INSIGHTS_APP_KEY_A", "graft-m4a-r45-INSIGHTS_APP_KEY_B"]}
        | .cron = "* * * * *" | .gate = "pending"' "$W/ops/exec-ck-pending.json" >"$W/ops/exec-r45.json"
    jq -c '{test_faults, gate, cron, step_dwell_ms, slots}' "$W/ops/exec-r45.json"
    date -u +%s%3N >"$O/start.ms"
    (export PATH=$HOME/.cargo/bin:$PATH && unset CARGO_TARGET_DIR && \
      node workers/ops/deploy.mjs $OPS $STORE graft-m4a-mint-wrap-key graft-m4a-r45-secrets-wrap-key-a graft-m4a-secrets-wrap-key-b 1 \
        --executor "$W/ops/exec-r45.json" 2>&1 | grep -E "Current Version|rror" | tail -3)
    deployment $OPS
    api "$B/workers/scripts/$OPS/schedules" | jq -c '{schedules: [.result.schedules[]?.cron]}'
  } 2>&1 | log ;;
cron)
  # R52: a deploy's Cron Trigger may not fire; a re-PUT does.
  { reset_cron $OPS >/dev/null
    api "$B/workers/scripts/$OPS/schedules" -X PUT -H 'content-type: application/json' --data '[{"cron":"* * * * *"}]' | jq -c '{schedules: .result.schedules}'
  } 2>&1 | log ;;
mcp)
  # graft-mcp with both client-secret slots and its own `Release` view, on its workers.dev origin (apps.json's copy with
  # `mcp_origin` there; the test edge's stand-in authorize does not check the record's callback), and the CIMD host
  # the SDK clients name. No graft-front: nothing in the graftyard.dev zone changes.
  { jq --arg o "$MCPO" '.mcp_origin = $o' "$W/apps/apps.json" >"$K/apps-wd.json"
    sed "s/graft-m3f-mcp-aux/$AUX_NAME/" tests/am2/hosted/aux.jsonc >tests/am2/hosted/.aux-r45.jsonc
    npx --prefix tests/m0 wrangler deploy -c tests/am2/hosted/.aux-r45.jsonc 2>&1 | grep -E "Current Version|rror"
    rm -f tests/am2/hosted/.aux-r45.jsonc
    (export PATH=$HOME/.cargo/bin:$PATH && unset CARGO_TARGET_DIR && \
      node workers/mcp/deploy.mjs $MCP_NAME "$K/apps-wd.json" --edge $EDGE_NAME --store $STORE --app-id "$MCP_APP" \
        --client-secret-b --sites "$W/sites.json" --anchor "$W/install.toml" 2>&1 | grep -E "Current Version|rror")
    deployment $MCP_NAME
    curl -s -o /dev/null -w 'graft-mcp metadata %{http_code}\n' "$MCPO/.well-known/oauth-protected-resource"
  } 2>&1 | log ;;
sink)
  # The hook receiver (tests/m5/bf46/r45-sink): it checks `X-Hub-Signature-256` against the secret the row gives each
  # hook (HOOK_SECRET, from $K/sink.secret).
  { ( umask 077; [ -s "$K/sink.secret" ] || python3 -c 'import os; print(os.urandom(24).hex())' >"$K/sink.secret" )
    (cd tests/m5/bf46/r45-sink && npx --prefix ../../../m0 wrangler deploy --name $SINK 2>&1 | grep -E "Current Version|rror"
     npx --prefix ../../../m0 wrangler secret put HOOK_SECRET --name $SINK <"$K/sink.secret" 2>&1 | tail -1
     # j4's existing hooks (slot 1 before the row) deliver to BF-57's receiver name; the same source checks BF-57's
     # per-hook secrets there (BF57_J4).
     npx --prefix ../../../m0 wrangler deploy --name $J4SINK --var BF57_J4:1 2>&1 | grep -E "Current Version|rror"
     npx --prefix ../../../m0 wrangler secret put HOOK_SECRET --name $J4SINK <"$K/sink.secret" 2>&1 | tail -1)
  } 2>&1 | log ;;
load)
  case ${3:?start|stop} in
    start) rm -f "$O/stop"
      amp orb service start bf46-r45-load --command "cd $PWD && source tests/am1/lf13/env-m4a.sh && GRAFT_MCP=$MCPO GRAFT_AUX=https://$AUX_NAME.$CF_WORKERS_SUBDOMAIN.workers.dev R45_SINK=https://$SINK.$CF_WORKERS_SUBDOMAIN.workers.dev R45_J4_SINK=https://$J4SINK.$CF_WORKERS_SUBDOMAIN.workers.dev R45_SINK_SECRET_FILE=$K/sink.secret R45_CS_A=$K/mcp-cs-a.value R45_CS_B=$K/mcp-cs-b.value R45_CLIENT_ID=Iv1.$(printf %016x "$MCP_APP") exec node tests/m5/bf46/r45.mjs load $PWD/$O/load.jsonl $PWD/$O/stop" 2>&1 | head -1 | log ;;
    stop) touch "$O/stop"; sleep 25; amp orb service stop bf46-r45-load 2>&1 | tail -1 | log ;;
  esac ;;
wrapcheck)
  { node tests/m5/bf46/r45.mjs wrapcheck m3org/j4,m3org/r45-hooks; } 2>&1 | log ;;
slots)
  # Each hook left on the row's repository and its SecretsWrapKey slot (none while the load deletes as it goes).
  { node tests/m5/bf46/r45.mjs slots "${3:?repo chain}"; } 2>&1 | log ;;
counts)
  { node tests/m5/bf46/r45.mjs counts "$O/load.jsonl" "${3:-0}"; } 2>&1 | log ;;
rotate)
  SEC=${3:?secret}; NS=${4:?slot}
  { a=$(node tests/m5/bf46/view.mjs prev); echo "prev $(echo "$a" | jq -r .prev | cut -c1-12) count $(echo "$a" | jq -r .count)"
    echo "{\"Intent\": {\"ttl_s\": 21600, \"intent\": {\"Rotate\": {\"secret\": \"$SEC\", \"slot\": $NS}}}}" >"$K/rot.json"
    "$G" --config "$W/install.toml" release entry "$K/rot.json" --out "$K/rot.unsigned" --prev "$(echo "$a" | jq -r .prev)" >/dev/null
    "$G" sign "$K/rot.unsigned" --out "$K/rot.signed" --key "$W/customer.key" >/dev/null
    "$G" --config "$W/install.toml" release post "$K/rot.signed" --edge "$GRAFT_EDGE"
    echo "Rotate $SEC to slot $NS posted at $(date -u +%s%3N)"
  } 2>&1 | log ;;
view)
  { node tests/m5/bf46/view.mjs entries "$(cat "$O/start.ms")"; } 2>&1 | log ;;
versions)
  { for n in $SCRIPTS; do echo "$(deployment "$n") versions $(nversions "$n")"; done; } 2>&1 | log ;;
logs)
  { KEEP="Rotate,IntentStep,IntentDone,PAGE,Committed" python3 tests/m5/bf46/logs.py $OPS "${3:-$(cat "$O/start.ms")}" "${4:-$(date +%s%3N)}" | tail -${5:-40}
  } 2>&1 | log ;;
teardown)
  { touch "$O/stop"; amp orb service stop bf46-r45-load >/dev/null 2>&1
    R45_SINK=https://$SINK.$CF_WORKERS_SUBDOMAIN.workers.dev node tests/m5/bf46/r45.mjs unhook m3org/r45-hooks
    # The lead's condition: no hook may stay on slot 0, the row's secret deleted below; stop here if one does.
    node tests/m5/bf46/r45.mjs wrapcheck m3org/j4,m3org/r45-hooks || { echo "teardown stopped before parking graft-ops"; exit 1; }
    reset_cron $OPS
    echo "graft-ops parked as $OPS_PARKED"
    # `versions deploy` takes the full version id and `@100%` (m3fs2's teardown: a short id or `@100` exits 1).
    full=$(api "$VAPI/workers/scripts/$OPS/versions?per_page=100" | jq -r --arg p "$OPS_PARKED" '.result.items[] | select(.id | startswith($p)) | .id')
    npx --prefix tests/m0 wrangler versions deploy "$full@100%" --name $OPS -y 2>&1 | grep -E "Deployed|rror" | tail -2
    reset_cron $OPS
    deployment $OPS
    for n in $MCP_NAME $AUX_NAME $SINK $J4SINK; do npx --prefix tests/m0 wrangler delete --name $n --force 2>&1 | tail -1; done
    for n in graft-m4a-MCP_CLIENT_SECRET_A graft-m4a-MCP_CLIENT_SECRET_B graft-m4a-MCP_SEAL_KEY_A graft-m4a-r45-MCP_APP_KEY_A \
             graft-m4a-r45-MCP_APP_KEY_B graft-m4a-r45-INSIGHTS_APP_KEY_A graft-m4a-r45-INSIGHTS_APP_KEY_B graft-m4a-r45-secrets-wrap-key-a; do
      id=$(secret_id "$n"); [ -n "$id" ] && api "$B/secrets_store/stores/$STORE/secrets/$id" -X DELETE | jq -c --arg n "$n" '{delete: $n, ok: .success}'
    done
    id=$(api "$B/tokens?per_page=50" | jq -r '.result[] | select(.name == "graft-m4a-r45-deploy") | .id')
    [ -n "$id" ] && api "$B/tokens/$id" -X DELETE | jq -c '{revoke: "graft-m4a-r45-deploy", ok: .success}'
    for f in "$K"/*.value "$K"/*.slot "$K"/*.json "$K"/sink.secret; do [ -f "$f" ] && shred -u "$f"; done
    rm -rf "$K/apps" "$K/apps-wd.json"; echo "row files shredded"
    node tests/m5/bf46/view.mjs slots
  } 2>&1 | log ;;
*) echo "step: state secrets apps token ops cron mcp sink load wrapcheck slots counts rotate view versions logs teardown"; exit 2 ;;
esac
