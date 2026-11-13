#!/bin/bash
# Builds containers/git from this worktree's HEAD as graft-m1-site-gitshard-git:pg and pushes it to the account's
# registry (as tests/am1/hotshards/image.sh), then prints the digest-pinned name. Output in tests/am1/raw/pool/<log>.
set -uo pipefail
cd "$(dirname "$0")/../../.."
L=tests/am1/raw/pool/$1
grep -q '^DONE' "$L" 2>/dev/null && exec sleep infinity
{ date -u +%FT%TZ; git rev-parse --short HEAD
  docker build -q -t graft-m1-site-gitshard-git:pg containers/git && \
  (cd tests/m0 && (npx wrangler containers push graft-m1-site-gitshard-git:pg || npx wrangler containers push graft-m1-site-gitshard-git:pg))
  echo "exit $?"
  docker inspect --format '{{join .RepoDigests "\n"}}' graft-m1-site-gitshard-git:pg
  date -u +%FT%TZ; echo DONE; } > "$L" 2>&1
exec sleep infinity
