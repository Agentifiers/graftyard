#!/bin/bash
# AM2-WP3's hosted day on graft-m4a (`tests/am2/results/am2-wp3.md`, "Hosted"): `graft-front` in front of the edge and
# `graft-mcp` on two Workers Custom Domains under graftyard.dev, a `FrontRoll` and an `McpRoll` executed by
# `graft-m4a-ops`, and the pre-signed `McpPointer` committed on a gate breach with the deploy token invalid. One step
# per call, each logged to tests/am2/raw/wp3-<tag>-<step>.log, which is not run again once it exists.
#
#   bash tests/am2/hosted/wp3-day.sh <tag> <step> [args]
#
# Needs `source tests/am1/lf13/env-m4a.sh`, CLOUDFLARE_*, CF_WORKERS_SUBDOMAIN, ~/.graft-m4a (with ops/ from M4-WP4),
# G (the graftyard binary), RELVIEW (tests/m3/hosted/tools relview), and the ops executor file OPS_PARKED (M4-WP4's
# exec-ck-pending.json) with its build head OPS_REF. Token values are read only from 0600 files and sent only in
# request bodies or headers; every log line goes through `scrub`.
#
# Steps, in order:
#   state       relview's site_release, rolled, uploads, current and drained; the deployments; the edge's formats
#   domains     the zone's custom domains and the account's Workers domains, listed (before and after)
#   token       mint graft-m4a-wp3-deploy (Workers Scripts Write, Containers Write) and PATCH it into the ops secret
#   edge        edge day: the live FORMATS_ENABLED plus 20.4 and 17.4, the live EXPORT kept (`off` as $3: the saved
#               list back, without them)
#   apps        apps init (graft-m4a- secrets), claims, register; then graft-mcp N deployed (VERSION binding)
#   front       graft-front with --mcp, --mcp-host and both --domain names; FRONT_TOKEN
#   bundles     front/mcp builds, then w1 (N), w2 (N+1, marker), w3 (next, marker) into graft-m4a-releases, each
#               with FMT_FROM's formats.txt (the last rolled release's prefix)
#   ops <file> [label]  graft-m4a-ops from OPS_REF with <exec file>
#   roll <R>    sign and post the next intent for release R (`release roll --entries`), with its pre-signed undos
#   view        the group's entries since the start, the current intent, and drained sites
#   block|unblock  DEPLOY_TOKEN set to an invalid value, or back to the row token
#   session <label>  one MCP client session through mcp.<host>: initialize, tools/list, one tool call (TS v2)
#   sampler start|stop  wp2-sample.mjs through mcp.<host> into tests/am2/raw/wp3-<tag>/mcp.jsonl
#   teardown    cron off, graft-ops parked (OPS_PARKED), graft-front, graft-mcp and aux deleted (with the domains),
#               the four apps secrets deleted, the row token revoked, edge back without 20.4 and 17.4
set -uo pipefail
cd "$(dirname "$0")/../../.."
TAG=${1:?tag}; STEP=${2:?step}
O=tests/am2/raw/wp3-$TAG; mkdir -p "$O"
W=$HOME/.graft-m4a; K=$W/wp3; mkdir -p "$K"; chmod 700 "$K"
STORE=${CF_SECRETS_STORE_ID:?}
OPS=graft-m4a-ops EDGE_NAME=graft-m4a-edge FRONT_NAME=graft-m4a-front MCP_NAME=graft-m4a-mcp AUX_NAME=graft-m4a-mcp-aux
ZONE=graftyard.dev
HOSTN=m4a.$ZONE MCPH=mcp.m4a.$ZONE
# The edge's live FORMATS_ENABLED (`live_formats`, read at the step; saved to $K/formats-before), so the day adds 20.4
# and 17.4 to whatever BF-47 left and the teardown puts back exactly that list.
B="https://api.cloudflare.com/client/v4/accounts/$CLOUDFLARE_ACCOUNT_ID"
api() { curl -s -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" "$1" "${@:2}"; }
scrub() {
  sed -e "s/$CLOUDFLARE_ACCOUNT_ID/<install-account>/g" -e "s/$CF_SECRETS_STORE_ID/<store>/g" -e "s/$CF_WORKERS_SUBDOMAIN/<sub>/g" \
      -e "s#$PWD/#<checkout>/#g" -e "s#$HOME/#~/#g"
}
N=$(ls "$O" | grep -c "^$STEP" || true)
L="$O/$STEP$( [ "$N" -gt 0 ] && echo "-$((N + 1))" ).log"
log() { { date -u +%FT%TZ; git rev-parse --short HEAD; cat; } | scrub >"$L"; cat "$L"; }
zone_id() { api "https://api.cloudflare.com/client/v4/zones?name=$ZONE" | jq -r '.result[0].id'; }
secret_id() { api "$B/secrets_store/stores/$STORE/secrets?per_page=100" | jq -r --arg n "$1" '.result[] | select(.name == $n) | .id'; }
cronoff() {
  api "$B/workers/scripts/$OPS/schedules" -X PUT -H 'content-type: application/json' --data '[{"cron":"0 0 1 1 *"}]' >/dev/null
  sleep 2
  api "$B/workers/scripts/$OPS/schedules" -X PUT -H 'content-type: application/json' --data '[]' | jq -c '{schedules: .result.schedules}'
}
deployment() { api "$B/workers/scripts/$1/deployments" | jq -c --arg n "$1" '.result.deployments[0] | {script: $n, created_on, versions: [.versions[] | "\(.version_id[0:8]) \(.percentage)"]}'; }
entries() { node tests/m5/bf46/view.mjs entries "${1:-0}"; }
live_formats() { api "$B/workers/scripts/$EDGE_NAME/settings" | jq -r '.result.bindings[] | select(.name == "FORMATS_ENABLED") | .text'; }
live_export() { api "$B/workers/scripts/$EDGE_NAME/settings" | jq -r '.result.bindings[] | select(.name == "EXPORT") | .bucket_name'; }
edge_day() {
  local x; x=$(live_export)
  node tests/m1/integ/deploy-edge.mjs "$W/sites.json" $EDGE_NAME --test-faults --anchor "$W/install.toml" --ops $OPS \
    --web /tmp/wp3web --var "FORMATS_ENABLED=$1" ${x:+--export-bucket $x} \
    --var RUN_SERVICE_PUBLIC=35f3192f9087eceb3316c4abecb151fd3f324daceb83565a4a5d93c0d5a06f38 ${2:+--var HOST=$2} 2>&1 |
    grep -E "Uploaded|Deployed|Current Version|rror" | tail -4
}

case $STEP in
domains)
  { Z=$(zone_id)
    echo "zone $ZONE custom domains (account Workers domains on it):"
    api "$B/workers/domains?zone_name=$ZONE" | jq -c '[.result[] | {hostname, service}]'
    echo "zone $ZONE DNS records:"; api "https://api.cloudflare.com/client/v4/zones/$Z/dns_records?per_page=100" | jq -c '[.result[] | "\(.type) \(.name)"]'
    echo "zone $ZONE spectrum apps:"; api "https://api.cloudflare.com/client/v4/zones/$Z/spectrum/apps" | jq -c '[.result[]? | .dns.name]'
    echo "zone $ZONE certificate packs:"; api "https://api.cloudflare.com/client/v4/zones/$Z/ssl/certificate_packs?status=all" | jq -c '[.result[]? | {id: .id[0:8], type, hosts, status}]'
  } 2>&1 | log ;;
