#!/bin/bash
# R47's hosted row on graft-m4a (`decisions/OPEN-QUESTIONS.md`, R47; `tests/am2/results/am2-wp4.md`, "R47 hosted"):
# a bad `graft-mcp` at 1% with `GATE` unset and the deploy token invalid, which `graft-ops` must breach on
# `GATE_ERRORS` by itself (`graftyard_release::gate::judge_roll`, from `graft-front`'s counts in the sites' `errors`
# objects) and swap back with the pre-signed `McpPointer`. One step per call, each logged to
# tests/am2/raw/r47-<tag>/<step>.log. The lead's order (2026-11-24): a site day for `/errors`; the front and mcp
# redeploys plus OPS_SCRIPTS; w4 and the two Aborts after the step-0 check; then the bad graft-mcp.
#
#   bash tests/am2/hosted/r47-day.sh <tag> <step> [args]
#
# Needs `source tests/am1/lf13/env-m4a.sh`, TOOLS_TOKEN_FILE and BED_TOKEN_FILE, CLOUDFLARE_*, CF_WORKERS_SUBDOMAIN,
# ~/.graft-m4a with apps/ and wp3/app-ids from AM2-WP3's day, G (the graftyard binary) and RELVIEW. Token values are
# read only from 0600 files and sent in bodies or headers; every log line goes through `scrub`.
#
# Steps:
#   state             relview's count, current, rolled, site_release; deployments and schedules; spend
#   token             mint graft-m4a-r47-deploy (Workers Scripts Write, Containers Write), PATCH it into the ops secret
#   siteday           all three sites from this checkout, built test-faults, with site day 70's flags and image; then
#                     the site crons reset; then RERETIRE
#   secrets           graft-mcp's two Secrets Store secrets, fresh values (no `apps init`: no App is re-registered)
#   mcp               graft-mcp from this checkout with AM2-WP3's apps.json and App id
#   domains           the zone's custom domains, DNS records and certificate packs (before and after)
#   front             graft-front with --mcp, --mcp-host and both custom domains; FRONT_TOKEN
#   bundles           front/mcp builds; w4 (N: this checkout's front and mcp) and w5 (N+1: only the bad mcp from
#                     tests/am2/hosted/r47-bad) under graft-m4a-releases/r47<tag>-w4/ and -w5/
#   ops <exec file>   graft-m4a-ops from this checkout with <exec file>
#   roll <R>          sign and post the next intent for release R (`release roll`), with its pre-signed undos
#   abort <R>         the step-0 check, then the customer's `Abort { current intent, target: R }`; refuses unless the
#                     current intent's next is 0 and its script's deployment is unchanged from `$K/dep-<script>`
#   view              the group's entries since the row's start, the current intent
#   block|unblock     DEPLOY_TOKEN set to an invalid value, or back to the row token
#   sampler start|stop  wp2-sample.mjs through mcp.<host> into $O/mcp.jsonl, so graft-front counts graft-mcp answers
#   status            graft-front's /_front/status: its pin, mismatch counts, and the GATE_ERRORS counts not yet sent
#   logs <from> [to]  graft-ops's lines (gate judgments with N+1's and N's counts, commits, pages) in the window
#   teardown          cron off, graft-ops parked by version id (OPS_PARKED), front and mcp deleted with their
#                     domains, graft-mcp's two secrets deleted, the row token revoked and shredded
#
# The w4 intents are held at step 0 by the token: with the deploy token blocked, graft-ops reads no deployment and
# waits, so `roll w4` and `abort` run blocked, and `unblock` lets graft-ops run the Abort to IntentDone.
set -uo pipefail
cd "$(dirname "$0")/../../.."
TAG=${1:?tag}; STEP=${2:?step}
O=tests/am2/raw/r47-$TAG; mkdir -p "$O"
W=$HOME/.graft-m4a; K=$W/r47; mkdir -p "$K"; chmod 700 "$K"
STORE=${CF_SECRETS_STORE_ID:?}
OPS=graft-m4a-ops EDGE_NAME=graft-m4a-edge FRONT_NAME=graft-m4a-front MCP_NAME=graft-m4a-mcp AUX_NAME=graft-m4a-mcp-aux
ZONE=graftyard.dev HOSTN=m4a.graftyard.dev MCPH=mcp.m4a.graftyard.dev
IMG=registry.cloudflare.com/$CLOUDFLARE_ACCOUNT_ID/graft-m1-site-gitshard-git@sha256:e55ab2ad178a4d2b639bf53a3c4ca08694024e344343b2e251821368b1d21d41
B="https://api.cloudflare.com/client/v4/accounts/$CLOUDFLARE_ACCOUNT_ID"
api() { curl -s -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" "$1" "${@:2}"; }
scrub() {
  sed -e "s/$CLOUDFLARE_ACCOUNT_ID/<install-account>/g" -e "s/$CF_SECRETS_STORE_ID/<store>/g" -e "s/$CF_WORKERS_SUBDOMAIN/<sub>/g" \
      -e "s#$PWD/#<checkout>/#g" -e "s#$HOME/#~/#g"
}
N=$(ls "$O" | grep -c "^$STEP" || true)
L="$O/$STEP$( [ "$N" -gt 0 ] && echo "-$((N + 1))" ).log"
log() { { date -u +%FT%TZ; git rev-parse --short HEAD; cat; } | scrub >"$L"; cat "$L"; }
secret_id() { api "$B/secrets_store/stores/$STORE/secrets?per_page=100" | jq -r --arg n "$1" '.result[] | select(.name == $n) | .id'; }
reset_cron() {
  api "$B/workers/scripts/$1/schedules" -X PUT -H 'content-type: application/json' --data '[{"cron":"0 0 1 1 *"}]' >/dev/null
  sleep 2
  api "$B/workers/scripts/$1/schedules" -X PUT -H 'content-type: application/json' --data '[]' | jq -c --arg n "$1" '{($n): .result.schedules}'
}
deployment() { api "$B/workers/scripts/$1/deployments" | jq -c --arg n "$1" '.result.deployments[0] | {script: $n, created_on, versions: [.versions[]? | "\(.version_id[0:8]) \(.percentage)"]}'; }
split() { api "$B/workers/scripts/$1/deployments" | jq -c '[.result.deployments[0].versions[]? | [.version_id, .percentage]]'; }
ANCHOR=$(grep -E '^(install|customer_key) ' "$W/install.toml" | sed -E 's/.*= *"([^"]*)"/\1/' | xargs)
view() { node tests/m5/bf46/view.mjs pages "$K/pages.json" >/dev/null && "$RELVIEW" $ANCHOR "${1:-0}" <"$K/pages.json"; }
cost() { PREFIX=graft-m4a python3 tests/fanout/acctcost.py "${1:-$(date -u +%Y-%m-%dT00:00:00Z)}" | head -1 | jq -c '{prefix_usd}'; }

case $STEP in
state)
  { view 0 | jq -c '{count, current, drained, site_release, rolled}'
    for n in $EDGE_NAME $OPS graft-m4a-site-a graft-m4a-site-b graft-m4a-site-c $FRONT_NAME $MCP_NAME; do
      deployment $n; api "$B/workers/scripts/$n/schedules" | jq -c --arg n $n '{($n): [.result.schedules[]?.cron]}'
    done
    echo "install day: $(cost)"
  } 2>&1 | log ;;
