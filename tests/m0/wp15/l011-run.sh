#!/usr/bin/env bash
# M0-WP15 L0-11 full run: reset and seed both deployments identically, then 1,000 recorded requests, then the diff.
set -e
export NODE_EXTRA_CA_CERTS=$HOME/.cache/wp15/tls/cert.pem SSL_CERT_FILE=$HOME/.cache/wp15/tls/bundle.pem
cd /home/user/workspace/repo/tests/m0
A=$(node -e 'import("./run/wp15.ts").then(async m => console.log(await m.tok("alice")))')
node run/wp15.ts setup
for H in https://graft-wp15-host.<workers-subdomain>.workers.dev https://graft-wp15-base.<workers-subdomain>.workers.dev; do bash wp15/seed.sh $H $A; done
rm -f results/raw/wp15-l011.jsonl
node wp15/l011-workload.ts --target 1000
node run/wp15-diff.ts > results/raw/wp15-l011-summary.json
echo DONE
