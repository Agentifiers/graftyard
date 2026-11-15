#!/bin/bash
# Builds containers/runner from this worktree's HEAD and pushes it to the runner account's registry as
# graft-runners-runner:<tag>, then prints the digest-pinned name `graftyard runners init --image` takes.
#   bash tests/m4/wp4/image.sh <tag>      (log in tests/m4/raw/wp4/image-<tag>.log)
# Needs CLOUDFLARE_RUNNER_API_TOKEN and RUNNER_ACCOUNT (the runner account's id), never the install's token.
#
# The orb's user is not in the docker group (/var/run/docker.sock is root:docker), but passwordless sudo reaches the
# daemon, so the build runs `sudo docker` and wrangler's push runs through a one-line wrapper named by
# WRANGLER_DOCKER_BIN. The engine has no buildx, so `wrangler containers build` (which passes --load) cannot run here;
# the plain `docker build` below is what tests/am1/hotshards/image.sh and tests/m3/wirefix/image.sh did.
set -euo pipefail
cd "$(dirname "$0")/../../.."
tag=${1:?usage: image.sh <tag>}
: "${CLOUDFLARE_RUNNER_API_TOKEN:?}" "${RUNNER_ACCOUNT:?}"
mkdir -p tests/m4/raw/wp4
log=tests/m4/raw/wp4/image-$tag.log
bin=$(mktemp -d)
printf '#!/bin/sh\nexec sudo -n docker "$@"\n' > "$bin/docker"
chmod +x "$bin/docker"
name=graft-runners-runner:$tag
{
  date -u +%FT%TZ; git rev-parse --short HEAD
  sudo -n docker build -q -t "$name" containers/runner
  (cd tests/m0 && WRANGLER_DOCKER_BIN="$bin/docker" CLOUDFLARE_API_TOKEN="$CLOUDFLARE_RUNNER_API_TOKEN" \
    CLOUDFLARE_ACCOUNT_ID="$RUNNER_ACCOUNT" npx wrangler containers push "$name")
  sudo -n docker inspect --format '{{join .RepoDigests "\n"}}' "$name"
  date -u +%FT%TZ; echo DONE
} > "$log" 2>&1
rm -rf "$bin"
grep "registry.cloudflare.com/$RUNNER_ACCOUNT/graft-runners-runner@sha256:" "$log" | tail -1
