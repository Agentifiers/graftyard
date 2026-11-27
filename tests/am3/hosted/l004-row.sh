#!/bin/bash
# LM-3's L0-04 row on graft-m4a (`build/BUILD-PLAN.md`, L0-04; `decisions/DECISIONS.md`, "`secrets-wait`"; the lead's
# GO, 2026-11-27): agent CI under `ci: AutoReadOnly` with Actions format 0x15.14. A copy of `gate-row.sh`: the agents
# half (`init` to `probe-deploy`) is `lf7-day.sh`'s with ROW=l004 TB=1 (state under ~/.graft-m4a/l004), `install` and
# `hook` are kill-row.sh's, the controller runs the hold harness (`tests/am3/hosted/gate/hold`: two pushes to the task's
# work branch, then held while `refs/heads/hold/<task>` exists), and the site and edge days are the batched row's. One
# step per call, each logged to tests/am3/raw/l004-<tag>/<step>.log through `scrub`; the days log under
# tests/m4/raw/wp3/lm3l004/<tag>/. Spend stops at $2.00 (STOP_USD) across graft-m4a and the runner account's
# `graft-agents-m4a-` and `graft-runners`, from the baseline `start` records, read before every step that starts
# anything.
#
#   bash tests/am3/hosted/l004-row.sh <tag> <step>
#
# Steps, in order:
#   start        the baseline: the time, and each account's spend since 00:00Z, before anything runs
#   state        the install's scripts, the live GRAFTYARD_SECRETS_WAIT and 21.14, the row's spend
#   site1        a site day from TREE (test-faults build done, BUILD_LOG ending "build exit 0"), every live GRAFTYARD_*
#                flag of site a kept, nothing added: 0x15.14's reader on every member
#   site2        the same with GRAFTYARD_SECRETS_WAIT=1 added (GRAFTYARD_AGENT_CI=1 is live and kept)
#   edge         an edge day from TREE: the live FORMATS_ENABLED plus 21.14, nothing else changed (WEB copied to
#                /tmp/bf71web, the old copy kept as .prev)
#   init | register | keys | infra | front | probe-deploy     lf7-day.sh's, as ROW=l004 TB=1
#   taskbox      $K/taskbox.json: the hold harness (two pushes 15 s apart, then held up to 100 minutes), no model arm
#   controller   lf7-day.sh's controller with that taskbox (`--test-faults --taskbox`); run again with FRONTB=1 once the
#                front exists
#   install | hook   kill-row.sh's: both Apps installed on m3org, agents enabled in-repo; graftyard-agent's webhook to
#                the front, a signed ping checked
#   repo | l004  `l004.mjs <line>`
#   teardown     the repository deleted, the policy reset, the value file shredded; both installations removed;
#                lf7-day.sh's teardown (every graft-agents-m4a- script, secret, token, bucket; no KEEP_SECRETS: the
#                store holds no secret of another row, lead 2026-11-27); every container application of
#                graft-agents-m4a- listed, failing loudly while any instance runs, then deleted; graft-runners'
#                Sandboxes listed, failing loudly while any runs; the install's shard sweep and shard kill
#   cost         the three spends since `start`
#
# Needs `. tests/am1/lf13/env-m4a.sh`, CF_SECRETS_STORE_ID, CLOUDFLARE_* and CLOUDFLARE_RUNNER_*, CF_WORKERS_SUBDOMAIN
# and CF_RUNNER_WORKERS_SUBDOMAIN, ~/.graft-m4a, G, RELVIEW, and for the days TREE, BUILD_LOG and WEB.
set -uo pipefail
cd "$(dirname "$0")/../../.."
TAG=${1:?tag}; STEP=${2:?step}
: "${CF_SECRETS_STORE_ID:?export CF_SECRETS_STORE_ID first (lf7-day.sh reads it)}"
export ROW=l004 TB=1
REPO=$PWD
O=tests/am3/raw/l004-$TAG; mkdir -p "$O"
D=tests/m4/raw/wp3/lm3l004/$TAG; mkdir -p "$D"
W=$HOME/.graft-m4a; K=$W/l004; mkdir -p "$K"; chmod 700 "$K"
P=graft-agents-m4a; CTRL=$P-controller; FRONT=$P-front; PROBE=$P-probe
RSTORE=$(cat "$HOME/.graft-m4a/runner-store")  # the runner account's store id, kept off the branch
EDGE_HOST=graft-m4a-edge.$CF_WORKERS_SUBDOMAIN.workers.dev
FRONT_URL=https://$FRONT.$CF_RUNNER_WORKERS_SUBDOMAIN.workers.dev
PROBE_URL=https://$PROBE.$CF_RUNNER_WORKERS_SUBDOMAIN.workers.dev
B="https://api.cloudflare.com/client/v4/accounts/$CLOUDFLARE_ACCOUNT_ID"
RB="https://api.cloudflare.com/client/v4/accounts/$CLOUDFLARE_RUNNER_ACCOUNT_ID"
STOP_USD=${STOP_USD:-2.00}
api() { curl -s -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" "$1" "${@:2}"; }
rapi() { curl -s -H "Authorization: Bearer $CLOUDFLARE_RUNNER_API_TOKEN" "$1" "${@:2}"; }
runner() { CLOUDFLARE_ACCOUNT_ID=$CLOUDFLARE_RUNNER_ACCOUNT_ID CLOUDFLARE_API_TOKEN=$CLOUDFLARE_RUNNER_API_TOKEN "$@"; }
scrub() {
  sed -e "s/$CLOUDFLARE_ACCOUNT_ID/<install-account>/g" -e "s/$CLOUDFLARE_RUNNER_ACCOUNT_ID/<runner-account>/g" \
      -e "s/$CF_WORKERS_SUBDOMAIN/<sub>/g" -e "s/$CF_RUNNER_WORKERS_SUBDOMAIN/<runner-sub>/g" \
      -e "s/$RSTORE/<runner-store>/g" -e "s/$CF_SECRETS_STORE_ID/<install-store>/g" -e "s#$PWD/#<checkout>/#g" -e "s#$HOME/#~/#g"
}
N=$(ls "$O" | grep -c "^$STEP" || true)
L="$O/$STEP$( [ "$N" -gt 0 ] && echo "-$((N + 1))" ).log"
log() { { date -u +%FT%TZ; git rev-parse --short HEAD; cat; } | scrub >"$L"; cat "$L"; }
day() { bash tests/am3/hosted/lf7-day.sh "$TAG" "$@"; }
l004() { node tests/am3/hosted/l004.mjs "$1" "$O" "l004-$TAG" "$K"; }
deployment() { api "$B/workers/scripts/$1/deployments" | jq -c --arg n "$1" '.result.deployments[0] | {script: $n, created_on, versions: [.versions[]? | "\(.version_id[0:8]) \(.percentage)"]}'; }
# One account's spend for a prefix since <from>.
usd() { PREFIX=$2 python3 tests/fanout/acctcost.py "$1" | head -1 | jq -r '.prefix_usd // 0'; }
rusd() { runner env PREFIX="$2" python3 tests/fanout/acctcost.py "$1" | head -1 | jq -r '.prefix_usd // 0'; }
# The row's spend since `start`, all three; fails past STOP_USD.
spend() {
  [ -s "$O/cost.from" ] || { echo "no baseline: run \`start\` first"; return 1; }
  local from a r h
  from=$(date -u -d @"$(cat "$O/cost.from")" +%FT%TZ)
  a=$(usd "$from" graft-m4a)
  r=$(rusd "$from" $P)
  h=$(rusd "$from" graft-runners)
  echo "spend since $from: graft-m4a \$$a, $P- \$$r, graft-runners \$$h, total \$$(python3 -c "print(round($a + $r + $h, 3))") (stop \$$STOP_USD)"
  if python3 -c "import sys; sys.exit(0 if $a + $r + $h < $STOP_USD else 1)"; then return 0; fi
  echo "STOP: past \$$STOP_USD; run teardown"; return 1
}
# Container applications with a name prefix and their running instances, as JSON lines (kill-row.sh's).
containers() {
  rapi "$RB/containers/applications" | jq -r --arg p "$1" '.result[]? | select(.name | startswith($p)) | "\(.id) \(.name)"' |
    while read -r id name; do
      n=$(rapi "$RB/containers/applications/$id/instances?per_page=1000" | jq '[.result.instances[]? // .result[]? | select(.status.state == "running" or .state == "running")] | length')
      echo "{\"app\": \"$name\", \"running\": ${n:-0}}"
    done
}
# The batched row's site day: every live GRAFTYARD_* flag of site a that BF-73's day does not set itself kept, plus $1.
site_day() {
  local sets='^GRAFTYARD_(HOST|ACTIVITY|AUDIT|APP_HOOKS|PR_TRIGGER|ACTIONS_CACHE|JOB_TOKENS|OIDC_HEAD_REF|OIDC_CLAIMS|PUSH_TRIGGER|SITES|INSTALL|CUSTOMER_KEY|GENESIS.*)$'
  local extra
  extra=$(api "$B/workers/scripts/graft-m4a-site-a/settings" |
    jq -r --arg s "$sets" '[.result.bindings[] | select(.type == "plain_text" and (.name | test("^GRAFTYARD_")) and (.name | test($s) | not)) | "--var \(.name)=\(.text)"] | join(" ")')
  echo "kept as live: ${extra:-none}; added: ${1:-none}"
  grep -q "build exit 0" "${BUILD_LOG:?}" || { echo "the site build did not finish"; exit 1; }
  echo "TREE $(git -C "${TREE:?}" rev-parse --short HEAD)"
  mkdir -p "$HOME/bf71" && cp "$BUILD_LOG" "$HOME/bf71/site-build.log"
  ROW=lm3l004 CLAIMS=1 EXTRA="$extra${1:+ $1}" HOSTV=$EDGE_HOST bash tests/m4/wp3/bf74/row.sh "$TAG/$STEP" site
}
sw_state() {
  for n in graft-m4a-site-a graft-m4a-site-b graft-m4a-site-c graft-m4a-edge; do
    echo "$(deployment $n) $(api "$B/workers/scripts/$n/settings" | jq -c '[.result.bindings[] | select(.type == "plain_text" and (.name | test("AGENT_CI|SECRETS_WAIT|FORMATS_ENABLED"))) | if .name == "FORMATS_ENABLED" then "FORMATS_ENABLED \(.text | split(",") | length) has 21.14: \(.text | split(",") | index("21.14") != null)" else "\(.name)=\(.text)" end]')"
  done
}

case $STEP in
start)
  { day0=$(date -u +%Y-%m-%dT00:00:00Z)
    [ -s "$O/cost.from" ] && echo "baseline already taken at $(date -u -d @"$(cat "$O/cost.from")" +%FT%TZ); kept" || date -u +%s >"$O/cost.from"
    echo "baseline $(date -u -d @"$(cat "$O/cost.from")" +%FT%TZ); spend since $day0 before the row:"
    echo "  graft-m4a \$$(usd "$day0" graft-m4a), $P- \$$(rusd "$day0" $P), graft-runners \$$(rusd "$day0" graft-runners)"
    echo "running containers: agents $(containers "$P-" | jq -sc '[.[] | select(.running > 0)]'), runners $(containers graft-runners- | jq -sc '[.[] | select(.running > 0)]')"
    echo "runner store secrets named $P-: $(rapi "$RB/secrets_store/stores/$RSTORE/secrets?per_page=100" | jq -c --arg p "$P-" '[.result[] | select(.name | startswith($p)) | .name]')"
    sw_state
  } 2>&1 | log ;;
