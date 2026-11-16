#!/bin/bash
# The third B0-30 run's site day on graft-m1 (operator, 2026-11-16): sites a to d from main 0e3d13cc (b030-fixes merged:
# (i-1), (i-2), (i-3), (ii)(a), create→/place; with main's receipt rule), the git shard image built from the same
# commit's containers/git (tag b030r3, tests/am1/raw/b030split-image-r3.log), SHARD_INSTANCE standard-4, and the same
# variables as site day 9. Then the edge from the same commit, as tests/am1/hosted/deploy-edge.sh deploys it (the edge
# calls graftyard_lookup::create, where the create→/place rule lives, and it ran 19b495a1 from 2026-11-13). Writes
# tests/am1/raw/b030r3.start at its start: the $190 stop counts from here. Idempotent: skipped once its log ends in DONE.
#   amp orb service start b030r3-siteday --command 'bash tests/am1/b030split/siteday-r3.sh'
cd "$(dirname "$0")/../../.."
export PATH=$HOME/.cargo/bin:$PATH CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/home/user/workspace/repo/.worktrees/target-am1
export BED_TOKEN_FILE=$HOME/.graft-wp6/bed.token CARGO_PROFILE_RELEASE_DEBUG=0
IMG=registry.cloudflare.com/<cf-account-id>/graft-m1-site-gitshard-git@sha256:c38059dbc4f5df1b384040ff123f19c9e8a3c7d95d812b2808ffce89b64e5668
EDGE=https://graft-m1-edge.<workers-subdomain>.workers.dev
L=tests/am1/raw/b030r3-siteday.log
if ! grep -q '^DONE' "$L" 2>/dev/null; then
  [ -s tests/am1/raw/b030r3.start ] || date -u +%FT%TZ > tests/am1/raw/b030r3.start
  echo '{"GRAFTYARD_AUDIT":"1","GRAFTYARD_REPO_REQUEST_RETENTION_S":"9600","SHARD_INSTANCE":"standard-4"}' > /tmp/b030-site-vars.json
  { date -u +%FT%TZ; git rev-parse --short HEAD; git status --short | head -5
    node workers/site/deploy.mjs tests/m3/finish/sites4.json --genesis tests/m1/wp7/genesis.json --test-faults \
      --vars /tmp/b030-site-vars.json --image "$IMG" --roll-shards "$EDGE"
    s=$?; echo "sites exit $s"; date -u +%FT%TZ
    e=1
    if [ $s = 0 ]; then
      node tests/m1/integ/deploy-edge.mjs tests/m3/finish/sites4.json graft-m1-edge --test-faults \
        --genesis tests/m1/wp7/genesis.json --ops graft-m1-ops \
        --var FORMATS_ENABLED=32.1,32.4,32.5,32.6,32.7,32.9,16.2,17.2,18.2,20.1,32.10,18.4,32.8,32.12,32.14,18.3 --var 'LOGINS={}'
      e=$?; echo "edge exit $e"
    fi
    echo "exit $((s | e))"; date -u +%FT%TZ; echo DONE; } > "$L" 2>&1
fi
exec sleep infinity
