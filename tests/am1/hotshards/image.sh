#!/bin/bash
# Builds containers/git from this worktree's HEAD and pushes it to the account's registry as
# graft-m1-site-gitshard-git:hs2 (hot-shards-2), then prints the digest-pinned name a site day's --image takes.
#   bash tests/am1/hosted/image.sh <log name>
set -uo pipefail
cd "$(dirname "$0")/../../.."
mkdir -p tests/am1/raw
{ date -u +%FT%TZ; git rev-parse --short HEAD
  docker build -q -t graft-m1-site-gitshard-git:hs2 containers/git && \
  (cd tests/m0 && npx wrangler containers push graft-m1-site-gitshard-git:hs2 || npx wrangler containers push graft-m1-site-gitshard-git:hs2)
  echo "exit $?"
  docker inspect --format '{{join .RepoDigests "\n"}}' graft-m1-site-gitshard-git:hs2
  date -u +%FT%TZ; echo DONE; } > tests/am1/raw/$1 2>&1
