#!/bin/bash
# LM-3's batched row on graft-m4a (`tests/am3/results/lm3-batch.md`): the site days, the edge day, the agents half
# (as the kill row's, `tests/am3/hosted/kill-row.sh` with ROW=kill under this row's tag), the lines (`batch.mjs`), a
# teardown that fails loudly on any running container, and the cost. One step per call, each logged to
# tests/am3/raw/batch-<tag>/<step>.log; the days log under tests/m4/raw/wp3/lm3batch/<tag>/.
#
#   bash tests/am3/hosted/batch-row.sh <tag> <step>
#
# Steps, in order:
#   state        the install's scripts and formats, the row's spend so far against the $2.50 stop (STOP printed past it)
#   site1        a site day from TREE (test-faults build done, BUILD_LOG ending "build exit 0"), every live GRAFTYARD_*
#                flag of site a kept, nothing added: 0x15.13's reader on every member
#   site2        the same with GRAFTYARD_AGENT_CI=1 added
#   edge         an edge day from TREE: the live FORMATS_ENABLED (all of it) plus 32.30 and 21.13, nothing else changed
#   agents       the kill row's agents half under this tag: init register keys infra taskbox controller front
#                probe-deploy install hook (one `kill-row.sh` call each; their logs stay under tests/am3/raw/kill-<tag>/)
#   repo | a59 | demote | assign | agentci | pulls      `batch.mjs <line>`
#   teardown     the repository deleted; the kill row's teardown (installs, agents resources, agents containers); the
#                install's shard sweep with --shardkill, failing loudly on any running shard container
#   cost         graft-m4a and the runner account since `site1`
#
# Needs `source tests/am1/lf13/env-m4a.sh`, CLOUDFLARE_* and CLOUDFLARE_RUNNER_*, CF_WORKERS_SUBDOMAIN and
# CF_RUNNER_WORKERS_SUBDOMAIN, ~/.graft-m4a, G (the graftyard binary), TREE, BUILD_LOG and /tmp/bf71web (the web build).
set -uo pipefail
TAG=${1:?tag}; STEP=${2:?step}
REPO=$(cd "$(dirname "$0")/../../.." && pwd); cd "$REPO"
O=$REPO/tests/am3/raw/batch-$TAG; mkdir -p "$O"
D=$REPO/tests/m4/raw/wp3/lm3batch/$TAG; mkdir -p "$D"
W=$HOME/.graft-m4a; K=$W/kill
WD=graft-m4a-edge.$CF_WORKERS_SUBDOMAIN.workers.dev
FRONT_URL=https://graft-agents-m4a-front.$CF_RUNNER_WORKERS_SUBDOMAIN.workers.dev
B="https://api.cloudflare.com/client/v4/accounts/$CLOUDFLARE_ACCOUNT_ID"
STOP=2.50
api() { curl -s -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" "$1" "${@:2}"; }
scrub() { sed -e "s/$CLOUDFLARE_ACCOUNT_ID/<install-account>/g" -e "s/$CLOUDFLARE_RUNNER_ACCOUNT_ID/<runner-account>/g" \
  -e "s/$CF_WORKERS_SUBDOMAIN/<sub>/g" -e "s/$CF_RUNNER_WORKERS_SUBDOMAIN/<runner-sub>/g" -e "s#$HOME/#~/#g" -e "s#$REPO/#<checkout>/#g"; }
N=$(ls "$O" | grep -c "^$STEP" || true)
L="$O/$STEP$( [ "$N" -gt 0 ] && echo "-$((N + 1))" ).log"
log() { { date -u +%FT%TZ; git rev-parse --short HEAD; cat; } | scrub >"$L"; cat "$L"; }
deployment() { api "$B/workers/scripts/$1/deployments" | jq -c --arg n "$1" '.result.deployments[0] | {script: $n, created_on, versions: [.versions[]? | "\(.version_id[0:8]) \(.percentage)"]}'; }
spend() {  # graft-m4a and the runner account's graft-agents-m4a- since site1
  local from; from=$(date -u -d @"$(cat "$O/cost.from")" +%FT%TZ)
  a=$(PREFIX=graft-m4a python3 tests/fanout/acctcost.py "$from" | head -1 | jq -r '.prefix_usd // 0')
  r=$(CLOUDFLARE_ACCOUNT_ID=$CLOUDFLARE_RUNNER_ACCOUNT_ID CLOUDFLARE_API_TOKEN=$CLOUDFLARE_RUNNER_API_TOKEN PREFIX=graft-agents-m4a \
    python3 tests/fanout/acctcost.py "$from" | head -1 | jq -r '.prefix_usd // 0')
  echo "row since $from: graft-m4a \$$a, runner graft-agents-m4a- \$$r, total \$$(python3 -c "print(round($a + $r, 3))") (stop \$$STOP)"
  python3 -c "import sys; sys.exit(0 if $a + $r < $STOP else 1)" || echo "STOP: the row passed its \$$STOP stop"
}
# BF-76's site day: every live GRAFTYARD_* flag of site a that BF-73's day does not set itself is passed on as it is.
site_day() {
  local sets='^GRAFTYARD_(HOST|ACTIVITY|AUDIT|APP_HOOKS|PR_TRIGGER|ACTIONS_CACHE|JOB_TOKENS|OIDC_HEAD_REF|OIDC_CLAIMS|PUSH_TRIGGER|SITES|INSTALL|CUSTOMER_KEY|GENESIS.*)$'
  local extra
  extra=$(api "$B/workers/scripts/graft-m4a-site-a/settings" |
    jq -r --arg s "$sets" '[.result.bindings[] | select(.type == "plain_text" and (.name | test("^GRAFTYARD_")) and (.name | test($s) | not)) | "--var \(.name)=\(.text)"] | join(" ")')
  echo "kept as live: ${extra:-none}; added: ${1:-none}"
  # bf71's days.sh reads its build check from ~/bf71/site-build.log: this row's build log, copied there.
  grep -q "build exit 0" "${BUILD_LOG:?}" || { echo "the site build did not finish"; exit 1; }
  mkdir -p "$HOME/bf71" && cp "$BUILD_LOG" "$HOME/bf71/site-build.log"
  ROW=lm3batch CLAIMS=1 EXTRA="$extra${1:+ $1}" HOSTV=$WD bash "$REPO/tests/m4/wp3/bf74/row.sh" "$TAG/$STEP" site
}

case $STEP in
state)
  { for n in graft-m4a-site-a graft-m4a-site-b graft-m4a-site-c graft-m4a-edge graft-m4a-ops; do
      echo "$(deployment $n) $(api "$B/workers/scripts/$n/settings" | jq -c '[.result.bindings[] | select(.type == "plain_text" and (.name | test("AGENT_CI|FORMATS_ENABLED"))) | "\(.name)=\(.text | split(",") | length) \(.text | .[0:40])"]')"
    done
    [ -s "$O/cost.from" ] && spend
  } 2>&1 | log ;;