token)
  { ( umask 077
      jq -nc --arg a "com.cloudflare.api.account.$CLOUDFLARE_ACCOUNT_ID" \
        '{name: "graft-m4a-wp3-deploy", policies: [{effect: "allow", resources: {($a): "*"},
          permission_groups: [{id: "e086da7e2179491d91ee5f35b3ca210a"}, {id: "bdbcd690c763475a985e8641dddc09f7"}]}]}' |
        api "$B/tokens" -X POST -H 'content-type: application/json' --data-binary @- >"$K/deploy.json"
      jq -c '{value: .result.value}' "$K/deploy.json" >"$K/deploy.value" )
    jq -c '{mint: "graft-m4a-wp3-deploy", ok: .success, id: (.result.id // "" | .[0:8]), errors: [.errors[]?.code]}' "$K/deploy.json"
    api "$B/secrets_store/stores/$STORE/secrets/$(secret_id graft-m4a-ops-deploy-token)" -X PATCH \
      -H 'content-type: application/json' --data-binary @"$K/deploy.value" | jq -c '{patch: "graft-m4a-ops-deploy-token", ok: .success}'
  } 2>&1 | log ;;
edge)
  { deployment $EDGE_NAME
    echo "live FORMATS_ENABLED: $(live_formats); EXPORT: $(live_export)"
    if [ "${3:-on}" = restore ]; then
      # The lead's ruling: the handoff's split back by version id (`<id>@<pct>` ...), with those versions' own vars.
      npx --prefix tests/m0 wrangler versions deploy "${@:4}" --name $EDGE_NAME -y 2>&1 | grep -E "Deployed|rror" | tail -2
    elif [ "${3:-on}" = off ]; then
      edge_day "$(cat "$K/formats-before")"
    else
      [ -s "$K/formats-before" ] || live_formats >"$K/formats-before"
      # HOST: the front's hostname, which every request through graft-front carries; the edge's workers.dev name
      # (deploy-edge.mjs's default) misses `Host::endpoint` and answers 404 (day d1's first session N0).
      edge_day "$(cat "$K/formats-before"),20.4,17.4" "$HOSTN"
    fi
    deployment $EDGE_NAME
    echo "now FORMATS_ENABLED: $(live_formats); EXPORT: $(live_export)"
  } 2>&1 | log ;;
