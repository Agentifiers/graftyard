#!/bin/bash
# Builds containers/git from this worktree's HEAD and pushes it to the account's registry as
# graft-m1-site-gitshard-git:$TAG (default hs3), then prints the digest-pinned name a site day's
# --image takes.
#   TAG=hs3 bash tests/am1/hotshards/image.sh <log name>
set -uo pipefail
cd "$(dirname "$0")/../../.."
mkdir -p tests/am1/raw
{ date -u +%FT%TZ; git rev-parse --short HEAD
  docker build -q -t graft-m1-site-gitshard-git:${TAG:-hs3} containers/git && \
  (cd tests/m0 && npx wrangler containers push graft-m1-site-gitshard-git:${TAG:-hs3} || npx wrangler containers push graft-m1-site-gitshard-git:${TAG:-hs3})
  echo "exit $?"
  docker inspect --format '{{join .RepoDigests "\n"}}' graft-m1-site-gitshard-git:${TAG:-hs3}
  date -u +%FT%TZ; echo DONE; } > tests/am1/raw/$1 2>&1
