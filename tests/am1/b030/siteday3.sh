#!/bin/bash
# B0-30 site day 3 on graft-m1 (sites a to d): the same commit and shard image as the first
# (tests/am1/b030/siteday.sh), plus the variables of the first (GRAFTYARD_AUDIT=1 only), undoing site day 2: its GRAFTYARD_REQUEST_RETENTION_S=9600 applied to every chain, and the restore check of non-tiered chains (applied at most RESTORE_CHECK_MAX) replays the log at the new retention, so authz:7100001 refused to join ("request rows differ"). The replay runs 30 days of journey 9 in 40 h
# (18x), so the design's 48 h REQUEST_RETENTION (2 days) is 160 min of replay; at 48 h the `request` table would hold
# the whole run. No shard roll: the image is unchanged. The edge is unchanged (it reads no site variable).
# Idempotent: skipped once its log ends in DONE.
#   amp orb service start b030-siteday3 --command 'bash tests/am1/b030/siteday3.sh'
cd "$(dirname "$0")/../../.."
export PATH=$HOME/.cargo/bin:$PATH CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/home/user/workspace/repo/.worktrees/target-am1
export BED_TOKEN_FILE=$HOME/.graft-wp6/bed.token CARGO_PROFILE_RELEASE_DEBUG=0
IMG=registry.cloudflare.com/<cf-account-id>/graft-m1-site-gitshard-git@sha256:172216ad0cbdf8779b0c20a698a0b5040a3de4295581408698ea3b157a8df3e9
L=tests/am1/raw/b030-siteday3.log
if ! grep -q '^DONE' "$L" 2>/dev/null; then
  echo '{"GRAFTYARD_AUDIT":"1"}' > /tmp/b030-site-vars.json
  { date -u +%FT%TZ; git rev-parse --short HEAD; git status --short | head -5
    node workers/site/deploy.mjs tests/m3/finish/sites4.json --genesis tests/m1/wp7/genesis.json --test-faults \
      --vars /tmp/b030-site-vars.json --image "$IMG"
    echo "exit $?"; date -u +%FT%TZ; echo DONE; } > "$L" 2>&1
fi
exec sleep infinity