apps)
  { A=$W/apps
    cargo run -q -p graftyard -- --config "$W/install.toml" apps init --host "${GRAFT_EDGE#https://}" --store $STORE \
      --secret-prefix graft-m4a- --mcp-origin "https://$MCPH" --out "$A"
    out=$(node tests/am2/hosted/apps.mjs claims "$A"); echo "$out"
    ids=$(echo "$out" | awk '$1 == "graftyard-mcp" || $1 == "graftyard-insights" {print $1, $2}')
    mcp=$(echo "$ids" | awk '$1 == "graftyard-mcp" {print $2}'); ins=$(echo "$ids" | awk '$1 == "graftyard-insights" {print $2}')
    echo "graftyard-mcp $mcp graftyard-insights $ins" | tee "$K/app-ids"
    cargo run -q -p graftyard -- apps init --app-ids "$mcp,$ins" --out "$A" && node tests/am2/hosted/apps.mjs register "$A"
    # Beside aux.jsonc, so its relative `main` resolves (a `<(...)` config does not).
    sed "s/graft-m3f-mcp-aux/$AUX_NAME/" tests/am2/hosted/aux.jsonc >tests/am2/hosted/.aux-m4a.jsonc
    npx --prefix tests/m0 wrangler deploy -c tests/am2/hosted/.aux-m4a.jsonc 2>&1 | grep -E "Current Version|rror"
    rm -f tests/am2/hosted/.aux-m4a.jsonc
    node workers/mcp/deploy.mjs $MCP_NAME "$A/apps.json" --edge $EDGE_NAME --store $STORE --app-id "$mcp" \
      --host-origin "https://$HOSTN" 2>&1 | grep -E "Current Version|rror"
    deployment $MCP_NAME
  } 2>&1 | log ;;
front)
  { node workers/front/deploy.mjs "$W/sites.json" $FRONT_NAME --edge $EDGE_NAME --anchor "$W/install.toml" \
      --mcp $MCP_NAME --mcp-host $MCPH --domain $HOSTN --domain $MCPH 2>&1 | grep -E "Current Version|$ZONE|rror"
    ( umask 077; [ -s "$K/front.token" ] || python3 -c 'import os; print(os.urandom(24).hex())' >"$K/front.token" )
    npx --prefix tests/m0 wrangler secret put FRONT_TOKEN --name $FRONT_NAME <"$K/front.token" 2>&1 | tail -1
    deployment $FRONT_NAME
  } 2>&1 | log ;;