state)
  { sw_state; spend || true; } 2>&1 | log ;;
site1)
  spend >/dev/null || exit 3
  { spend; site_day; } 2>&1 | log ;;
site2)
  spend >/dev/null || exit 3
  { spend; site_day "--var GRAFTYARD_SECRETS_WAIT=1"; } 2>&1 | log ;;
edge)
  spend >/dev/null || exit 3
  { spend
    [ -d "${WEB:?WEB, the web build}" ] || { echo "no web build at $WEB"; exit 1; }
    echo "edge from TREE $(git -C "${TREE:?}" rev-parse --short HEAD)"
    [ -d /tmp/bf71web ] && { rm -rf /tmp/bf71web.prev; mv /tmp/bf71web /tmp/bf71web.prev; }
    cp -r "$WEB" /tmp/bf71web
    ROW=lm3l004 ADDFMT=21.14 HOSTV=$EDGE_HOST bash tests/m4/wp3/bf74/row.sh "$TAG" edge
    live=$(api "$B/workers/scripts/graft-m4a-edge/settings" | jq -r '.result.bindings[] | select(.name == "FORMATS_ENABLED") | .text')
    echo "FORMATS_ENABLED now $(echo "$live" | tr ',' '\n' | wc -l) formats; has 21.14: $(echo ",$live," | grep -c ',21.14,')"
  } 2>&1 | log ;;
