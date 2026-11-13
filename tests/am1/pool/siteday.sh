#!/bin/bash
# pool-grants' site day on graft-m3c from this worktree's HEAD: test-faults, the install's anchor, SHARD_INSTANCE
# standard-4, and the shard image tests/am1/pool/image.sh built from this branch's containers/git (unchanged from
# main a2687fe). Then graft-m3c-edge (unchanged code, redeployed so it carries the same formats and `--ops`), and every
# running shard and pool shard torn down so each starts on the new site version. Output in tests/am1/raw/pool/<log>.
#   IMG=<digest-pinned image> [SITE_ONLY=1] bash tests/am1/pool/siteday.sh <log name>
set -uo pipefail
export PATH=$HOME/.cargo/bin:$PATH CARGO_INCREMENTAL=0
cd "$(dirname "$0")/../../.."
source tests/am1/lf13/env-m3c.sh
FORMATS=32.1,32.4,32.5,32.6,32.7,32.9,32.11,32.13,16.2,17.2,18.2,20.1,20.2,32.8,32.12,32.14,18.3,32.18
L=tests/am1/raw/pool/$1
grep -q '^DONE' "$L" 2>/dev/null && exec sleep infinity
VARS=$(mktemp)
echo '{"SHARD_INSTANCE": "standard-4"}' > "$VARS"
{ date -u +%FT%TZ; git rev-parse --short HEAD; git status --short | grep -v '^??' | head -5; echo "image $IMG"
  node workers/site/deploy.mjs $HOME/.graft-m3c/sites.json --test-faults --anchor $HOME/.graft-m3c/install.toml \
    --image "$IMG" --vars "$VARS"
  echo "site exit $?"; date -u +%FT%TZ
  if [ -z "${SITE_ONLY:-}" ]; then
    node tests/m1/integ/deploy-edge.mjs $HOME/.graft-m3c/sites.json graft-m3c-edge --test-faults \
      --anchor $HOME/.graft-m3c/install.toml --ops graft-m3c-ops --var FORMATS_ENABLED=$FORMATS
    echo "edge exit $?"; date -u +%FT%TZ
  fi
  python3 tests/fanout/teardown.py --all-nets --pools; echo "teardown exit $?"
  date -u +%FT%TZ; echo DONE; } > "$L" 2>&1
rm -f "$VARS"
exec sleep infinity
