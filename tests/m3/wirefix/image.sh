#!/bin/bash
# Builds containers/git from this worktree's HEAD (the paged /refs routes) and pushes it as
# graft-m1-site-gitshard-git:wirefix, then prints the digest-pinned name a site day's --image takes.
#   bash tests/m3/wirefix/image.sh <log name>   (output in tests/m3/raw/wirefix/<log name>)
set -uo pipefail
cd "$(dirname "$0")/../../.."
mkdir -p tests/m3/raw/wirefix
{ date -u +%FT%TZ; git rev-parse --short HEAD
  docker build -q -t graft-m1-site-gitshard-git:wirefix containers/git && \
  (cd tests/m0 && npx wrangler containers push graft-m1-site-gitshard-git:wirefix || npx wrangler containers push graft-m1-site-gitshard-git:wirefix)
  echo "exit $?"
  docker inspect --format '{{join .RepoDigests "\n"}}' graft-m1-site-gitshard-git:wirefix
  date -u +%FT%TZ; echo DONE; } > tests/m3/raw/wirefix/$1 2>&1