token)
  { ( umask 077
      jq -nc --arg a "com.cloudflare.api.account.$CLOUDFLARE_ACCOUNT_ID" \
        '{name: "graft-m4a-r47-deploy", policies: [{effect: "allow", resources: {($a): "*"},
          permission_groups: [{id: "e086da7e2179491d91ee5f35b3ca210a"}, {id: "bdbcd690c763475a985e8641dddc09f7"}]}]}' |
        api "$B/tokens" -X POST -H 'content-type: application/json' --data-binary @- >"$K/deploy.json"
      jq -c '{value: .result.value}' "$K/deploy.json" >"$K/deploy.value" )
    jq -c '{mint: "graft-m4a-r47-deploy", ok: .success, id: (.result.id // "" | .[0:8]), errors: [.errors[]?.code]}' "$K/deploy.json"
    api "$B/secrets_store/stores/$STORE/secrets/$(secret_id graft-m4a-ops-deploy-token)" -X PATCH \
      -H 'content-type: application/json' --data-binary @"$K/deploy.value" | jq -c '{patch: "graft-m4a-ops-deploy-token", ok: .success}'
  } 2>&1 | log ;;
siteday)
  # Site day 70's flags and image (m4a60), built test-faults; `--test-faults` is not passed to deploy.mjs.
  { (cd workers/site && unset CARGO_TARGET_DIR && export PATH=$HOME/.cargo/bin:$PATH &&
       worker-build --release --features test-faults >/dev/null 2>&1) && echo "site built test-faults"
    echo "start $(date -u +%FT%TZ)"
    node workers/site/deploy.mjs "$W/sites.json" --anchor "$W/install.toml" --image "$IMG" --image-prev "$IMG" \
      --vars "$W/site-vars.json" --skip-build --ops $OPS \
      --var GRAFTYARD_HOST=graft-m4a-edge.$CF_WORKERS_SUBDOMAIN.workers.dev --var GRAFTYARD_ACTIVITY=1 --var GRAFTYARD_AUDIT=1 \
      --var GRAFTYARD_APP_HOOKS=1 --var GRAFTYARD_PR_TRIGGER=1 \
      --audit-stream 2a0357004afd44d9a74cf410dd2c715a --run-key $STORE/graft-m4a-run-service-key \
      >"$O/siteday-deploy.raw" 2>&1
    echo "exit $? at $(date -u +%FT%TZ)"
    grep -E "Current Version|rror" "$O/siteday-deploy.raw" | tail -8; rm -f "$O/siteday-deploy.raw"
    for s in a b c; do reset_cron graft-m4a-site-$s; done
    for s in a b c; do deployment graft-m4a-site-$s; done
    RETIRE_LOG=$O/reretire.log bash tests/fanout/reretire.sh tests/fanout/reretire/graft-m4a.txt r47-$TAG 2>&1 | tail -1
  } 2>&1 | log ;;
