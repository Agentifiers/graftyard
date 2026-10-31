#!/usr/bin/env bash
# Deploys the M0 test bed: three site scripts with cyclic cross-script DO bindings, then the edge.
# Needs CLOUDFLARE_API_TOKEN and CLOUDFLARE_ACCOUNT_ID. Writes the edge's bearer token to .m0-token.
#
# Each site is first deployed binding only its own classes, so that a class added in this deploy exists before another
# script binds it with script_name. Do not run this during B0-15: any new version of a site script resets its members.
set -euo pipefail
cd "$(dirname "$0")"

node gen-config.ts own
for k in a b c; do npx wrangler deploy -c ".gen/site-$k.jsonc"; done

node gen-config.ts cycle
for k in a b c; do npx wrangler deploy -c ".gen/site-$k.jsonc"; done
npx wrangler deploy -c .gen/edge.jsonc

if [ ! -s .m0-token ]; then
  openssl rand -hex 32 > .m0-token
  npx wrangler secret put M0_TOKEN -c .gen/edge.jsonc < .m0-token
fi