bundles)
  { git fetch -q origin main
    for s in front mcp; do (cd workers/$s && worker-build --release --out-dir build >/dev/null 2>&1) && echo "$s built"; done
    aws() { npx --prefix tests/m0 wrangler r2 object put "graft-m4a-releases/$1" --file "$2" --remote 2>&1 | tail -1; }
    fmt=$K/formats.txt
    # The formats of the release the install last rolled (BF-47's R9, `FMT_FROM`), so w1..w3 read every enabled
    # format, 65000.1 and anything BF-47 enabled included.
    npx --prefix tests/m0 wrangler r2 object get "graft-m4a-releases/${FMT_FROM:?the last rolled release prefix}/formats.txt" --file "$fmt" --remote >/dev/null 2>&1
    echo "formats: $(tr '\n' ' ' <"$fmt")"
    for b in "w1:" "w2:--marker" "w3:--marker"; do
      l=${b%%:*}; d=$K/rel/$l; rm -rf "$d"
      python3 tests/am2/hosted/wp3-bundle.py "$d" "am2-wp3-$TAG-$l" --formats "$fmt" --front workers/front/build \
        --mcp workers/mcp/build ${b#*:} | tee "$K/rel-$l.json"
      openssl pkeyutl -sign -rawin -inkey "$W/release.key" -in "$d/release.json" -out "$d/release.sig"
      (cd "$d" && find . -type f | sed 's#^\./##') | while read -r f; do aws "$l/$f" "$d/$f" >/dev/null; done
      echo "$l uploaded under graft-m4a-releases/$l/"
    done
  } 2>&1 | log ;;
ops)
  F=${3:?exec file}
  { (cd /tmp && rm -rf wp3-ops && git -C "$OLDPWD" worktree add -f --detach /tmp/wp3-ops "${OPS_REF:-origin/main}" >/dev/null 2>&1)
    echo "graft-ops from $(git -C /tmp/wp3-ops rev-parse --short HEAD), $(jq -c '{gate, scripts: (.scripts | keys), test_faults, cron}' "$F")"
    (cd /tmp/wp3-ops && unset CARGO_TARGET_DIR && export PATH=$HOME/.cargo/bin:$PATH \
      && ln -sfn "$HOME/workspace/repo/tests/m0/node_modules" tests/m0/node_modules \
      && node workers/ops/deploy.mjs $OPS $STORE graft-m4a-mint-wrap-key - graft-m4a-secrets-wrap-key-b 1 \
           --executor "$F" 2>&1 | grep -E "Current Version|rror" | tail -3)
    deployment $OPS
    api "$B/workers/scripts/$OPS/schedules" | jq -c '{schedules: [.result.schedules[]?.cron]}'
  } 2>&1 | log ;;
roll)
  R=${3:?release id}
  { node tests/m5/bf46/view.mjs pages "$K/entries.json"
    "$G" --config "$W/install.toml" release roll "$R" --entries "$K/entries.json" --key "$W/customer.key" --ttl-s 21600 \
      --edge "$GRAFT_EDGE"
  } 2>&1 | tee -a "$O/answers.txt" | log ;;
state)
  { node tests/m5/bf46/view.mjs pages "$K/entries.json" >/dev/null; "$RELVIEW" "$(sed -n 's/^install *= *"\(.*\)"/\1/p' "$W/install.toml")" \
      "$(sed -n 's/^customer_key *= *"\(.*\)"/\1/p' "$W/install.toml")" 0 <"$K/entries.json" |
      jq -c '{count, current, drained, site_release, rolled, uploads, enabled}'
    for n in $EDGE_NAME $OPS graft-m4a-site-a graft-m4a-site-b graft-m4a-site-c; do deployment $n; done
    api "$B/workers/scripts/$OPS/schedules" | jq -c '{ops_schedules: [.result.schedules[]?.cron]}'
    echo "edge FORMATS_ENABLED: $(live_formats); EXPORT: $(live_export)"
  } 2>&1 | log ;;
