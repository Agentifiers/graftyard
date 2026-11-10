#!/bin/bash
# B0-30's site day on graft-m1, as one orb service: sites a to d from this worktree's HEAD with the shard image built
# from containers/git at origin/main 1e25da8 (fanout; containers/ is unchanged on b0-30), then graft-m1-edge from the
# same commit with its FORMATS_ENABLED unchanged (tests/am1/hosted/deploy-edge.sh's list).
#
# The first run (07bb30e0, log raw/b030-siteday1.log) deployed sites a to d at 06:48Z to 06:50Z and was stopped at
# 07:25Z while `--roll-shards` was still moving other networks' running shard containers, one catch-up at a time
# (105 done). B0-30's network had no shard then, so its first shard starts on the new image; the rest of the install's
# old shards were left on their image. Idempotent across restarts: a step whose log ends in DONE is not run again.
#   amp orb service start b030-siteday --command 'bash tests/am1/b030/siteday.sh'
cd "$(dirname "$0")/../../.."
IMG=registry.cloudflare.com/<cf-account-id>/graft-m1-site-gitshard-git@sha256:172216ad0cbdf8779b0c20a698a0b5040a3de4295581408698ea3b157a8df3e9
S=tests/am1/raw/b030-siteday1.log
E=tests/am1/raw/b030-edge.log
grep -q '^DONE' "$S" 2>/dev/null || grep -q 'Current Version ID: 125026fd' "$S" || bash tests/am1/hosted/siteday.sh b030-siteday1.log "$IMG"
grep -q '^DONE' "$E" 2>/dev/null || bash tests/am1/hosted/deploy-edge.sh b030-edge.log
{ grep -E 'Current Version ID|^exit' "$S" "$E"; echo DONE; } > tests/am1/raw/b030-siteday.status
exec sleep infinity
