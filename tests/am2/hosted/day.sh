#!/bin/bash
# AM2-WP1's hosted day on graft-m3f, after the site day and edge day (`tests/am2/results/am2-wp1.md`, "Hosted plan"),
# from this checkout's HEAD. One step per call, each logged to tests/am2/raw/day-<tag>-<step>.log; a step whose log
# exists is not run again. Needs `source tests/am1/lf13/env-m3f.sh`, TOOLS_TOKEN_FILE, CLOUDFLARE_*, and
# CF_WORKERS_SUBDOMAIN.
#   bash tests/am2/hosted/day.sh <tag> init|claims|register|deploy|smoke|l008|clients|a43|teardown [insights id]
# - init      `graftyard apps init` step 1 into ~/.graft-m3f/apps (four secrets, `graft-m3f-` prefix, the shared store)
# - claims    the two ClaimSlug requests; graftyard-mcp answers Taken (App 327680), graftyard-insights a new id
# - register  step 2 with --app-ids 327680,<insights id>, then the seven commands in file order
# - deploy    graft-m3f-mcp-aux (the CIMD host, tests/m0/src/wp15/aux.ts) and graft-m3f-mcp
# - smoke     metadata 200, the door 401 with resource_metadata
# - l008      tests/am2/hosted/l008.mjs: L0-08's cells and LF-6 through the edge
# - clients   the four SDK clients, one session each
# - a43       the four SDK clients with the first /token answer dropped
# - teardown  delete graft-m3f-mcp, graft-m3f-mcp-aux, and the four secrets (store ids in the log)
set -uo pipefail
cd "$(dirname "$0")/../../.."
tag=${1:?tag}; step=${2:?step}
STORE=bfaa0c1573de4775b1b92a12fb89acb4
APPS=$HOME/.graft-m3f/apps
MCP_NAME=graft-m3f-mcp AUX_NAME=graft-m3f-mcp-aux
export GRAFT_MCP=https://$MCP_NAME.$CF_WORKERS_SUBDOMAIN.workers.dev GRAFT_AUX=https://$AUX_NAME.$CF_WORKERS_SUBDOMAIN.workers.dev
L=tests/am2/raw/day-$tag-$step.log
mkdir -p tests/am2/raw
[ -e "$L" ] && { echo "$L exists: one run per step and tag"; exit 1; }
wr() { npx --prefix tests/m0 wrangler "$@"; }
{
  date -u +%FT%TZ; git rev-parse --short HEAD
  case $step in
    init) cargo run -q -p graftyard -- --config $HOME/.graft-m3f/install.toml apps init --host "${GRAFT_EDGE#https://}" \
            --store $STORE --secret-prefix graft-m3f- --mcp-origin "$GRAFT_MCP" --out "$APPS" ;;
    claims) node tests/am2/hosted/apps.mjs claims "$APPS" ;;
    register) cargo run -q -p graftyard -- apps init --app-ids 327680,${3:?insights id} --out "$APPS" &&
              node tests/am2/hosted/apps.mjs register "$APPS" ;;
    deploy) wr deploy -c tests/am2/hosted/aux.jsonc &&
            node workers/mcp/deploy.mjs $MCP_NAME "$APPS/apps.json" --edge graft-m3f-edge --store $STORE --app-id 327680 ;;
    smoke) curl -s -o /dev/null -w 'prm %{http_code}\n' "$GRAFT_MCP/.well-known/oauth-protected-resource"
           curl -s -o /dev/null -w 'asm %{http_code}\n' "$GRAFT_MCP/.well-known/oauth-authorization-server"
           curl -s -D - -o /dev/null -X POST "$GRAFT_MCP/" | grep -iE '^(HTTP|www-authenticate)'
           curl -s -o /dev/null -w 'cimd %{http_code}\n' "$GRAFT_AUX/cimd/ts-v2.json" ;;
    l008) L008_REGISTER=$APPS node tests/am2/hosted/l008.mjs "$tag" ;;
    clients|a43)
      d=; [ $step = a43 ] && d=--drop-token
      for v in v2 v1; do node tests/am2/hosted/ts-client.mjs $v 30 $d; done
      (cd tests/am2/hosted/goclient && go run . -calls 30 $d)
      ~/.cache/am2-venv/bin/python tests/am2/hosted/py_client.py 30 $d ;;
    teardown)
      node workers/mcp/deploy.mjs $MCP_NAME --delete; wr delete --name $AUX_NAME --force
      for n in MCP_APP_KEY_A INSIGHTS_APP_KEY_A MCP_CLIENT_SECRET_A MCP_SEAL_KEY_A; do
        id=$(curl -s -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" \
          "https://api.cloudflare.com/client/v4/accounts/$CLOUDFLARE_ACCOUNT_ID/secrets_store/stores/$STORE/secrets?search=graft-m3f-$n" \
          | jq -r --arg n "graft-m3f-$n" '.result[] | select(.name == $n) | .id')
        [ -n "$id" ] && curl -s -X DELETE -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" \
          "https://api.cloudflare.com/client/v4/accounts/$CLOUDFLARE_ACCOUNT_ID/secrets_store/stores/$STORE/secrets/$id" | jq -c '{n: "'"$n"'", success}'
      done ;;
    *) echo "unknown step $step"; exit 2 ;;
  esac
  echo "step exit $?"; date -u +%FT%TZ; echo DONE
} > "$L" 2>&1
tail -3 "$L"
