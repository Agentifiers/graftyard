#!/bin/bash
# BF-72's site day and edge day on graft-m4a (tests/m4/wp3/bf72/plan.md; only on the lead's GO, after B0-07 r2 and
# AM2's kill row). Both keep everything BF-71's days set (tests/m4/wp3/bf71/days.sh) and add only what BF-72 needs:
# the site day ships 0x15.9's reader (`RunPlanned`, `JobOutputs`, `FinishWith`) and sets GRAFTYARD_REUSABLE=1; the edge
# day adds 21.9 (0x15.9) to the live FORMATS_ENABLED, so dispatch writes a plan, and keeps RUN_SERVICE_PUBLIC,
# GRAFTYARD_CACHE_LIMIT and GRAFTYARD_ARTIFACTS.
#
#   bash tests/m4/wp3/bf72/days.sh <worktree at the BF-72 head> site|edge|check
#
# `site` needs `worker-build --release --features test-faults` in the worktree's workers/site first (log in
# $HOME/bf72/site-build.log ending "build exit 0"); `edge` needs a web build in /tmp/bf72web.
set -uo pipefail
TREE=${1:?worktree}; PH=${2:?site|edge|check}
REPO=$(cd "$(dirname "$0")/../../../.." && pwd)
O=$REPO/tests/m4/raw/wp3/bf72; mkdir -p "$O"
W=$HOME/.graft-m4a
STORE=bfaa0c1573de4775b1b92a12fb89acb4
IMG=registry.cloudflare.com/$CLOUDFLARE_ACCOUNT_ID/graft-m1-site-gitshard-git@sha256:e55ab2ad178a4d2b639bf53a3c4ca08694024e344343b2e251821368b1d21d41
B="https://api.cloudflare.com/client/v4/accounts/$CLOUDFLARE_ACCOUNT_ID"
api() { curl -s -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" "$B$1" "${@:2}"; }
scrub() { sed -e "s/$CLOUDFLARE_ACCOUNT_ID/<install-account>/g" -e "s/$CF_WORKERS_SUBDOMAIN/<sub>/g" -e "s#$HOME/#~/#g"; }
log() { { date -u +%FT%TZ; git -C "$TREE" rev-parse --short HEAD; cat; } | scrub >"$O/day-$PH.log"; cat "$O/day-$PH.log"; }
deployment() { api "/workers/scripts/$1/deployments" | jq -c --arg n "$1" '.result.deployments[0] | {script: $n, created_on, versions: [.versions[]? | "\(.version_id[0:8]) \(.percentage)"]}'; }
vars() { api "/workers/scripts/$1/settings" | jq -c '[.result.bindings[] | select(.type == "plain_text" and (.name | test("^(GRAFTYARD_(JOB|ACTIONS|PR|APP|ACTIVITY|AUDIT|HOST|CACHE|ARTIFACTS|REUSABLE)|FORMATS_ENABLED|RUN_SERVICE)"))) | "\(.name)=\(.text)"] | sort'; }
SITES="graft-m4a-site-a graft-m4a-site-b graft-m4a-site-c"

case $PH in
site)
  { grep -q "build exit 0" "$HOME/bf72/site-build.log" || { echo "the site build did not finish"; exit 1; }
    for n in $SITES; do echo "before $(deployment $n)"; done
    (cd "$TREE" && node workers/site/deploy.mjs "$W/sites.json" --anchor "$W/install.toml" --image "$IMG" --image-prev "$IMG" \
      --vars "$W/site-vars.json" --skip-build --ops graft-m4a-ops \
      --var GRAFTYARD_HOST=graft-m4a-edge.$CF_WORKERS_SUBDOMAIN.workers.dev --var GRAFTYARD_ACTIVITY=1 --var GRAFTYARD_AUDIT=1 \
      --var GRAFTYARD_APP_HOOKS=1 --var GRAFTYARD_PR_TRIGGER=1 --var GRAFTYARD_ACTIONS_CACHE=1 --var GRAFTYARD_JOB_TOKENS=1 --var GRAFTYARD_REUSABLE=1 \
      --audit-stream 2a0357004afd44d9a74cf410dd2c715a --run-key $STORE/graft-m4a-run-service-key --oidc $STORE \
      >"$O/site-deploy.raw" 2>&1); echo "deploy exit $?"
    grep -E "Current Version|rror" "$O/site-deploy.raw" | tail -6
    for n in $SITES; do
      api "/workers/scripts/$n/schedules" -X PUT -H 'content-type: application/json' --data '[{"cron":"0 0 1 1 *"}]' >/dev/null
      sleep 2
      api "/workers/scripts/$n/schedules" -X PUT -H 'content-type: application/json' --data '[]' | jq -c --arg n $n '{($n): .result.schedules}'
      echo "after $(deployment $n) $(vars $n)"
    done
  } 2>&1 | log ;;
edge)
  { [ -d /tmp/bf72web ] || { echo "no web build in /tmp/bf72web"; exit 1; }
    before=$(api "/workers/scripts/graft-m4a-edge/settings" | jq -c '[.result.bindings[] | select(.type == "plain_text") | {(.name): .text}] | add')
    echo "$before" | jq -S . >"$O/edge-before.json"
    fmt=$(echo "$before" | jq -r .FORMATS_ENABLED); rsp=$(echo "$before" | jq -r .RUN_SERVICE_PUBLIC)
    lim=$(echo "$before" | jq -r .GRAFTYARD_CACHE_LIMIT)
    new=$fmt; echo ",$new," | grep -q ",21.9," || new=$new,21.9
    echo "FORMATS_ENABLED adds: ${new#"$fmt"}"
    echo "before $(deployment graft-m4a-edge)"
    (cd "$TREE" && export PATH=$HOME/.cargo/bin:$PATH && unset CARGO_TARGET_DIR && CARGO_INCREMENTAL=0 \
      node tests/m1/integ/deploy-edge.mjs "$W/sites.json" graft-m4a-edge --test-faults --anchor "$W/install.toml" --ops graft-m4a-ops \
        --web /tmp/bf72web --var "FORMATS_ENABLED=$new" --var "RUN_SERVICE_PUBLIC=$rsp" --var "GRAFTYARD_CACHE_LIMIT=$lim" \
        --var GRAFTYARD_ARTIFACTS=1 >"$O/edge-deploy.raw" 2>&1); echo "deploy exit $?"
    grep -E "Current Version|rror" "$O/edge-deploy.raw" | tail -4
    api "/workers/scripts/graft-m4a-edge/settings" | jq -c '[.result.bindings[] | select(.type == "plain_text") | {(.name): .text}] | add' | jq -S . >"$O/edge-after.json"
    echo "binding diff (plain text; only FORMATS_ENABLED may change):"; diff "$O/edge-before.json" "$O/edge-after.json" | grep '^[<>]' | grep -v GRAFTYARD_SITES | cut -c1-80
    echo "after $(deployment graft-m4a-edge)"
    (cd "$TREE" && python3 tests/fanout/edgever.py "$O/edge-deploy.raw" graft-m4a-edge); echo "edgever exit $?"
  } 2>&1 | log ;;
check)
  { for n in $SITES graft-m4a-edge graft-m4a-ops; do echo "$(deployment $n) $(vars $n)"; done
    for n in $SITES graft-m4a-ops; do api "/workers/scripts/$n/schedules" | jq -c --arg n $n '{($n): [.result.schedules[]?.cron]}'; done
  } 2>&1 | log ;;
*) echo "phase: site edge check"; exit 2 ;;
esac
