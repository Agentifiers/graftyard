#!/bin/bash
# Builds containers/git from this worktree's HEAD and pushes it to the account's registry as
# graft-m1-site-gitshard-git:$TAG (default hs2, hot-shards-2's), then prints the digest-pinned name a site day's
# --image takes.
#   TAG=rb bash tests/am1/hotshards/image.sh <log name>
set -uo pipefail
cd "$(dirname "$0")/../../.."
mkdir -p tests/am1/raw
{ date -u +%FT%TZ; git rev-parse --short HEAD
  docker build -q -t graft-m1-site-gitshard-git:${TAG:-hs2} containers/git && \
  (cd tests/m0 && npx wrangler containers push graft-m1-site-gitshard-git:${TAG:-hs2} || npx wrangler containers push graft-m1-site-gitshard-git:${TAG:-hs2})
  echo "exit $?"
  docker inspect --format '{{join .RepoDigests "\n"}}' graft-m1-site-gitshard-git:${TAG:-hs2}
  date -u +%FT%TZ; echo DONE; } > tests/am1/raw/$1 2>&1
