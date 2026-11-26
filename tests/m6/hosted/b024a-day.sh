#!/bin/bash
# B0-24a's site day, edge day and tools edge on graft-m3f (tests/m6/results/m6-wp2.md, "B0-24a on the Spectrum
# ingress: plan"), from this checkout's HEAD, which must hold the row's code (main `b6a44183` or later):
# - a shard image and an sshfront image built from HEAD's `containers/` and pushed digest-pinned, since main changed
#   both after m3f's last images (`7d5c7540…` from `2a020ed1`; `6085be00…` from `b9dd485f`, before `graftyard_2`);
# - the sites with `--test-faults --ssh`, which sets `SSH_KILL=1` on each, the install's anchor, standard-4 (`--vars`),
#   and the export bucket; the edge with `--ssh` and its live FORMATS_ENABLED; the tools edge from HEAD's
#   `tests/m1/wp7/edge.ts` (its `/ssh_kill`, `/ssh_status`, and `/commit_many` with `leader`);
# - every site schedule `[]` after, and the checks the smoke needs: `SSH_KILL` on each site, `meta/ssh` with slot A,
#   and `/ssh_status` answering for each site through the tools edge.
# One-shot per tag: an orb service that reruns it does nothing. Steps: `images`, `deploy`, or `all`.
#   bash tests/m6/hosted/b024a-day.sh <tag> [all|images|deploy]   -> tests/m6/raw/b024a/day-<tag>[-<step>].log
# `deploy` alone needs IMG and SIMG (digest-pinned), as `images` prints them.
set -uo pipefail
cd "$(dirname "$0")/../../.."
T=${1:?tag}; STEP=${2:-all}
O=tests/m6/raw/b024a; mkdir -p "$O"
L=$O/day-$T$([ "$STEP" = all ] || echo "-$STEP").log
[ -e "$L" ] && exec sleep infinity
A=${CLOUDFLARE_ACCOUNT_ID:?}
: "${GRAFT_EDGE:?source tests/am1/lf13/env-m3f.sh}" "${GRAFT_TOOLS_EDGE:?source tests/am1/lf13/env-m3f.sh}"
api() { curl -s -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" "https://api.cloudflare.com/client/v4/accounts/$A$1" "${@:2}"; }
STORE=${CF_SECRETS_STORE_ID:-$(api /secrets_store/stores | jq -r '.result[0].id')}
S="graft-m3f-site-a graft-m3f-site-b graft-m3f-site-c"
WR() { npx --prefix tests/m0 wrangler "$@"; }
scrub() { sed -e "s/$A/<cf-account-id>/g" -e "s/$STORE/<store>/g" -e "s/${CF_WORKERS_SUBDOMAIN:?}/<sub>/g" -e "s#$HOME/#~/#g"; }
live_formats() { api /workers/scripts/graft-m3f-edge/settings | jq -r '.result.bindings[] | select(.name == "FORMATS_ENABLED") | .text'; }
# The digest wrangler printed for the pushed tag.
pushed() { WR containers push "$1" 2>&1 | grep -oE 'sha256:[0-9a-f]{64}' | tail -1; }
{ date -u +%FT%TZ; echo "head $(git rev-parse --short HEAD)"; git status --short -- workers crates containers tests/m1/wp7 | head -5
  git merge-base --is-ancestor b6a44183 HEAD || { echo "HEAD lacks b6a44183, the row's code: stop"; exit 1; }
  [ -d tests/m0/node_modules ] || npm ci --prefix tests/m0 --silent
  if [ "$STEP" = all ] || [ "$STEP" = images ]; then
    docker build -q -t "graft-m3f-gitshard:b024a-$T" containers/git | tail -1
    docker build -q -t "graft-m3f-sshfront:b024a-$T" containers/ssh | tail -1
    G=$(pushed "graft-m3f-gitshard:b024a-$T"); H=$(pushed "graft-m3f-sshfront:b024a-$T")
    [ -n "$G" ] && [ -n "$H" ] || { echo "an image push gave no digest: stop"; exit 1; }
    IMG=registry.cloudflare.com/$A/graft-m3f-gitshard@$G
    SIMG=registry.cloudflare.com/$A/graft-m3f-sshfront@$H
    echo "IMG=$IMG"; echo "SIMG=$SIMG"; date -u +%FT%TZ
  fi
  if [ "$STEP" = all ] || [ "$STEP" = deploy ]; then
    : "${IMG:?}" "${SIMG:?}"
    F=$(live_formats); echo "live FORMATS_ENABLED: $F"
    [ -n "$F" ] || { echo "no live FORMATS_ENABLED: stop"; exit 1; }
    node workers/site/deploy.mjs ~/.graft-m3f/sites.json --test-faults --anchor ~/.graft-m3f/install.toml \
      --image "$IMG" --vars ~/.graft-m3f/site-vars.json --ssh "$STORE" --ssh-image "$SIMG" --edge graft-m3f-edge \
      --export-bucket graft-m3f-export 2>&1 | grep -E "Current Version|Deployed|rror|exit" | tail -8
    echo "site exit ${PIPESTATUS[0]}"; date -u +%FT%TZ
    node tests/m1/integ/deploy-edge.mjs ~/.graft-m3f/sites.json graft-m3f-edge --test-faults \
      --anchor ~/.graft-m3f/install.toml --ops graft-m3f-ops --ssh ~/.graft-m3f/ssh/hostkeys.json \
      --var FORMATS_ENABLED="$F" --export-bucket graft-m3f-export 2>&1 | grep -E "Current Version|Deployed|rror" | tail -4
    echo "edge exit ${PIPESTATUS[0]}"; date -u +%FT%TZ
    echo "now FORMATS_ENABLED: $(live_formats)"
    SITES=~/.graft-m3f/sites.json GENESIS=~/.graft-m3f/genesis-records.json EDGES=graft-m3f-tools-edge \
      node tests/m1/wp7/deploy-edge.mjs 2>&1 | grep -E "Current Version|Deployed|rror" | tail -3
    echo "tools edge exit ${PIPESTATUS[0]}"; date -u +%FT%TZ
    # Crons off: the site day puts `*/15` back; a schedule set then cleared is how the earlier days got `[]` to stick.
    for try in 1 2 3 4; do
      for s in $S; do
        api "/workers/scripts/$s/schedules" -X PUT -H 'content-type: application/json' -d '[{"cron":"0 0 1 1 *"}]' >/dev/null; sleep 3
        api "/workers/scripts/$s/schedules" -X PUT -H 'content-type: application/json' -d '[]' >/dev/null
      done
      left=0; for s in $S; do [ "$(api /workers/scripts/$s/schedules | jq -c .result.schedules)" = "[]" ] || left=1; done
      [ $left = 0 ] && break; sleep 10
    done
    for s in $S graft-m3f-edge graft-m3f-tools-edge; do
      echo "$s $(api /workers/scripts/$s/deployments | jq -c '.result.deployments[0] | [.created_on, [.versions[] | "\(.version_id[0:8]) \(.percentage)"]]') crons $(api /workers/scripts/$s/schedules | jq -c '[.result.schedules[]?.cron]')"
    done
    for s in $S; do echo "$s SSH_KILL=$(api /workers/scripts/$s/settings | jq -r '[.result.bindings[] | select(.name == "SSH_KILL") | .text][0]')"; done
    echo "meta/ssh $(curl -s -m 30 "$GRAFT_EDGE/api/graftyard/v1/meta/ssh" | jq -c '{active, A: [.slots.A[]?.fingerprint]}')"
    TOK=$(cat ~/.graft-m3f/tools.token)
    for site in 0 1 2; do
      echo "ssh_status $site $(curl -s -m 30 -H "authorization: Bearer $TOK" -H 'content-type: application/json' \
        -d "{\"site\":$site}" "$GRAFT_TOOLS_EDGE/ssh_status" | jq -c '{status, running: .v.running, banner: .v.banner, open: .v.open, image: (.v.image // "" | .[-12:])}')"
    done
    unset TOK
  fi
  date -u +%FT%TZ; echo DONE; } 2>&1 | scrub >"$L"
exec sleep infinity
