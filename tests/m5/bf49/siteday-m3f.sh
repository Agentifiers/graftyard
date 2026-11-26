#!/bin/bash
# BF-49's site day and edge day on graft-m3f (tests/m5/results/m5-wp5-hosted.md), from this checkout's HEAD: the
# install's anchor, the shard and sshfront images it runs, standard-4, SSH kept on (ssh.graftyard.dev is served here),
# and the export bucket. One-shot: a second run with the same tag does nothing.
#   bash tests/m5/bf49/siteday-m3f.sh <tag> [site-only|edge-only]  -> tests/m5/results/raw/bf49/siteday-<tag>.log
set -uo pipefail
cd "$(dirname "$0")/../../.."
A=${CLOUDFLARE_ACCOUNT_ID:?}
IMG=registry.cloudflare.com/$A/graft-m3f-gitshard@sha256:6ee7a5abf57fbd2962f0dd8e213cbdf0b943b14944b6413de2dec41b5bf02e27
SIMG=registry.cloudflare.com/$A/graft-m3f-sshfront@sha256:fc6eda91789c41db39323a7bb72d37176a6f09f05d42e8714ed05f641dcc40a0
STORE=${CF_SECRETS_STORE_ID:?}
FORMATS=32.1,32.4,32.5,32.6,32.7,32.9,32.11,32.13,16.2,17.2,18.2,20.1,20.2,32.8,32.12,32.14,18.3,32.18,32.26
BUCKET=graft-m3f-export
L=tests/m5/results/raw/bf49/siteday-$1.log
mkdir -p "$(dirname "$L")"
[ -e "$L" ] && { echo "$L exists: one run per tag"; exit 1; }
{ date -u +%FT%TZ; git rev-parse --short HEAD; git status --short | head -5
  if [ "${2:-}" != edge-only ]; then
    node workers/site/deploy.mjs ~/.graft-m3f/sites.json --test-faults --anchor ~/.graft-m3f/install.toml \
      --image "$IMG" --vars ~/.graft-m3f/site-vars.json --ssh $STORE --ssh-image "$SIMG" --edge graft-m3f-edge \
      --export-bucket $BUCKET
    echo "site exit $?"; date -u +%FT%TZ
  fi
  if [ "${2:-}" != site-only ]; then
    node tests/m1/integ/deploy-edge.mjs ~/.graft-m3f/sites.json graft-m3f-edge --test-faults \
      --anchor ~/.graft-m3f/install.toml --ops graft-m3f-ops --ssh ~/.graft-m3f/ssh/hostkeys.json \
      --var FORMATS_ENABLED=$FORMATS --export-bucket $BUCKET
    echo "edge exit $?"; date -u +%FT%TZ
  fi
  echo DONE; } 2>&1 | sed "s/${CF_SECRETS_STORE_ID:?}/<store>/g" > "$L"
