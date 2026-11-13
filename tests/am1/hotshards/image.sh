#!/bin/bash
# Builds containers/git from this worktree's HEAD and pushes it to the account's registry as
# graft-m1-site-gitshard-git:hs3m (hot-shards-3 merged with main 020b3e5), then prints the digest-pinned name a site day's --image takes.
#   bash tests/am1/hosted/image.sh <log name>
set -uo pipefail
cd "$(dirname "$0")/../../.."
mkdir -p tests/am1/raw
{ date -u +%FT%TZ; git rev-parse --short HEAD
  docker build -q -t graft-m1-site-gitshard-git:hs3m containers/git && \
  (cd tests/m0 && npx wrangler containers push graft-m1-site-gitshard-git:hs3m || npx wrangler containers push graft-m1-site-gitshard-git:hs3m)
  echo "exit $?"
  docker inspect --format '{{join .RepoDigests "\n"}}' graft-m1-site-gitshard-git:hs3m
  date -u +%FT%TZ; echo DONE; } > tests/am1/raw/$1 2>&1
