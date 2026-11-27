#!/bin/bash
# LM-3's in-repo batched row on graft-m4a (`tests/am3/results/lm3-inrepo.md`): one site day, one edge day adding 32.32
# and 32.33, the agents half (the kill row's, `tests/am3/hosted/kill-row.sh`, under this row's tag), the lines
# (`inrepo.mjs`), a teardown that fails loudly on any running container, and the cost. One step per call, each logged
# to tests/am3/raw/inrepo-<tag>/<step>.log; the days log under tests/m4/raw/wp3/lm3inrepo/<tag>/.
#
#   bash tests/am3/hosted/inrepo-row.sh <tag> <step>
#
# Steps, in order:
#   state      the install's scripts and formats, the row's spend so far against the $3.00 stop (STOP printed past it)
#   site       a site day from TREE (test-faults build done, BUILD_LOG ending "build exit 0"), every live GRAFTYARD_*
#              flag of site a kept, nothing added: the readers of 32.32 and 32.33 on every member
#   edge       an edge day from TREE: the live FORMATS_ENABLED (all of it) plus 32.32 and 32.33, nothing else changed;
#              WEB is the web build's client directory, copied to /tmp/bf71web (the old copy kept as .prev)
#   agents     the kill row's agents half under this tag, with the harness held while `hold/<task>` exists:
#              init register keys infra taskbox controller front FRONTB-taskbox controller probe-deploy install hook
#   repo | reader | l012 | b036 | lf9 | b037 | pause | a3 | l014 | l004      `inrepo.mjs <line>`
#   teardown   the repository deleted, m3-reader suspended and its PAT revoked, the policy reset; the kill row's
#              teardown (installs, every graft-agents-m4a- resource, agents containers, failing loudly); graft-runners'
#              Sandboxes listed, failing loudly while any runs; the install's shard sweep with --shardkill
#   cost       graft-m4a and the runner account since `site`
#
# Needs `source tests/am1/lf13/env-m4a.sh`, CF_SECRETS_STORE_ID, CLOUDFLARE_* and CLOUDFLARE_RUNNER_*,
# CF_WORKERS_SUBDOMAIN and CF_RUNNER_WORKERS_SUBDOMAIN, ~/.graft-m4a, G (the graftyard binary), RELVIEW, TREE,
# BUILD_LOG and WEB.
set -uo pipefail
: "${CF_SECRETS_STORE_ID:?export CF_SECRETS_STORE_ID, as M4-WP4 requires}"
TAG=${1:?tag}; STEP=${2:?step}
REPO=$(cd "$(dirname "$0")/../../.." && pwd); cd "$REPO"
O=$REPO/tests/am3/raw/inrepo-$TAG; mkdir -p "$O"
D=$REPO/tests/m4/raw/wp3/lm3inrepo/$TAG; mkdir -p "$D"
W=$HOME/.graft-m4a; K=$W/kill
WD=graft-m4a-edge.$CF_WORKERS_SUBDOMAIN.workers.dev
FRONT_URL=https://graft-agents-m4a-front.$CF_RUNNER_WORKERS_SUBDOMAIN.workers.dev
PROBE_URL=https://graft-agents-m4a-probe.$CF_RUNNER_WORKERS_SUBDOMAIN.workers.dev
B="https://api.cloudflare.com/client/v4/accounts/$CLOUDFLARE_ACCOUNT_ID"
RB="https://api.cloudflare.com/client/v4/accounts/$CLOUDFLARE_RUNNER_ACCOUNT_ID"
STOP=3.00
AP=graft-agents-m4a
api() { curl -s -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" "$1" "${@:2}"; }
rapi() { curl -s -H "Authorization: Bearer $CLOUDFLARE_RUNNER_API_TOKEN" "$1" "${@:2}"; }
scrub() { sed -e "s/$CLOUDFLARE_ACCOUNT_ID/<install-account>/g" -e "s/$CLOUDFLARE_RUNNER_ACCOUNT_ID/<runner-account>/g" \
  -e "s/$CF_WORKERS_SUBDOMAIN/<sub>/g" -e "s/$CF_RUNNER_WORKERS_SUBDOMAIN/<runner-sub>/g" -e "s/$CF_SECRETS_STORE_ID/<install-store>/g" \
  -e "s#$HOME/#~/#g" -e "s#$REPO/#<checkout>/#g"; }
N=$(ls "$O" | grep -c "^$STEP" || true)
L="$O/$STEP$( [ "$N" -gt 0 ] && echo "-$((N + 1))" ).log"
log() { { date -u +%FT%TZ; git rev-parse --short HEAD; cat; } | scrub >"$L"; cat "$L"; }
deployment() { api "$B/workers/scripts/$1/deployments" | jq -c --arg n "$1" '.result.deployments[0] | {script: $n, created_on, versions: [.versions[]? | "\(.version_id[0:8]) \(.percentage)"]}'; }
spend() {  # graft-m4a and the runner account (graft-agents-m4a- and graft-runners) since `site`
  local from; from=$(date -u -d @"$(cat "$O/cost.from")" +%FT%TZ)
  a=$(PREFIX=graft-m4a python3 tests/fanout/acctcost.py "$from" | head -1 | jq -r '.prefix_usd // 0')
  r=$(CLOUDFLARE_ACCOUNT_ID=$CLOUDFLARE_RUNNER_ACCOUNT_ID CLOUDFLARE_API_TOKEN=$CLOUDFLARE_RUNNER_API_TOKEN PREFIX=graft-agents-m4a \
    python3 tests/fanout/acctcost.py "$from" | head -1 | jq -r '.prefix_usd // 0')
  h=$(CLOUDFLARE_ACCOUNT_ID=$CLOUDFLARE_RUNNER_ACCOUNT_ID CLOUDFLARE_API_TOKEN=$CLOUDFLARE_RUNNER_API_TOKEN PREFIX=graft-runners \
    python3 tests/fanout/acctcost.py "$from" | head -1 | jq -r '.prefix_usd // 0')
  echo "row since $from: graft-m4a \$$a, graft-agents-m4a- \$$r, graft-runners \$$h, total \$$(python3 -c "print(round($a + $r + $h, 3))") (stop \$$STOP)"
  python3 -c "import sys; sys.exit(0 if $a + $r + $h < $STOP else 1)" || echo "STOP: the row passed its \$$STOP stop"
}
# graft-runners' Sandboxes that run, as one number (L0-04's runner; teardown fails while any runs).
runners_running() {
  rapi "$RB/containers/applications" | jq -r '.result[]? | select(.name | startswith("graft-runners-")) | .id' |
    while read -r id; do rapi "$RB/containers/applications/$id/instances?per_page=1000" |
      jq '[.result.instances[]? // .result[]? | select(.status.state == "running" or .state == "running")] | length'; done |
    awk '{s += $1} END {print s + 0}'
}
# BF-76's site day: every live GRAFTYARD_* flag of site a that BF-73's day does not set itself is passed on as it is.
site_day() {
  local sets='^GRAFTYARD_(HOST|ACTIVITY|AUDIT|APP_HOOKS|PR_TRIGGER|ACTIONS_CACHE|JOB_TOKENS|OIDC_HEAD_REF|OIDC_CLAIMS|PUSH_TRIGGER|SITES|INSTALL|CUSTOMER_KEY|GENESIS.*)$'
  local extra
  extra=$(api "$B/workers/scripts/graft-m4a-site-a/settings" |
    jq -r --arg s "$sets" '[.result.bindings[] | select(.type == "plain_text" and (.name | test("^GRAFTYARD_")) and (.name | test($s) | not)) | "--var \(.name)=\(.text)"] | join(" ")')
  echo "kept as live: ${extra:-none}; added: none"
  grep -q "build exit 0" "${BUILD_LOG:?}" || { echo "the site build did not finish"; exit 1; }
  mkdir -p "$HOME/bf71" && cp "$BUILD_LOG" "$HOME/bf71/site-build.log"
  ROW=lm3inrepo CLAIMS=1 EXTRA="$extra" HOSTV=$WD bash "$REPO/tests/m4/wp3/bf74/row.sh" "$TAG/site" site
}

case $STEP in
state)
  { for n in graft-m4a-site-a graft-m4a-site-b graft-m4a-site-c graft-m4a-edge graft-m4a-ops; do
      echo "$(deployment $n) $(api "$B/workers/scripts/$n/settings" | jq -c '[.result.bindings[] | select(.type == "plain_text" and (.name | test("FORMATS_ENABLED"))) | "\(.name)=\(.text | split(",") | length) has 32.32: \(.text | split(",") | index("32.32") != null), 32.33: \(.text | split(",") | index("32.33") != null)"]')"
    done
    [ -s "$O/cost.from" ] && spend
  } 2>&1 | log ;;
