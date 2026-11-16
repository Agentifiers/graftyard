#!/bin/bash
# A site day on graft-m2b from this worktree's HEAD: test-faults, the install's anchor, the am1runs shard image,
# and --roll-shards through graft-m2b-edge. Audit stays off, as m3-rows and wire-fix ran it.
#   bash tests/am1/runs/siteday.sh <log name> <image>   (output in tests/am1/raw/runs/<log name>)
set -uo pipefail
export PATH=$HOME/.cargo/bin:$PATH CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/home/user/workspace/repo/.worktrees/target-rows
export BED_TOKEN_FILE=$HOME/.graft-m2b/bed.token
cd "$(dirname "$0")/../../.."
{ date -u +%FT%TZ; git rev-parse --short HEAD; git status --short | head -5
  # NOROLL=1: no --roll-shards (shards are derived caches; a fresh repository's shards start on the image).
  ROLL=(--roll-shards https://graft-m2b-edge.<workers-subdomain>.workers.dev); [ -n "${NOROLL:-}" ] && ROLL=()
  # SHARD_INSTANCE=standard-4: the site var the shard container starts at (workers/site/src/shard.rs, `instance`).
  # ROLLUP=1: the site var GRAFTYARD_ROLLUP (G9's roll-up gate, `log_archive::rollup_enabled`), with 32.25 enabled.
  VARS=(); if [ -n "${SHARD_INSTANCE:-}${ROLLUP:-}" ]; then
    python3 -c 'import json,os; v={k:os.environ[e] for k,e in (("SHARD_INSTANCE","SHARD_INSTANCE"),("GRAFTYARD_ROLLUP","ROLLUP")) if os.environ.get(e)}; print(json.dumps(v))' \
      > /tmp/siteday-vars.json; VARS=(--vars /tmp/siteday-vars.json); cat /tmp/siteday-vars.json; fi
  echo "SHARD_INSTANCE=${SHARD_INSTANCE:-unset} GRAFTYARD_ROLLUP=${ROLLUP:-unset}"
  node workers/site/deploy.mjs $HOME/.graft-m2b/sites.json --test-faults --anchor $HOME/.graft-m2b/install.toml \
    --image "$2" "${VARS[@]}" "${ROLL[@]}"
  echo "exit $?"; date -u +%FT%TZ; echo DONE; } > tests/am1/raw/runs/$1 2>&1
