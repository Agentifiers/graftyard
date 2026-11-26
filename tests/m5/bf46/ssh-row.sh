#!/bin/bash
# BF-46's `SshHostKeys` row on graft-m3f (`build/BUILD-PLAN.md`, M5-WP6; `tests/m5/results/m5-wp6.md`, "BF-46 hosted,
# R45's rows"; the lead's ruling A, 2026-11-24): a `Rotate { SshHostKeys, B }` and back to A, executed by a test-faults
# `graft-m3f-ops` with `OPS_SSH_ANNOUNCE_MS` lowered, while stock `ssh`/`git` sessions run through ssh.graftyard.dev
# (the Spectrum app graft-m3f already serves; nothing in the zone changes). Pass: no new script version across the
# rotation, zero failed sessions, and the host key the sessions see is slot A's before the switch and slot B's after.
# One step per call, each logged to tests/m5/results/raw/bf46ssh/<tag>/<step>.log through `scrub`.
#
#   bash tests/m5/bf46/ssh-row.sh <tag> <step> [args]
#
# Needs `source tests/am1/lf13/env-m3f.sh`, CLOUDFLARE_*, CF_WORKERS_SUBDOMAIN, ~/.graft-m3f with keys2/ (BF-53's
# executor and drain seeds, ruling B) and customer.key, G (the graftyard binary) and RELVIEW (tests/m3/hosted/tools).
# Secret values are read from 0600 files and sent only as request bodies; no step prints one.
#
# Steps:
#   state        deployments, schedules, the `Release` view's count, current intent and slots, meta/ssh
#   image        builds containers/ssh at this head and pushes it as graft-m3f-sshfront:<tag> (prints the digest)
#   day          site day (test-faults, --ssh with SIMG, BF-53's shard image, --anchor, the export bucket) and edge
#                day (live FORMATS_ENABLED); every schedule back to []
#   token        mint graft-m3f-bf46-deploy (Workers Scripts Write, Containers Write) into graft-m3f-bf46-deploy-token
#   keys         graft-m3f-bf46-executor-key and -drain-key in the store from keys2/ (values via files only)
#   ops          graft-m3f-ops test-faults from this head with the executor, 1-minute cron, announce wait ANNOUNCE_MS
#   newslot      a fresh slot B with deploy/ssh-front/hostkeys.mjs (--sites guards against another install's slot);
#                the old B is not kept, since nothing served it
#   probe start|stop  the session prober (`ssh-probe.sh`) as an orb service
#   rotate S N   sign and post `Rotate { S, N }` with the customer key
#   view         the group's entries since the row's start, the current intent and slots
#   versions     every graft-m3f script's deployment and version count since the start (no new version is the pass)
#   teardown     probe off; cron []; graft-m3f-ops back to 3ed6e453 by version id; the row's token and store secrets
#                deleted; slot A active again (checked)
set -uo pipefail
cd "$(dirname "$0")/../../.."
TAG=${1:?tag}; STEP=${2:?step}
O=tests/m5/results/raw/bf46ssh/$TAG; mkdir -p "$O"
W=$HOME/.graft-m3f; K=$W/bf46; mkdir -p "$K"; chmod 700 "$K"
A=${CLOUDFLARE_ACCOUNT_ID:?}
STORE=${CF_SECRETS_STORE_ID:?}
OPS=graft-m3f-ops; VAPI=; OPS_PARKED=${OPS_PARKED:-3ed6e453}
S="graft-m3f-site-a graft-m3f-site-b graft-m3f-site-c"
IMG=registry.cloudflare.com/$A/graft-m3f-gitshard@sha256:7d5c7540889f6256b7f62669e0f29a6dea629f877ad131dae52c8350ba5ab749
B="https://api.cloudflare.com/client/v4/accounts/$A"
api() { curl -s -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" "$B$1" "${@:2}"; }
scrub() { sed -e "s/$A/<cf-account-id>/g" -e "s/$CF_SECRETS_STORE_ID/<store>/g" -e "s/${CF_WORKERS_SUBDOMAIN:?}/<sub>/g" -e "s#$HOME/#~/#g" -e "s#$PWD/#<checkout>/#g"; }
N=$(ls "$O" | grep -c "^$STEP\(-[0-9]*\)\?\.log$" || true)
L="$O/$STEP$( [ "$N" -gt 0 ] && echo "-$((N + 1))" ).log"
log() { { date -u +%FT%TZ; git rev-parse --short HEAD; cat; date -u +%FT%TZ; } | scrub >"$L"; cat "$L"; }
secret_id() { api "/secrets_store/stores/$STORE/secrets?per_page=100" | jq -r --arg n "$1" '.result[] | select(.name == $n) | .id'; }
# put_secret <name> <file holding {"value": ...}>: create or PATCH.
put_secret() {
  local id; id=$(secret_id "$1")
  if [ -n "$id" ]; then
    api "/secrets_store/stores/$STORE/secrets/$id" -X PATCH -H 'content-type: application/json' --data-binary @"$2" |
      jq -c --arg n "$1" '{patch: $n, ok: .success, errors: [.errors[]?.code]}'
  else
    jq -c --arg n "$1" '[{name: $n, value: .value, comment: "BF-46 SshHostKeys row", scopes: ["workers"]}]' "$2" |
      api "/secrets_store/stores/$STORE/secrets" -X POST -H 'content-type: application/json' --data-binary @- |
      jq -c --arg n "$1" '{create: $n, ok: .success, errors: [.errors[]?.code]}'
  fi
}
crons_off() {
  for s in "$@"; do
    api "/workers/scripts/$s/schedules" -X PUT -H 'content-type: application/json' -d '[{"cron":"0 0 1 1 *"}]' >/dev/null; sleep 2
    api "/workers/scripts/$s/schedules" -X PUT -H 'content-type: application/json' -d '[]' >/dev/null
  done
}
dep() { echo "$1 $(api "/workers/scripts/$1/deployments" | jq -c '.result.deployments[0] | [.created_on, [.versions[] | "\(.version_id[0:8]) \(.percentage)"]]') crons $(api "/workers/scripts/$1/schedules" | jq -c '[.result.schedules[]?.cron]')"; }
nversions() { api "/workers/scripts/$1/versions?per_page=100" | jq -r '.result.items | length'; }
meta() { curl -s -m 30 "$GRAFT_EDGE/api/graftyard/v1/meta/ssh" | jq -c '{active, A: [.slots.A[]?.fingerprint], B: [.slots.B[]?.fingerprint]}'; }
view() { node tests/m5/bf46/view.mjs "$@"; }
export BF46_HOME=$W

case $STEP in
state)
  { for s in $S graft-m3f-edge $OPS; do dep "$s"; done
    view slots; echo "meta/ssh $(meta)"
    echo "store: $(api "/secrets_store/stores/$STORE/secrets?per_page=100" | jq -c '{count: (.result | length), bf46: [.result[] | select(.name | startswith("graft-m3f-bf46")) | .name]}')"
  } 2>&1 | log ;;