secrets)
  # graft-mcp's two Secrets Store bindings with fresh values: the row's traffic is the RFC 9728 metadata, which no
  # App credential reaches, so `apps init` (which would re-register the Apps) is not run.
  { for n in MCP_CLIENT_SECRET_A MCP_SEAL_KEY_A; do
      v=$(python3 -c 'import os; print(os.urandom(32).hex())')
      jq -nc --arg n "graft-m4a-$n" --arg v "$v" '[{name: $n, value: $v, comment: "R47 row", scopes: ["workers"]}]' |
        api "$B/secrets_store/stores/$STORE/secrets" -X POST -H 'content-type: application/json' --data-binary @- |
        jq -c --arg n "graft-m4a-$n" '{create: $n, ok: .success, errors: [.errors[]?.code]}'
    done
  } 2>&1 | log ;;
mcp)
  { read -r _ mcp _ <"$W/wp3/app-ids"
    node workers/mcp/deploy.mjs $MCP_NAME "$W/apps/apps.json" --edge $EDGE_NAME --store $STORE --app-id "$mcp" \
      --host-origin "https://$HOSTN" 2>&1 | grep -E "Current Version|rror"
    deployment $MCP_NAME
  } 2>&1 | log ;;
domains)
  { Z=$(api "https://api.cloudflare.com/client/v4/zones?name=$ZONE" | jq -r '.result[0].id')
    echo "zone $ZONE Workers custom domains:"; api "$B/workers/domains?zone_name=$ZONE" | jq -c '[.result[] | {hostname, service}]'
    echo "zone $ZONE DNS records:"; api "https://api.cloudflare.com/client/v4/zones/$Z/dns_records?per_page=100" | jq -c '[.result[] | "\(.type) \(.name)"]'
    echo "zone $ZONE certificate packs:"; api "https://api.cloudflare.com/client/v4/zones/$Z/ssl/certificate_packs?status=all" | jq -c '[.result[]? | {id: .id[0:8], type, hosts, status}]'
  } 2>&1 | log ;;
front)
  { node workers/front/deploy.mjs "$W/sites.json" $FRONT_NAME --edge $EDGE_NAME --anchor "$W/install.toml" \
      --mcp $MCP_NAME --mcp-host $MCPH --domain $HOSTN --domain $MCPH 2>&1 | grep -E "Current Version|$ZONE|rror"
    ( umask 077; [ -s "$K/front.token" ] || python3 -c 'import os; print(os.urandom(24).hex())' >"$K/front.token" )
    npx --prefix tests/m0 wrangler secret put FRONT_TOKEN --name $FRONT_NAME <"$K/front.token" 2>&1 | tail -1
    deployment $FRONT_NAME
  } 2>&1 | log ;;
