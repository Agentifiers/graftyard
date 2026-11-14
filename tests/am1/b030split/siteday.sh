#!/bin/bash
# b030-split site day on graft-m1 (sites a to d): the rerun's day-1.905 site day (e9418153: every piece of an archive step against its step's target, keep_led's heard guard restored), after the day-1.82 site day (3117f62: archive drain throughput, 8 ranges a promotion, 8 objects an Archived entry, promotion ahead first, a 10 s wait on a shard with another after it; main c390251: own-bucket-first, decode-once, leader-clock-check), on top of the rerun day (main 31d19c9: liveness-slow-storage, repo-write-mark, m4-web-2, narrow-reads-3 with READ_PIPELINE; the split arms its check inside the alarm, 7c495d9), MetaSlim entries bounded by SLIM_BYTES (4cc1eae), the leader-only work pacing and keep-led (d879b0b) and the split's own re-arm, the metadata child group (MetaSplit, MetaSeed, MetaDropped, MetaSlim),
# the archive drain loop, and main's hot-shards 1 to 4 (promotion on a warm shard, in-band /archive/promote, the
# rehydrate resume fix). Shard image: hot-shards-4's (35c398c); main's later container changes (e7a584ed, 8fc30892) add only the M4 UI's /ui routes and /ingest's spool_ms and index_ms, read with serde defaults, which B0-30 does not need. SHARD_INSTANCE is
# standard-4 (operator approval for test installs). GRAFTYARD_REPO_REQUEST_RETENTION_S = 9600 as in site day 8
# (tests/am1/b030/siteday8.sh): 30 days in 40 h scales the 48 h REQUEST_RETENTION to 160 min.
# Idempotent: skipped once its log ends in DONE.
#   amp orb service start b030split-siteday --command 'bash tests/am1/b030split/siteday.sh'
cd "$(dirname "$0")/../../.."
export PATH=$HOME/.cargo/bin:$PATH CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/home/user/workspace/repo/.worktrees/target-am1
export BED_TOKEN_FILE=$HOME/.graft-wp6/bed.token CARGO_PROFILE_RELEASE_DEBUG=0
IMG=registry.cloudflare.com/<cf-account-id>/graft-m1-site-gitshard-git@sha256:f6d2ef408c766594b541156664de402d3db5107aa480ed3676fe2d62e34fed09
L=tests/am1/raw/b030split-siteday.log
if ! grep -q '^DONE' "$L" 2>/dev/null; then
  echo '{"GRAFTYARD_AUDIT":"1","GRAFTYARD_REPO_REQUEST_RETENTION_S":"9600","SHARD_INSTANCE":"standard-4"}' > /tmp/b030-site-vars.json
  { date -u +%FT%TZ; git rev-parse --short HEAD; git status --short | head -5
    node workers/site/deploy.mjs tests/m3/finish/sites4.json --genesis tests/m1/wp7/genesis.json --test-faults \
      --vars /tmp/b030-site-vars.json --image "$IMG"
    echo "exit $?"; date -u +%FT%TZ; echo DONE; } > "$L" 2>&1
fi
exec sleep infinity