image)
  { docker build -q -t "graft-m3f-sshfront:$TAG" containers/ssh 2>&1 | tail -1
    npx --prefix tests/m0 wrangler containers push "graft-m3f-sshfront:$TAG" 2>&1 | grep -E "sha256|rror|Pushed|digest" | tail -3
  } 2>&1 | log ;;
day)
  SIMG=${SIMG:?the pushed sshfront image, digest-pinned}
  { echo "shard image ${IMG##*@}; sshfront ${SIMG##*@}"
    F=$(api /workers/scripts/graft-m3f-edge/settings | jq -r '.result.bindings[] | select(.name == "FORMATS_ENABLED") | .text')
    echo "live FORMATS_ENABLED: $F"
    node workers/site/deploy.mjs "$W/sites.json" --test-faults --anchor "$W/install.toml" --image "$IMG" \
      --vars "$W/site-vars.json" --ssh $STORE --ssh-image "$SIMG" --edge graft-m3f-edge \
      --export-bucket graft-m3f-export 2>&1 | grep -E "Current Version|Deployed|rror|exit" | tail -8
    echo "site exit ${PIPESTATUS[0]} $(date -u +%FT%TZ)"
    node tests/m1/integ/deploy-edge.mjs "$W/sites.json" graft-m3f-edge --test-faults \
      --anchor "$W/install.toml" --ops $OPS --ssh "$W/ssh/hostkeys.json" \
      --var FORMATS_ENABLED=$F --export-bucket graft-m3f-export 2>&1 | grep -E "Current Version|Deployed|rror" | tail -4
    echo "edge exit ${PIPESTATUS[0]} $(date -u +%FT%TZ)"
    for try in 1 2 3; do crons_off $S
      left=0; for s in $S; do [ "$(api /workers/scripts/$s/schedules | jq -c .result.schedules)" = "[]" ] || left=1; done
      [ $left = 0 ] && break; sleep 10; done
    for s in $S graft-m3f-edge; do dep "$s"; done
  } 2>&1 | log ;;