init | register | keys | infra | front | probe-deploy | view | status | logs)
  spend >/dev/null || exit 3
  day "$STEP" "${@:3}" ;;
taskbox)
  spend >/dev/null || exit 3
  { read -r app ws <"$K/app-ids"
    (cd tests/am3/hosted/gate/hold && DOCKER_BUILDKIT=0 docker build -q -t $P-sandbox:l004-$TAG . >/dev/null) &&
      runner npx --prefix tests/m0 wrangler containers push $P-sandbox:l004-$TAG 2>&1 | grep -E "digest|Pushed image" | tail -2
    dig=$(docker image inspect $P-sandbox:l004-$TAG --format '{{index .RepoDigests 0}}' 2>/dev/null | sed 's/.*@//')
    [ -n "$dig" ] || dig=$(docker image inspect $P-sandbox:l004-$TAG --format '{{.Id}}')
    img=registry.cloudflare.com/$CLOUDFLARE_RUNNER_ACCOUNT_ID/$P-sandbox@$dig
    jq -n --arg a "$app" --arg w "$ws" --arg store "$RSTORE" --arg host "$EDGE_HOST" --arg p "$P-" --arg img "$img" \
      --arg repo "m3org/l004-$TAG" \
      '{agent_app_id: ($a|tonumber), workspace_app_id: ($w|tonumber), store: $store, image: $img, host: $host,
        harness: ["python3", "/harness.py", $host, $repo, "2", "15"],
        secrets: {AGENT_APP_KEY: "\($p)AGENTS_APP_KEY_A", WORKSPACE_APP_KEY: "\($p)WORKSPACE_APP_KEY_A", MODEL_KEY: null}}' \
      >"$K/taskbox.json"
    [ -n "${FRONTB:-}" ] && jq --arg f $FRONT '. + {agents_front: $f}' "$K/taskbox.json" >"$K/tb.tmp" && mv "$K/tb.tmp" "$K/taskbox.json"
    jq -c '{image, harness, agents_front}' "$K/taskbox.json"
  } 2>&1 | log ;;
