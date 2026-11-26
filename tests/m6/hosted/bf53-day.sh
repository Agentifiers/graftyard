#!/bin/bash
# BF-53's site day and edge day on graft-m3f (`tests/m6/results/bf53.md`), from this checkout's HEAD: test-faults,
# the install's anchor, the shard image in $IMG (built from the same head), sshfront reused, standard-4, SSH kept on
# (ssh.graftyard.dev is served here), the export bucket, and the edge's live FORMATS_ENABLED plus 17.5 (Tokens V5,
# which `POST /api/graftyard/v1/lifeboat/tokens` needs). Every site schedule ends `[]`. One-shot per tag: an orb
# service that reruns it does nothing (b9m3f1's rerun put the crons back).
#   IMG=<pinned shard image> bash tests/m6/hosted/bf53-day.sh <tag>   -> tests/m6/raw/bf53/day-<tag>.log
set -uo pipefail
cd "$(dirname "$0")/../../.."
T=${1:?tag}
O=tests/m6/raw/bf53; mkdir -p "$O"
L=$O/day-$T.log
[ -e "$L" ] && exec sleep infinity
A=${CLOUDFLARE_ACCOUNT_ID:?}
IMG=${IMG:?the pinned shard image}
SIMG=registry.cloudflare.com/$A/graft-m3f-sshfront@sha256:fc6eda91789c41db39323a7bb72d37176a6f09f05d42e8714ed05f641dcc40a0
STORE=${CF_SECRETS_STORE_ID:?}
S="graft-m3f-site-a graft-m3f-site-b graft-m3f-site-c"
api() { curl -s -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" "https://api.cloudflare.com/client/v4/accounts/$A$1" "${@:2}"; }
scrub() { sed -e "s/$A/<cf-account-id>/g" -e "s/$CF_SECRETS_STORE_ID/<store>/g" -e "s/${CF_WORKERS_SUBDOMAIN:?}/<sub>/g" -e "s#$HOME/#~/#g"; }
live_formats() { api /workers/scripts/graft-m3f-edge/settings | jq -r '.result.bindings[] | select(.name == "FORMATS_ENABLED") | .text'; }
{ date -u +%FT%TZ; git rev-parse --short HEAD; git status --short -- workers crates containers | head -5
  echo "image ${IMG##*@}"
  F=$(live_formats); echo "live FORMATS_ENABLED: $F"
  case ",$F," in *,17.5,*) ;; *) F=$F,17.5 ;; esac
  node workers/site/deploy.mjs ~/.graft-m3f/sites.json --test-faults --anchor ~/.graft-m3f/install.toml \
    --image "$IMG" --vars ~/.graft-m3f/site-vars.json --ssh $STORE --ssh-image "$SIMG" --edge graft-m3f-edge \
    --export-bucket graft-m3f-export 2>&1 | grep -E "Current Version|Deployed|rror|exit" | tail -8
  echo "site exit ${PIPESTATUS[0]}"; date -u +%FT%TZ
  node tests/m1/integ/deploy-edge.mjs ~/.graft-m3f/sites.json graft-m3f-edge --test-faults \
    --anchor ~/.graft-m3f/install.toml --ops graft-m3f-ops --ssh ~/.graft-m3f/ssh/hostkeys.json \
    --var FORMATS_ENABLED=$F --export-bucket graft-m3f-export 2>&1 | grep -E "Current Version|Deployed|rror" | tail -4
  echo "edge exit ${PIPESTATUS[0]}"; date -u +%FT%TZ
  echo "now FORMATS_ENABLED: $(live_formats)"
  # Crons off: the site day puts `*/15` back; a schedule set then cleared is how the earlier days got `[]` to stick.
  for try in 1 2 3 4; do
    for s in $S; do
      api "/workers/scripts/$s/schedules" -X PUT -H 'content-type: application/json' -d '[{"cron":"0 0 1 1 *"}]' >/dev/null; sleep 3
      api "/workers/scripts/$s/schedules" -X PUT -H 'content-type: application/json' -d '[]' >/dev/null
    done
    left=0; for s in $S; do [ "$(api /workers/scripts/$s/schedules | jq -c .result.schedules)" = "[]" ] || left=1; done
    [ $left = 0 ] && break; sleep 10
  done
  for s in $S graft-m3f-edge; do
    echo "$s $(api /workers/scripts/$s/deployments | jq -c '.result.deployments[0] | [.created_on, [.versions[] | .version_id[0:8]]]') crons $(api /workers/scripts/$s/schedules | jq -c '[.result.schedules[]?.cron]')"
  done
  date -u +%FT%TZ; echo DONE; } 2>&1 | scrub >"$L"
exec sleep infinity