token)
  { if ! jq -e .success "$K/deploy.json" >/dev/null 2>&1; then
      ( umask 077
        jq -nc --arg a "com.cloudflare.api.account.$A" '{name: "graft-m3f-bf46-deploy", policies: [{effect: "allow",
          resources: {($a): "*"}, permission_groups: [{id: "e086da7e2179491d91ee5f35b3ca210a"}, {id: "bdbcd690c763475a985e8641dddc09f7"}]}]}' |
          api /tokens -X POST -H 'content-type: application/json' --data-binary @- >"$K/deploy.json" )
    fi
    jq -c '{mint: "graft-m3f-bf46-deploy", ok: .success, id: (.result.id // "" | .[0:8]), errors: [.errors[]?.code]}' "$K/deploy.json"
    ( umask 077; jq -c '{value: .result.value}' "$K/deploy.json" >"$K/deploy.value" )
    put_secret graft-m3f-bf46-deploy-token "$K/deploy.value"
  } 2>&1 | log ;;
keys)
  { for k in executor drain; do
      ( umask 077; jq -Rc '{value: (. | gsub("\\s"; ""))}' "$W/keys2/$k.seed" >"$K/$k.value" )
      put_secret "graft-m3f-bf46-$k-key" "$K/$k.value"; shred -u "$K/$k.value"
    done
  } 2>&1 | log ;;
ops)
  { jq -n --arg sites "$W/sites.json" --arg anchor "$W/install.toml" --argjson ms "${ANNOUNCE_MS:-300000}" '{
      sites: $sites, anchor: $anchor, test_faults: true, gate: "pending", cron: "* * * * *",
      scripts: {"site/0": "graft-m3f-site-a", "site/1": "graft-m3f-site-b", "site/2": "graft-m3f-site-c", edge: "graft-m3f-edge"},
      deploy_token: "graft-m3f-bf46-deploy-token", executor_key: "graft-m3f-bf46-executor-key",
      drain_key: "graft-m3f-bf46-drain-key", ssh_announce_ms: $ms, step_dwell_ms: 60000}' >"$K/exec.json"
    jq -c '{test_faults, gate, cron, ssh_announce_ms, scripts: (.scripts | keys)}' "$K/exec.json"
    (unset CARGO_TARGET_DIR; node workers/ops/deploy.mjs $OPS $STORE graft-m3f-mint-wrap-key --executor "$K/exec.json" 2>&1 |
      grep -E "Current Version|Deployed|rror" | tail -3)
    dep $OPS
  } 2>&1 | log ;;
