#!/bin/bash
# A site day and edge day on graft-m3f from this worktree's HEAD, for the LM-1 rows split onto it (B0-31, B0-32,
# L0-12; tests/am1/results/lm1-rows-m3f.md): test-faults, the install's anchor, the shard image IMG, SHARD_INSTANCE
# from ~/.graft-m3f/site-vars.json (standard-4), then graft-m3f-edge with `--ops graft-m3f-ops` and the install's
# FORMATS_ENABLED (tests/am1/results/install-m3f.md). Log the day in install-m3f.md's site-day table first.
#   IMG=<digest-pinned image> bash tests/fanout/siteday-m3f.sh <tag> [site-only|edge-only]
#   -> tests/am1/raw/runs/siteday-m3f-<tag>.log, ending "DONE"
set -uo pipefail
cd "$(dirname "$0")/../.."
export PATH=$HOME/.cargo/bin:$PATH
export BED_TOKEN_FILE=$HOME/.graft-m3f/bed.token
FORMATS=32.1,32.4,32.5,32.6,32.7,32.9,32.11,32.13,16.2,17.2,18.2,20.1,20.2,32.8,32.12,32.14,18.3,32.18
L=tests/am1/raw/runs/siteday-m3f-$1.log
mkdir -p tests/am1/raw/runs
[ -e "$L" ] && exec sleep infinity
{ date -u +%FT%TZ; git rev-parse --short HEAD; git status --short | head -5; echo "image $IMG"
  if [ "${2:-}" != edge-only ]; then
    node workers/site/deploy.mjs $HOME/.graft-m3f/sites.json --test-faults --anchor $HOME/.graft-m3f/install.toml \
      --image "$IMG" --vars $HOME/.graft-m3f/site-vars.json
    echo "site exit $?"; date -u +%FT%TZ
  fi
  [ "${2:-}" = site-only ] || node tests/m1/integ/deploy-edge.mjs $HOME/.graft-m3f/sites.json graft-m3f-edge --test-faults \
    --anchor $HOME/.graft-m3f/install.toml --ops graft-m3f-ops --var FORMATS_ENABLED=$FORMATS
  echo "edge exit $?"; date -u +%FT%TZ; echo DONE; } > "$L" 2>&1
exec sleep infinity
