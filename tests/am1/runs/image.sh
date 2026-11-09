#!/bin/bash
# Builds containers/git from this worktree's HEAD (session refs, paged /refs) and pushes it as
# graft-m1-site-gitshard-git:am1runs, then prints the digest-pinned name a site day's --image takes.
#   bash tests/am1/runs/image.sh <log name>   (output in tests/am1/raw/runs/<log name>)
set -uo pipefail
cd "$(dirname "$0")/../../.."
{ date -u +%FT%TZ; git rev-parse --short HEAD
  docker build -q -t graft-m1-site-gitshard-git:am1runs containers/git && \
  (cd tests/m0 && npx wrangler containers push graft-m1-site-gitshard-git:am1runs || npx wrangler containers push graft-m1-site-gitshard-git:am1runs)
  echo "exit $?"
  docker inspect --format '{{join .RepoDigests "\n"}}' graft-m1-site-gitshard-git:am1runs
  date -u +%FT%TZ; echo DONE; } > tests/am1/raw/runs/$1 2>&1