controller)
  spend >/dev/null || exit 3
  day controller ;;
install)
  spend >/dev/null || exit 3
  { node tests/am3/hosted/kill.mjs install "$K/agents" "$K/app-ids"; echo "install exit $?"; } 2>&1 | log ;;
hook)
  { ( umask 077; jq -Rc --arg u "$FRONT_URL/" '{url: $u, secret: ., content_type: "json"}' <"$K/webhook.secret" >"$K/hook.body" )
    curl -s -m 60 -X POST -H "x-probe-token: $(cat "$K/probe.token")" -H 'content-type: application/json' \
      --data-binary @"$K/hook.body" "$PROBE_URL/hook" | jq -c '{status, url: .body.url, secret: .body.secret, events: .body.events}'
    shred -u "$K/hook.body"
    body='{"zen":"l004-row hook check"}'
    sig=$(printf '%s' "$body" | openssl dgst -sha256 -hmac "$(cat "$K/webhook.secret")" | sed 's/.* //')
    for i in 1 2 3 4 5 6; do
      s=$(curl -s -o /dev/null -w '%{http_code}' -X POST -H 'x-github-event: ping' -H "x-github-delivery: hookcheck-$TAG-$i-$(date +%s)" \
        -H "x-hub-signature-256: sha256=$sig" -H 'content-type: application/json' --data "$body" "$FRONT_URL/")
      echo "signed ping $i: $s"; [ "$s" = 202 ] && break; sleep 10
    done
    [ "$s" = 202 ] || echo "HOOK CHECK FAILED: the controller refuses the row's webhook secret"
  } 2>&1 | log
  if grep -q "HOOK CHECK FAILED" "$L"; then exit 2; fi ;;
repo | l004)
  spend >/dev/null || exit 3
  { spend; l004 "$STEP"; echo "$STEP exit $?"; spend || true; } 2>&1 | log ;;
teardown)
  { l004 teardown
    node tests/am3/hosted/kill.mjs uninstall
    day teardown | tail -6
    for i in 1 2 3 4 5 6; do
      left=$(containers "$P-" | jq -s '[.[] | select(.running > 0)]')
      [ "$left" = "[]" ] && break
      sleep 10
    done
    echo "agents container applications: $(containers "$P-" | jq -sc .)"
    rapi "$RB/containers/applications" | jq -r --arg p "$P-" '.result[]? | select(.name | startswith($p)) | .id' |
      while read -r id; do rapi "$RB/containers/applications/$id" -X DELETE | jq -c '{delete_app: .success, errors: [.errors[]?.code]}'; done
    if [ "$left" != "[]" ]; then echo "TEARDOWN FAILED: agents containers still running: $left"; fi
    for i in $(seq 1 12); do
      run=$(containers graft-runners- | jq -s '[.[] | select(.running > 0)]')
      [ "$run" = "[]" ] && break
      sleep 10
    done
    echo "graft-runners applications: $(containers graft-runners- | jq -sc .)"
    if [ "$run" != "[]" ]; then echo "TEARDOWN FAILED: graft-runners Sandboxes still running: $run"; fi
    python3 tests/fanout/teardown.py --sweep --shardkill 2>&1 | tail -2
    echo "agents apps after: $(rapi "$RB/containers/applications" | jq -c --arg p "$P-" '[.result[]? | select(.name | startswith($p)) | .name]')"
    echo "runner store secrets named $P-: $(rapi "$RB/secrets_store/stores/$RSTORE/secrets?per_page=100" | jq -c --arg p "$P-" '[.result[] | select(.name | startswith($p)) | .name]')"
  } 2>&1 | log
  if grep -q "TEARDOWN FAILED\|SWEEP FAILED" "$L"; then echo "FAIL: containers left running" >&2; exit 2; fi ;;
cost)
  spend 2>&1 | log ;;
*) echo "step: start state site1 site2 edge init register keys infra front probe-deploy taskbox controller install hook repo l004 teardown cost"; exit 2 ;;
esac