newslot)
  { node deploy/ssh-front/hostkeys.mjs $STORE "${2:-B}" "$K/hostkeys.json" --sites "$W/sites.json" 2>&1 | tail -2
    jq -c '{slots: (.slots | map_values([.[].fingerprint]))}' "$K/hostkeys.json"
  } 2>&1 | log ;;
probe)
  case ${3:?start|stop} in
    # A service starts without this shell's environment, so it sources the install's env itself.
    start) rm -f "$O/stop"; amp orb service start bf46-ssh-probe --command "cd $PWD && source tests/am1/lf13/env-m3f.sh && exec bash tests/m5/bf46/ssh-probe.sh $PWD/$O/sessions.jsonl $PWD/$O/stop" 2>&1 | head -1 | log ;;
    stop) touch "$O/stop"; sleep 5; amp orb service stop bf46-ssh-probe 2>&1 | tail -1 | log ;;
  esac ;;
rotate)
  SEC=${3:?secret}; NS=${4:?slot}
  { a=$(view prev); echo "prev $(echo "$a" | jq -r .prev | cut -c1-12) count $(echo "$a" | jq -r .count)"
    echo "{\"Intent\": {\"ttl_s\": 21600, \"intent\": {\"Rotate\": {\"secret\": \"$SEC\", \"slot\": $NS}}}}" >"$K/rot.json"
    "$G" --config "$W/install.toml" release entry "$K/rot.json" --out "$K/rot.unsigned" --prev "$(echo "$a" | jq -r .prev)" >/dev/null
    "$G" sign "$K/rot.unsigned" --out "$K/rot.signed" --key "$W/customer.key" >/dev/null
    "$G" --config "$W/install.toml" release post "$K/rot.signed" --edge "$GRAFT_EDGE"
    echo "Rotate $SEC to slot $NS posted"
  } 2>&1 | log ;;
view)
  { [ -s "$O/start.ms" ] || date -u +%s%3N >"$O/start.ms"
    view entries "$(cat "$O/start.ms")"; echo "meta/ssh $(meta)"
  } 2>&1 | log ;;
versions)
  { for s in $S graft-m3f-edge $OPS; do echo "$(dep "$s") versions $(nversions "$s")"; done } 2>&1 | log ;;
teardown)
  { touch "$O/stop"; amp orb service stop bf46-ssh-probe >/dev/null 2>&1
    crons_off $OPS
    # `versions deploy` takes the full version id and `@100%` (m3fs2's teardown: a short id or `@100` exits 1).
    full=$(api "$VAPI/workers/scripts/$OPS/versions?per_page=100" | jq -r --arg p "$OPS_PARKED" '.result.items[] | select(.id | startswith($p)) | .id')
    npx --prefix tests/m0 wrangler versions deploy "$full@100%" --name $OPS -y 2>&1 | grep -E "Deployed|rror" | tail -2
    crons_off $OPS; dep $OPS
    id=$(api "/tokens?per_page=50" | jq -r '.result[] | select(.name == "graft-m3f-bf46-deploy") | .id')
    [ -n "$id" ] && api "/tokens/$id" -X DELETE | jq -c '{revoke: "graft-m3f-bf46-deploy", ok: .success}'
    for n in graft-m3f-bf46-deploy-token graft-m3f-bf46-executor-key graft-m3f-bf46-drain-key; do
      id=$(secret_id "$n"); [ -n "$id" ] && api "/secrets_store/stores/$STORE/secrets/$id" -X DELETE | jq -c --arg n "$n" '{delete: $n, ok: .success}'
    done
    for f in "$K"/deploy.json "$K"/deploy.value; do [ -f "$f" ] && shred -u "$f"; done
    view slots; echo "meta/ssh $(meta)"
  } 2>&1 | log ;;
*) echo "step: state image day token keys ops newslot probe rotate view versions teardown"; exit 2 ;;
esac
