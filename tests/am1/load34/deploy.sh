#!/bin/bash
# Deploys Load34 (tests/am1/load34/load34.mjs) as `graft-m1-load`: the `Load34` client class, the three graft-m1 sites'
# `Member` classes by script_name (feed polls go to members, as the tools edge reaches them), and the R2 bucket
# `graft-m1-load` for staged tip:move packs and the clients' samples. A test Worker, not a site script: deploying it
# resets no member. Writes its bearer token to $M3_STATE/load34.token (mode 600) the first time, never prints it.
#   bash tests/am1/load34/deploy.sh                    # needs CLOUDFLARE_API_TOKEN and CLOUDFLARE_ACCOUNT_ID
set -euo pipefail
cd "$(dirname "$0")"
W=${WRANGLER:-../../m0/node_modules/.bin/wrangler}
TOK=${LOAD34_TOKEN_FILE:-${M3_STATE:-$HOME/.graft-m3h}/load34.token}
mkdir -p .gen
node config.mjs > .gen/graft-m1-load.jsonc
$W r2 bucket list 2>/dev/null | grep -q '^name: *graft-m1-load$' || $W r2 bucket create graft-m1-load
$W deploy -c .gen/graft-m1-load.jsonc
if [ ! -s "$TOK" ]; then
  (umask 077; openssl rand -hex 32 > "$TOK")
  $W secret put TOKEN -c .gen/graft-m1-load.jsonc < "$TOK"
fi
