#!/bin/bash
# A17's timing row (decisions/OPEN-QUESTIONS.md, A17; tests/m5/results/m5-wp6.md, "A17: plan"): the time from a
# Secrets Store write to the first read of the new value in a running isolate that read the old one, with no new
# script version. Everything is in the runner account and named graft-a17-: a throwaway secret, the probe Worker
# (tests/m5/a17/probe, which answers a hash of the value, never the value) and its watch objects. No install is used.
#
#   bash tests/m5/a17/row.sh <tag> <step>
#
# Steps, in order: deploy (secret v0, probe, PROBE_TOKEN); watch start (one watch object per location hint, each
# reading every second); sample start (an orb service reading /h every second); put (a new value, PATCHed in place,
# the write's Cloudflare time from the answer's Date header); log (each watch object's changes); versions; then
# sample stop, watch stop, teardown (probe and secret deleted, files shredded). One log per step under
# tests/m5/results/raw/a17/<tag>/. Token values and secret values stay in 0600 files; no step prints one.
set -uo pipefail
cd "$(dirname "$0")/../../.."
TAG=${1:?tag}; STEP=${2:?step}
O=tests/m5/results/raw/a17/$TAG; mkdir -p "$O"
K=$HOME/.graft-a17; mkdir -p "$K"; chmod 700 "$K"
NAME=graft-a17-probe
RB="https://api.cloudflare.com/client/v4/accounts/$CLOUDFLARE_RUNNER_ACCOUNT_ID"
ra() { curl -s -H "Authorization: Bearer $CLOUDFLARE_RUNNER_API_TOKEN" "$@"; }
runner() { CLOUDFLARE_ACCOUNT_ID=$CLOUDFLARE_RUNNER_ACCOUNT_ID CLOUDFLARE_API_TOKEN=$CLOUDFLARE_RUNNER_API_TOKEN "$@"; }
scrub() { sed -e "s/$CLOUDFLARE_RUNNER_ACCOUNT_ID/<runner-account>/g" -e "s/${CF_WORKERS_SUBDOMAIN:-none}/<sub>/g" -e "s#$HOME/#~/#g"; }
N=$(ls "$O" | grep -c "^$STEP" || true)
L="$O/$STEP$( [ "$N" -gt 0 ] && echo "-$((N + 1))" ).log"
log() { { date -u +%FT%TZ; git rev-parse --short HEAD; cat; } | scrub >"$L"; cat "$L"; }
STORE=$(ra "$RB/secrets_store/stores" | jq -r '.result[] | select(.name == "graft-runners") | .id')
URL=https://$NAME.$CF_WORKERS_SUBDOMAIN.workers.dev
secret_id() { ra "$RB/secrets_store/stores/$STORE/secrets?per_page=100" | jq -r --arg n "$NAME" '.result[] | select(.name == $n) | .id'; }
draw() { ( umask 077; python3 -c 'import os; print(os.urandom(24).hex())' | jq -Rc '{value: .}' >"$1" ); }
h16() { jq -r .value "$1" | tr -d '\n' | sha256sum | cut -c1-16; }
# Cloudflare's clock: the orb's ran 28 s behind it on 2026-11-25 (tests/m5/results/raw/bf46r45/r1/clock.txt).
cfnow() { curl -sI https://api.cloudflare.com/client/v4/ | tr -d '\r' | sed -n 's/^[Dd]ate: //p'; }

case $STEP in
deploy)
  { echo "orb minus Cloudflare: $(( $(date +%s) - $(date -d "$(cfnow)" +%s) )) s"
    draw "$K/v0.value"; ( umask 077; python3 -c 'import os; print(os.urandom(24).hex())' >"$K/probe.token" )
    jq -c --arg n "$NAME" '[{name: $n, value: .value, comment: "A17 probe", scopes: ["workers"]}]' "$K/v0.value" |
      ra "$RB/secrets_store/stores/$STORE/secrets" -X POST -H 'content-type: application/json' --data-binary @- |
      jq -c '{create: "graft-a17-probe", ok: .success, errors: [.errors[]?.code]}'
    echo "v0 sha256 $(h16 "$K/v0.value")"
    jq --arg s "$STORE" '.secrets_store_secrets[0].store_id = $s' tests/m5/a17/probe/wrangler.json >tests/m5/a17/probe/.wrangler.json
    (cd tests/m5/a17/probe && runner npx --prefix ../../../m0 wrangler deploy -c .wrangler.json 2>&1 | grep -E "Current Version|rror"
     runner npx --prefix ../../../m0 wrangler secret put PROBE_TOKEN --name $NAME <"$K/probe.token" 2>&1 | tail -1)
    rm -f tests/m5/a17/probe/.wrangler.json
    ra "$RB/workers/scripts/$NAME/deployments" | jq -c '.result.deployments[0] | {created_on, versions: [.versions[] | "\(.version_id[0:8]) \(.percentage)"]}'
  } 2>&1 | log ;;
