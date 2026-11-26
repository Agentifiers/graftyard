#!/bin/bash
# BF-70 r2's site day and edge day on graft-m4a (the lead's ruling, 2026-11-25): job tokens on, so a run's declared
# `permissions:` reach the shard (F127). Both keep everything LM-1's site day 3 and edge day d3d76118 set and add only
# what slice B needs (`decisions/DECISIONS.md`, `gh-token`, "Order").
#
#   bash tests/m4/wp3/oidc/days.sh <worktree at main> site|edge|check <tag>
#
#   site   the site day from the worktree's build (`worker-build --release --features test-faults` in workers/site
#          first): LM-1's site day 3 flags (GRAFTYARD_HOST, ACTIVITY, AUDIT, APP_HOOKS, PR_TRIGGER,
#          ACTIONS_CACHE, the audit stream, the run key, `--oidc`) plus GRAFTYARD_JOB_TOKENS=1. Then each site's cron
#          reset to [] and the reretire check.
#   edge   the edge day: the live version's FORMATS_ENABLED and RUN_SERVICE_PUBLIC read from its bindings, plus 21.5
#          (0x15.5) and 18.14 (0x12.14); GRAFTYARD_CACHE_LIMIT kept; test-faults as before. Then the binding diff
#          (only FORMATS_ENABLED may change) and edgever.
#   check  the versions, the bindings that matter, and whether the Release group must enable the formats (it records
#          the group's enabled set; F124: test installs enable Actions formats by the edge var and the sites' flag).
#
# Needs CLOUDFLARE_*, CF_WORKERS_SUBDOMAIN, ~/.graft-m4a, and for `edge` a web build in /tmp/bf70web
# (`cd web && VITE_MOCK_API=false npx vite build`, then dist/client copied there).
set -uo pipefail
TREE=${1:?worktree}; PH=${2:?site|edge|check}; TAG=${3:?tag}
REPO=$(cd "$(dirname "$0")/../../../.." && pwd)
O=$REPO/tests/m4/raw/wp3/oidc-hosted/$TAG; mkdir -p "$O"
W=$HOME/.graft-m4a
STORE=${CF_SECRETS_STORE_ID:?}
IMG=registry.cloudflare.com/$CLOUDFLARE_ACCOUNT_ID/graft-m1-site-gitshard-git@sha256:e55ab2ad178a4d2b639bf53a3c4ca08694024e344343b2e251821368b1d21d41
B="https://api.cloudflare.com/client/v4/accounts/$CLOUDFLARE_ACCOUNT_ID"
api() { curl -s -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" "$B$1" "${@:2}"; }
scrub() { sed -e "s/$CLOUDFLARE_ACCOUNT_ID/<install-account>/g" -e "s/$CF_SECRETS_STORE_ID/<store>/g" -e "s/$CF_WORKERS_SUBDOMAIN/<sub>/g" -e "s#$HOME/#~/#g"; }
log() { { date -u +%FT%TZ; git -C "$TREE" rev-parse --short HEAD; cat; } | scrub >"$O/day-$PH.log"; cat "$O/day-$PH.log"; }
deployment() { api "/workers/scripts/$1/deployments" | jq -c --arg n "$1" '.result.deployments[0] | {script: $n, created_on, versions: [.versions[]? | "\(.version_id[0:8]) \(.percentage)"]}'; }
vars() { api "/workers/scripts/$1/settings" | jq -c '[.result.bindings[] | select(.type == "plain_text" and (.name | test("^(GRAFTYARD_(JOB|ACTIONS|PR|APP|ACTIVITY|AUDIT|HOST|CACHE)|FORMATS_ENABLED|RUN_SERVICE)"))) | "\(.name)=\(.text)"] | sort'; }
SITES="graft-m4a-site-a graft-m4a-site-b graft-m4a-site-c"

case $PH in
site)
  { grep -q "build exit 0" "$HOME/bf70/site-build.log" || { echo "the site build did not finish"; exit 1; }
    for n in $SITES; do echo "before $(deployment $n)"; done
    (cd "$TREE" && node workers/site/deploy.mjs "$W/sites.json" --anchor "$W/install.toml" --image "$IMG" --image-prev "$IMG" \
      --vars "$W/site-vars.json" --skip-build --ops graft-m4a-ops \
      --var GRAFTYARD_HOST=graft-m4a-edge.$CF_WORKERS_SUBDOMAIN.workers.dev --var GRAFTYARD_ACTIVITY=1 --var GRAFTYARD_AUDIT=1 \
      --var GRAFTYARD_APP_HOOKS=1 --var GRAFTYARD_PR_TRIGGER=1 --var GRAFTYARD_ACTIONS_CACHE=1 --var GRAFTYARD_JOB_TOKENS=1 \
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
  { [ -d /tmp/bf70web ] || { echo "no web build in /tmp/bf70web"; exit 1; }
    before=$(api "/workers/scripts/graft-m4a-edge/settings" | jq -c '[.result.bindings[] | select(.type == "plain_text") | {(.name): .text}] | add')
    echo "$before" | jq -S . >"$O/edge-before.json"
    fmt=$(echo "$before" | jq -r .FORMATS_ENABLED); rsp=$(echo "$before" | jq -r .RUN_SERVICE_PUBLIC)
    lim=$(echo "$before" | jq -r .GRAFTYARD_CACHE_LIMIT)
    new=$fmt
    for f in 21.5 18.14; do echo ",$new," | grep -q ",$f," || new=$new,$f; done
    echo "FORMATS_ENABLED adds: ${new#"$fmt"}"
    echo "before $(deployment graft-m4a-edge)"
    (cd "$TREE" && export PATH=$HOME/.cargo/bin:$PATH && unset CARGO_TARGET_DIR && CARGO_INCREMENTAL=0 \
      node tests/m1/integ/deploy-edge.mjs "$W/sites.json" graft-m4a-edge --test-faults --anchor "$W/install.toml" --ops graft-m4a-ops \
        --web /tmp/bf70web --var "FORMATS_ENABLED=$new" --var "RUN_SERVICE_PUBLIC=$rsp" --var "GRAFTYARD_CACHE_LIMIT=$lim" \
        >"$O/edge-deploy.raw" 2>&1); echo "deploy exit $?"
    grep -E "Current Version|rror" "$O/edge-deploy.raw" | tail -4
    api "/workers/scripts/graft-m4a-edge/settings" | jq -c '[.result.bindings[] | select(.type == "plain_text") | {(.name): .text}] | add' | jq -S . >"$O/edge-after.json"
    echo "binding diff (plain text):"; diff "$O/edge-before.json" "$O/edge-after.json" | grep '^[<>]' | grep -v GRAFTYARD_SITES | cut -c1-80
    echo "after $(deployment graft-m4a-edge)"
    (cd "$TREE" && python3 tests/fanout/edgever.py "$O/edge-deploy.raw" graft-m4a-edge); echo "edgever exit $?"
  } 2>&1 | log ;;
check)
  { for n in $SITES graft-m4a-edge graft-m4a-ops; do echo "$(deployment $n) $(vars $n)"; done
    for n in $SITES graft-m4a-ops; do api "/workers/scripts/$n/schedules" | jq -c --arg n $n '{($n): [.result.schedules[]?.cron]}'; done
  } 2>&1 | log ;;
*) echo "phase: site edge check"; exit 2 ;;
esac
