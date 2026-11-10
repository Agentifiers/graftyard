#!/bin/bash
# B0-30 site day 6 on graft-m1 (sites a to d): the same shard image as the first (tests/am1/b030/siteday.sh), with
# GRAFTYARD_REPO_REQUEST_RETENTION_S = 9600 (the site's `request_retention`, repository chains only). The replay runs
# 30 days of journey 9 in 40 h (18x), so the design's 48 h REQUEST_RETENTION scales to 160 min (9,600 s); at 48 h the
# `request` table would hold the whole run. Site day 2 set the install-wide GRAFTYARD_REQUEST_RETENTION_S instead, and
# non-tiered chains' restore check refused it (authz:7100001: "request rows differ"); site day 3 took it back out.
# Site day 5 is site day 4's image and variables with the install-source fix (workers/site/src/load.rs, `source`):
# reading every log row above the archive into memory took site a's isolate down on the first 2 to 39 archive.
# Site day 6 adds the shard's catch-up in pieces (crates/git/src/host.rs, `ShardHost::catch_up`, CATCH_UP_ENTRIES):
# a shard 4,480 entries behind took its isolate past memory on every attempt (b0-30.md, "Blocked on", 8).
# No shard roll: the container image is unchanged; the catch-up runs in the site script. Idempotent: skipped once its log ends in DONE.
#   amp orb service start b030-siteday6 --command 'bash tests/am1/b030/siteday6.sh'
cd "$(dirname "$0")/../../.."
export PATH=$HOME/.cargo/bin:$PATH CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/home/user/workspace/repo/.worktrees/target-am1
export BED_TOKEN_FILE=$HOME/.graft-wp6/bed.token CARGO_PROFILE_RELEASE_DEBUG=0
IMG=registry.cloudflare.com/<cf-account-id>/graft-m1-site-gitshard-git@sha256:172216ad0cbdf8779b0c20a698a0b5040a3de4295581408698ea3b157a8df3e9
L=tests/am1/raw/b030-siteday6.log
if ! grep -q '^DONE' "$L" 2>/dev/null; then
  echo '{"GRAFTYARD_AUDIT":"1","GRAFTYARD_REPO_REQUEST_RETENTION_S":"9600"}' > /tmp/b030-site-vars.json
  { date -u +%FT%TZ; git rev-parse --short HEAD; git status --short | head -5
    node workers/site/deploy.mjs tests/m3/finish/sites4.json --genesis tests/m1/wp7/genesis.json --test-faults \
      --vars /tmp/b030-site-vars.json --image "$IMG"
    echo "exit $?"; date -u +%FT%TZ; echo DONE; } > "$L" 2>&1
fi
exec sleep infinity
