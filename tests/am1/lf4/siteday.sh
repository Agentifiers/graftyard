#!/bin/bash
# lf4-rest's site day on graft-m3c from this worktree's HEAD: test-faults, the install's anchor, the shard image
# graft-m3c already runs (containers/ unchanged since e46da45). No shard roll. Then graft-m3c-edge with the same
# FORMATS_ENABLED and, new, `--ops graft-m3c-ops`. Output in tests/am1/raw/lf4/<log name>.
#   bash tests/am1/lf4/siteday.sh <log name> [edge-only]
set -uo pipefail
export PATH=$HOME/.cargo/bin:$PATH CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=$HOME/.cache/lf4/target
export BED_TOKEN_FILE=$HOME/.graft-m3c/bed.token
cd /home/user/workspace/repo
IMG=registry.cloudflare.com/${CLOUDFLARE_ACCOUNT_ID:?}/graft-m1-site-gitshard-git@sha256:172216ad0cbdf8779b0c20a698a0b5040a3de4295581408698ea3b157a8df3e9
# lf4-rest-3 adds 32.18 (`IssueOp::CreateCheck`), which only a site day from lf4-rest-3 or later reads.
FORMATS=32.1,32.4,32.5,32.6,32.7,32.9,32.11,32.13,16.2,17.2,18.2,20.1,20.2,32.8,32.12,32.14,18.3,32.18
L=tests/am1/raw/lf4/$1
mkdir -p tests/am1/raw/lf4
# A supervised service restarts a script that exits; a finished day must not deploy again.
grep -q '^DONE' "$L" 2>/dev/null && exec sleep infinity
{ date -u +%FT%TZ; git rev-parse --short HEAD; git status --short | head -5
  if [ "${2:-}" != edge-only ]; then
    node workers/site/deploy.mjs $HOME/.graft-m3c/sites.json --test-faults --anchor $HOME/.graft-m3c/install.toml --image "$IMG"
    echo "site exit $?"; date -u +%FT%TZ
  fi
  node tests/m1/integ/deploy-edge.mjs $HOME/.graft-m3c/sites.json graft-m3c-edge --test-faults \
    --anchor $HOME/.graft-m3c/install.toml --ops graft-m3c-ops --var FORMATS_ENABLED=$FORMATS
  echo "edge exit $?"; date -u +%FT%TZ; echo DONE; } > "$L" 2>&1
