#!/usr/bin/env bash
# M0-WP15 L0-07 full run: reset and seed the layer-on host, then each vendor driver with 100 pushes after revoke.
set -e
export NODE_EXTRA_CA_CERTS=$HOME/.cache/wp15/tls/cert.pem
cd /home/user/workspace/repo/tests/m0
A=$(node -e 'import("./run/wp15.ts").then(async m => console.log(await m.tok("alice")))')
node run/wp15.ts setup
bash wp15/seed.sh https://graft-wp15-host.${CF_WORKERS_SUBDOMAIN:?}.workers.dev $A
for v in claude cursor codex ghmcp; do node wp15/l007-vendor.ts $v --pushes 100 || echo "$v exited $?"; done
echo DONE