view)
  { deployment $FRONT_NAME; deployment $MCP_NAME; deployment $OPS
    entries "$(cat "$O/start.ms" 2>/dev/null || echo 0)" | tail -40
  } 2>&1 | log ;;
block)
  { echo '{"value":"wp3-blocked-invalid-token"}' |
      api "$B/secrets_store/stores/$STORE/secrets/$(secret_id graft-m4a-ops-deploy-token)" -X PATCH -H 'content-type: application/json' --data-binary @- |
      jq -c '{block: .success, modified: .result.modified}'
    echo '{"value":"wp3-blocked-invalid-token"}' | jq -r .value | (read -r t; curl -s -o /dev/null -w 'probe %{http_code}\n' -H "Authorization: Bearer $t" "$B/workers/scripts/$OPS/deployments")
  } 2>&1 | log ;;
unblock)
  { api "$B/secrets_store/stores/$STORE/secrets/$(secret_id graft-m4a-ops-deploy-token)" -X PATCH -H 'content-type: application/json' \
      --data-binary @"$K/deploy.value" | jq -c '{unblock: .success, modified: .result.modified}'
  } 2>&1 | log ;;
session)
  { GRAFT_EDGE=https://$HOSTN GRAFT_MCP=https://$MCPH GRAFT_AUX=https://$AUX_NAME.$CF_WORKERS_SUBDOMAIN.workers.dev REPORT=1 LIST=1 \
      node tests/am2/hosted/ts-client.mjs v2 1
  } 2>&1 | log ;;
sampler)
  case ${3:?start|stop} in
    start) rm -f "$O/stop"; date -u +%s%3N >>"$O/sampler-starts"
           amp orb service start wp3-sampler --command "cd $PWD && node tests/m5/wp2-sample.mjs front https://$MCPH /.well-known/oauth-protected-resource 8 $PWD/$O/mcp.jsonl $PWD/$O/stop" 2>&1 | head -1 | log ;;
    stop) touch "$O/stop"; sleep 3; amp orb service stop wp3-sampler 2>&1 | tail -1 | log ;;
  esac ;;
teardown)
  { touch "$O/stop"; amp orb service stop wp3-sampler >/dev/null 2>&1
    cronoff
    # The lead's ruling: graft-ops parks as the version the handoff named (OPS_PARKED, a version id), not a rebuild.
    echo "graft-ops parked as ${OPS_PARKED:?OPS_PARKED version id}"
    npx --prefix tests/m0 wrangler versions deploy "$OPS_PARKED@100" --name $OPS -y 2>&1 | grep -E "Deployed|rror" | tail -2
    cronoff
    deployment $OPS
    for n in $FRONT_NAME $MCP_NAME $AUX_NAME; do npx --prefix tests/m0 wrangler delete --name $n --force 2>&1 | tail -1; done
    for n in MCP_APP_KEY_A INSIGHTS_APP_KEY_A MCP_CLIENT_SECRET_A MCP_SEAL_KEY_A; do
      id=$(secret_id "graft-m4a-$n")
      [ -n "$id" ] && api "$B/secrets_store/stores/$STORE/secrets/$id" -X DELETE | jq -c --arg n "graft-m4a-$n" '{delete: $n, ok: .success}'
    done
    id=$(api "$B/tokens?per_page=50" | jq -r '.result[] | select(.name == "graft-m4a-wp3-deploy") | .id')
    [ -n "$id" ] && api "$B/tokens/$id" -X DELETE | jq -c '{revoke: "graft-m4a-wp3-deploy", ok: .success}'
    for f in "$K"/deploy.json "$K"/deploy.value "$K"/front.token; do [ -f "$f" ] && shred -u "$f"; done
    git worktree remove --force /tmp/wp3-ops 2>/dev/null; echo "row token files shredded"
  } 2>&1 | log ;;
*) echo "step: state domains token edge apps front bundles ops roll view block unblock session sampler teardown"; exit 2 ;;
esac