site1) { [ -s "$O/cost.from" ] || date -u +%s >"$O/cost.from"; site_day; } 2>&1 | log ;;
site2) { site_day "--var GRAFTYARD_AGENT_CI=1"; } 2>&1 | log ;;
edge) { ROW=lm3batch ADDFMT=32.30,21.13 HOSTV=$WD bash "$REPO/tests/m4/wp3/bf74/row.sh" "$TAG" edge
        live=$(api "$B/workers/scripts/graft-m4a-edge/settings" | jq -r '.result.bindings[] | select(.name == "FORMATS_ENABLED") | .text')
        echo "FORMATS_ENABLED now $(echo "$live" | tr ',' '\n' | wc -l) formats; has 32.30: $(echo ",$live," | grep -c ',32.30,'), 21.13: $(echo ",$live," | grep -c ',21.13,')"
      } 2>&1 | log ;;
agents)
  # The harness pushes twice to this row's repository, then reports `Finished` (A59).
  for s in init register keys infra taskbox controller front probe-deploy install hook; do
    HARNESS_REPO=m3org/batch-$TAG HARNESS_PUSHES=2 bash tests/am3/hosted/kill-row.sh "$TAG" $s >/dev/null 2>&1; echo "kill-row $s exit $?"
  done 2>&1 | log ;;
repo | a59 | demote | assign | agentci | auto | pulls)
  { node tests/am3/hosted/batch.mjs "$STEP" "$O" "batch-$TAG" "$K/ctrl.token" "$FRONT_URL"; echo "$STEP exit $?"; } 2>&1 | log ;;
teardown)
  { node tests/am3/hosted/batch.mjs delete "$O" "batch-$TAG"
    bash tests/am3/hosted/kill-row.sh "${AGENTS_TAG:-$TAG}" teardown 2>&1 | tail -8; echo "kill-row teardown exit ${PIPESTATUS[0]}"
    python3 tests/fanout/teardown.py --sweep --shardkill 2>&1 | tail -4; echo "sweep exit ${PIPESTATUS[0]}"
  } 2>&1 | log
  if grep -q "TEARDOWN FAILED\|SWEEP FAILED\|kill-row teardown exit [^0]\|sweep exit [^0]" "$L"; then echo "FAIL: containers left running" >&2; exit 2; fi ;;
cost) spend 2>&1 | log ;;
*) echo "step: state site1 site2 edge agents repo a59 demote assign agentci pulls teardown cost"; exit 2 ;;
esac
