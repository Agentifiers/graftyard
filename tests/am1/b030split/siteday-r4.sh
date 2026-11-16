#!/bin/bash
# B0-30's short run (b030r4, operator 2026-11-16, option A): the site day on graft-m1 from the main checked out here,
# with a git shard image built from the same commit's containers/git (tests/am1/hotshards/image.sh, tag b030r4),
# SHARD_INSTANCE standard-4 and site day 9's variables, then the edge from the same commit with
# tests/am1/hosted/deploy-edge.sh's flags. Writes tests/am1/raw/b030r4.start first: the $150 run stop counts from here.
# Before and after, tests/fanout/acctcost.py for the day so far goes into the log (rule 1). Idempotent: skipped once
# its log ends in DONE.
#   amp orb service start b030r4-siteday --command 'bash tests/am1/b030split/siteday-r4.sh'
cd "$(dirname "$0")/../../.."
export PATH=$HOME/.cargo/bin:$PATH CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/home/user/workspace/repo/.worktrees/target-am1
export BED_TOKEN_FILE=$HOME/.graft-wp6/bed.token CARGO_PROFILE_RELEASE_DEBUG=0
EDGE=https://graft-m1-edge.<workers-subdomain>.workers.dev
L=tests/am1/raw/b030r4-siteday.log
if ! grep -q '^DONE' "$L" 2>/dev/null; then
  [ -s tests/am1/raw/b030r4.start ] || date -u +%FT%TZ > tests/am1/raw/b030r4.start
  { date -u +%FT%TZ; git rev-parse --short HEAD; git status --short | head -5
    echo "acctcost before: $(python3 tests/fanout/acctcost.py | head -1)"
    TAG=b030r4 bash tests/am1/hotshards/image.sh b030split-image-r4.log
    IMG=$(grep '^registry.cloudflare.com/.*@sha256:' tests/am1/raw/b030split-image-r4.log | head -1)
    grep -q '^exit 0' tests/am1/raw/b030split-image-r4.log && [ -n "$IMG" ] || { echo "image failed"; IMG=; }
    echo "image $IMG"
    s=1
    if [ -n "$IMG" ]; then
      echo '{"GRAFTYARD_AUDIT":"1","GRAFTYARD_REPO_REQUEST_RETENTION_S":"9600","SHARD_INSTANCE":"standard-4"}' > /tmp/b030-site-vars.json
      node workers/site/deploy.mjs tests/m3/finish/sites4.json --genesis tests/m1/wp7/genesis.json --test-faults \
        --vars /tmp/b030-site-vars.json --image "$IMG" --roll-shards "$EDGE"
      s=$?
    fi
    echo "sites exit $s"; date -u +%FT%TZ
    e=1
    if [ $s = 0 ]; then
      node tests/m1/integ/deploy-edge.mjs tests/m3/finish/sites4.json graft-m1-edge --test-faults \
        --genesis tests/m1/wp7/genesis.json --ops graft-m1-ops \
        --var FORMATS_ENABLED=32.1,32.4,32.5,32.6,32.7,32.9,16.2,17.2,18.2,20.1,32.10,18.4,32.8,32.12,32.14,18.3 --var 'LOGINS={}'
      e=$?; echo "edge exit $e"
    fi
    echo "acctcost after: $(python3 tests/fanout/acctcost.py | head -1)"
    echo "exit $((s | e))"; date -u +%FT%TZ; echo DONE; } > "$L" 2>&1
fi
exec sleep infinity