site) { [ -s "$O/cost.from" ] || date -u +%s >"$O/cost.from"; spend; site_day; } 2>&1 | log ;;
edge)
  { spend
    [ -d "${WEB:?WEB, the web build}" ] || { echo "no web build at $WEB"; exit 1; }
    echo "edge from TREE $(git -C "${TREE:?}" rev-parse --short HEAD)"
    [ -d /tmp/bf71web ] && { rm -rf /tmp/bf71web.prev; mv /tmp/bf71web /tmp/bf71web.prev; }
    cp -r "$WEB" /tmp/bf71web
    ROW=lm3inrepo ADDFMT=32.32,32.33 HOSTV=$WD bash "$REPO/tests/m4/wp3/bf74/row.sh" "$TAG" edge
    live=$(api "$B/workers/scripts/graft-m4a-edge/settings" | jq -r '.result.bindings[] | select(.name == "FORMATS_ENABLED") | .text')
    echo "FORMATS_ENABLED now $(echo "$live" | tr ',' '\n' | wc -l) formats; has 32.32: $(echo ",$live," | grep -c ',32.32,'), 32.33: $(echo ",$live," | grep -c ',32.33,')"
  } 2>&1 | log ;;
agents)
  # The harness pushes twice to this row's repository, then holds while `hold/<task>` exists, then reports `Finished`.
  { spend
    for s in init register keys infra taskbox controller front FRONTB probe-deploy install hook; do
      if [ $s = FRONTB ]; then
        HARNESS_REPO=m3org/inrepo-$TAG HARNESS_PUSHES=2 FRONTB=1 bash tests/am3/hosted/kill-row.sh "$TAG" taskbox >/dev/null 2>&1
        echo "kill-row FRONTB taskbox exit $?"; s=controller
      fi
      HARNESS_REPO=m3org/inrepo-$TAG HARNESS_PUSHES=2 bash tests/am3/hosted/kill-row.sh "$TAG" $s >/dev/null 2>&1; e=$?
      echo "kill-row $s exit $e"; [ $e -eq 0 ] || break
    done
  } 2>&1 | log ;;