watch)
  { for h in enam weur apac; do
      echo "$h $(curl -s -H "authorization: Bearer $(cat "$K/probe.token")" "$URL/w/$h/${3:?start|stop}")"
    done; } 2>&1 | log ;;
sample)
  case ${3:?start|stop} in
    start) rm -f "$O/stop"
      amp orb service start a17-sample --command "cd $PWD && exec bash -c 'while [ ! -e $PWD/$O/stop ]; do curl -s -m 5 -H \"authorization: Bearer \$(cat $K/probe.token)\" $URL/h; echo; sleep 1; done >>$PWD/$O/sample.jsonl'" 2>&1 | head -1 | log ;;
    stop) touch "$O/stop"; sleep 3; amp orb service stop a17-sample 2>&1 | tail -1 | log ;;
  esac ;;
put)
  # The runbook's in-place write: PATCH over the one slot, as `HostedRunnersAppKey` is rotated.
  { n=$(ls "$K"/v*.value | wc -l); draw "$K/v$n.value"
    t0=$(date +%s%3N)
    hdr=$(mktemp); ra -D "$hdr" "$RB/secrets_store/stores/$STORE/secrets/$(secret_id)" -X PATCH -H 'content-type: application/json' \
      --data-binary @"$K/v$n.value" | jq -c --arg n "v$n" '{patch: $n, ok: .success, errors: [.errors[]?.code]}'
    t1=$(date +%s%3N)
    echo "v$n sha256 $(h16 "$K/v$n.value") written: orb $t0..$t1 ms, answer Date $(tr -d '\r' <"$hdr" | sed -n 's/^[Dd]ate: //p')"
    rm -f "$hdr"
    echo "orb minus Cloudflare: $(( $(date +%s) - $(date -d "$(cfnow)" +%s) )) s"
  } 2>&1 | log ;;
log)
  { for h in enam weur apac; do
      echo "$h $(curl -s -H "authorization: Bearer $(cat "$K/probe.token")" "$URL/w/$h/log")"
    done; } 2>&1 | log ;;
versions)
  { ra "$RB/workers/scripts/$NAME/versions?per_page=100" | jq -c '{versions: (.result.items | length)}'
    ra "$RB/workers/scripts/$NAME/deployments" | jq -c '.result.deployments[0] | {created_on, versions: [.versions[] | "\(.version_id[0:8]) \(.percentage)"]}'
  } 2>&1 | log ;;
teardown)
  { touch "$O/stop"; amp orb service stop a17-sample >/dev/null 2>&1
    runner npx --prefix tests/m0 wrangler delete --name $NAME --force 2>&1 | tail -1
    id=$(secret_id); [ -n "$id" ] && ra "$RB/secrets_store/stores/$STORE/secrets/$id" -X DELETE | jq -c '{delete: "graft-a17-probe", ok: .success}'
    for f in "$K"/*; do [ -f "$f" ] && shred -u "$f"; done; rmdir "$K"; echo "files shredded"
    ra "$RB/workers/scripts" | jq -c '{scripts: [.result[].id]}'
    ra "$RB/secrets_store/stores/$STORE/secrets?per_page=100" | jq -c '{store: [.result[].name]}'
  } 2>&1 | log ;;
*) echo "step: deploy watch sample put log versions teardown"; exit 2 ;;
esac