bundles)
  { for s in front mcp; do (cd workers/$s && worker-build --release --out-dir build >/dev/null 2>&1) && echo "$s built"; done
    put() { npx --prefix tests/m0 wrangler r2 object put "graft-m4a-releases/$1" --file "$2" --remote 2>&1 | tail -1; }
    fmt=$W/wp3/formats.txt; echo "formats: $(wc -l <"$fmt") lines, from AM2-WP3's FMT_FROM"
    # w4 carries front/ and mcp/ (N for both); w5 only mcp/, so `release roll w5` signs an McpRoll and no FrontRoll.
    # N1=clean (the McpRoll 100%-hold row): w5 is this checkout's mcp again with a marker module, a healthy N+1.
    w5="w5:--mcp tests/am2/hosted/r47-bad"; [ "${N1:-bad}" = clean ] && w5="w5:--mcp workers/mcp/build --marker"
    # BUNDLES overrides the list, as `<label>:<wp3-bundle.py flags>` items separated by `;` (h1's w4m: an mcp-only N
    # whose files differ from w4's, because main's mcp build matched r1's w4 and `release roll w4` skipped the McpRoll).
    IFS=';' read -ra list <<<"${BUNDLES:-w4:--front workers/front/build --mcp workers/mcp/build;$w5}"
    for b in "${list[@]}"; do
      l=${b%%:*}; d=$K/rel/$l; rm -rf "$d"; p=r47$TAG-$l
      python3 tests/am2/hosted/wp3-bundle.py "$d" "$p" --formats "$fmt" ${b#*:} | tee "$K/rel-$l.json"
      openssl pkeyutl -sign -rawin -inkey "$W/release.key" -in "$d/release.json" -out "$d/release.sig"
      (cd "$d" && find . -type f | sed 's#^\./##') | while read -r f; do put "$p/$f" "$d/$f" >/dev/null; done
      echo "$l uploaded under graft-m4a-releases/$p/"
    done
  } 2>&1 | log ;;
ops)
  F=${3:?exec file}
  { echo "graft-ops from $(git rev-parse --short HEAD), $(jq -c '{gate, scripts: (.scripts | keys), test_faults, cron, step_dwell_ms, first_day_ms, step_min_entries}' "$F")"
    (export PATH=$HOME/.cargo/bin:$PATH && unset CARGO_TARGET_DIR && \
      node workers/ops/deploy.mjs $OPS $STORE graft-m4a-mint-wrap-key - graft-m4a-secrets-wrap-key-b 1 \
        --executor "$F" 2>&1 | grep -E "Current Version|rror" | tail -3)
    deployment $OPS
    api "$B/workers/scripts/$OPS/schedules" | jq -c '{schedules: [.result.schedules[]?.cron]}'
  } 2>&1 | log ;;
roll)
  R=${3:?release id}
  { node tests/m5/bf46/view.mjs pages "$K/pages.json" >/dev/null
    for s in $FRONT_NAME $MCP_NAME; do split $s >"$K/dep-$s"; done
    "$G" --config "$W/install.toml" release roll "$R" --entries "$K/pages.json" --key "$W/customer.key" --ttl-s 21600 \
      --edge "$GRAFT_EDGE"
    view 0 | jq -c '{count, current}'
  } 2>&1 | log ;;
abort)
  R=${3:?target release id}
  { v=$(view 0); c=$(echo "$v" | jq -c .current); echo "current: $c"
    case $(echo "$c" | jq -r .intent) in FrontRoll*) s=$FRONT_NAME ;; McpRoll*) s=$MCP_NAME ;; *) s= ;; esac
    now=$(split "$s"); was=$(cat "$K/dep-$s" 2>/dev/null)
    echo "step-0 check: next $(echo "$c" | jq -r .next), $s deployment $now, at roll $was"
    if [ -z "$s" ] || [ "$(echo "$c" | jq -r .next)" != 0 ] || [ "$now" != "$was" ]; then
      echo "STOP: the intent has stepped or is not a FrontRoll/McpRoll; no Abort posted"; exit 3
    fi
    ID=$(echo "$v" | jq -r '[.entries[] | select(.kind == "Intent")][-1].entry' | sed 's/.* id //')
    echo "abort intent $ID to $R"
    echo "$v" | jq -r .keys_entry | python3 -c "import sys; sys.stdout.buffer.write(bytes.fromhex(sys.stdin.read().strip()))" >"$K/keys.entry"
    echo "{\"Abort\": {\"intent\": \"$ID\", \"target\": \"$R\"}}" >"$K/abort.json"
    "$G" --config "$W/install.toml" release entry "$K/abort.json" --out "$K/abort.unsigned" --prev "$(echo "$v" | jq -r .prev)" \
      --keys "$K/keys.entry" >/dev/null &&
      "$G" sign "$K/abort.unsigned" --out "$K/abort.signed" --key "$W/customer.key" >/dev/null &&
      "$G" --config "$W/install.toml" release post "$K/abort.signed" --edge "$GRAFT_EDGE" 2>&1
    view 0 | jq -c '{count, current}'
  } 2>&1 | log ;;