repo | reader | l012 | b036 | lf9 | b037 | pause | a3 | l014 | l004)
  { spend | tee /dev/stderr | grep -q "^STOP" && { echo "not run: past the stop"; exit 3; }
    PROBE_URL=$PROBE_URL node tests/am3/hosted/inrepo.mjs "$STEP" "$O" "inrepo-$TAG" "$K/ctrl.token" "$FRONT_URL"; echo "$STEP exit $?"
  } 2>&1 | log ;;
teardown)
  # A cancelled task's box keeps its alarm (F145) and its storage, so the row deletes them with the controller's
  # `TaskBox` namespace: before, the namespace's objects and 60 s of the controller's alarm lines; after, no namespace
  # named for the controller, so no box alarm can run (TEARDOWN FAILED otherwise).
  { dons() { rapi "$RB/workers/durable_objects/namespaces?per_page=100" | jq -c --arg p "$AP-" '[.result[]? | select(.script // "" | startswith($p)) | {id, name, class}]'; }
    ns=$(dons); echo "box namespaces before: $ns"
    for id in $(echo "$ns" | jq -r '.[].id'); do
      echo "  $id objects: $(rapi "$RB/workers/durable_objects/namespaces/$id/objects?limit=1000" | jq -c '.result | length')"
    done
    CLOUDFLARE_ACCOUNT_ID=$CLOUDFLARE_RUNNER_ACCOUNT_ID CLOUDFLARE_API_TOKEN=$CLOUDFLARE_RUNNER_API_TOKEN \
      timeout 60 npx --prefix tests/m0 wrangler tail $AP-controller --format json >"$O/teardown-ctail.json" 2>/dev/null
    echo "box alarm lines in 60 s before: $(grep -c 'graft-controller: alarm\|alarm:' "$O/teardown-ctail.json")"
    PROBE_URL=$PROBE_URL node tests/am3/hosted/inrepo.mjs teardown "$O" "inrepo-$TAG"; echo "inrepo teardown exit $?"
    bash tests/am3/hosted/kill-row.sh "$TAG" teardown 2>&1 | tail -8; echo "kill-row teardown exit ${PIPESTATUS[0]}"
    left=$(dons); echo "box namespaces after: $left"
    [ "$left" = "[]" ] || echo "TEARDOWN FAILED: TaskBox storage and alarms remain: $left"
    for i in 1 2 3 4 5 6 7 8 9 10 11 12; do n=$(runners_running); [ "$n" = 0 ] && break; sleep 10; done
    echo "graft-runners Sandboxes running: $n"
    [ "$n" = 0 ] || echo "TEARDOWN FAILED: graft-runners Sandboxes still running: $n"
    python3 tests/fanout/teardown.py --sweep --shardkill 2>&1 | tail -4; echo "sweep exit ${PIPESTATUS[0]}"
  } 2>&1 | log
  if grep -q "TEARDOWN FAILED\|SWEEP FAILED\|teardown exit [^0]\|sweep exit [^0]" "$L"; then echo "FAIL: containers left running or teardown failed" >&2; exit 2; fi ;;
cost) spend 2>&1 | log ;;
*) echo "step: state site edge agents repo reader l012 b036 lf9 b037 pause a3 l014 l004 teardown cost"; exit 2 ;;
esac