view)
  { deployment $FRONT_NAME; deployment $MCP_NAME; deployment $OPS
    view "${3:-0}" | jq -c '{count, current, rolled, entries}'
  } 2>&1 | log ;;
block)
  { echo '{"value":"r47-blocked-invalid-token"}' |
      api "$B/secrets_store/stores/$STORE/secrets/$(secret_id graft-m4a-ops-deploy-token)" -X PATCH -H 'content-type: application/json' --data-binary @- |
      jq -c '{block: .success, modified: .result.modified}'
    curl -s -o /dev/null -w 'probe %{http_code}\n' -H "Authorization: Bearer r47-blocked-invalid-token" "$B/workers/scripts/$OPS/deployments"
  } 2>&1 | log ;;
unblock)
  { api "$B/secrets_store/stores/$STORE/secrets/$(secret_id graft-m4a-ops-deploy-token)" -X PATCH -H 'content-type: application/json' \
      --data-binary @"$K/deploy.value" | jq -c '{unblock: .success, modified: .result.modified}'
  } 2>&1 | log ;;
sampler)
  case ${3:?start|stop} in
    start) rm -f "$O/stop"; date -u +%s%3N >>"$O/sampler-starts"
           amp orb service start r47-sampler --command "cd $PWD && node tests/m5/wp2-sample.mjs front https://$MCPH /.well-known/oauth-protected-resource ${4:-2} $PWD/$O/mcp.jsonl $PWD/$O/stop" 2>&1 | head -1 | log ;;
    stop) touch "$O/stop"; sleep 3; amp orb service stop r47-sampler 2>&1 | tail -1 | log ;;
  esac ;;
status)
  { curl -s -H "x-front-token: $(cat "$K/front.token")" "https://$HOSTN/_front/status" | jq -c '{front, mcpPin, counts, errorsPending}'
  } 2>&1 | log ;;
logs)
  { KEEP="graft-ops gate,PAGE,IntentStep,IntentDone,Committed,Paged,Error,ALARM" \
      python3 tests/m5/bf46/logs.py $OPS "${3:?from ms}" "${4:-$(date +%s%3N)}" | tail -${5:-60}
  } 2>&1 | log ;;
teardown)
  { touch "$O/stop"; amp orb service stop r47-sampler >/dev/null 2>&1
    reset_cron $OPS
    echo "graft-ops parked as ${OPS_PARKED:?OPS_PARKED version id}"
    npx --prefix tests/m0 wrangler versions deploy "$OPS_PARKED@100" --name $OPS -y 2>&1 | grep -E "Deployed|rror" | tail -2
    reset_cron $OPS
    deployment $OPS
    for n in $FRONT_NAME $MCP_NAME; do npx --prefix tests/m0 wrangler delete --name $n --force 2>&1 | tail -1; done
    for n in MCP_CLIENT_SECRET_A MCP_SEAL_KEY_A; do
      id=$(secret_id "graft-m4a-$n")
      [ -n "$id" ] && api "$B/secrets_store/stores/$STORE/secrets/$id" -X DELETE | jq -c --arg n "graft-m4a-$n" '{delete: $n, ok: .success}'
    done
    id=$(api "$B/tokens?per_page=50" | jq -r '.result[] | select(.name == "graft-m4a-r47-deploy") | .id')
    [ -n "$id" ] && api "$B/tokens/$id" -X DELETE | jq -c '{revoke: "graft-m4a-r47-deploy", ok: .success}'
    for f in "$K"/deploy.json "$K"/deploy.value "$K"/front.token; do [ -f "$f" ] && shred -u "$f"; done
    echo "row token files shredded"
  } 2>&1 | log ;;
*) echo "step: state token siteday secrets mcp domains front bundles ops roll abort view block unblock sampler status logs teardown"; exit 2 ;;
esac
