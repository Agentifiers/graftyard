#!/bin/bash
# LM-3's "a Sandbox killed mid-task resumes" on graft-m4a (`build/BUILD-PLAN.md`, AM3-WP1, the kill row; the lead's
# rulings, 2026-11-25). The runner account stands in as the agents account, every resource there named
# `graft-agents-m4a-`. The agents half reuses `tests/am3/hosted/lf7-day.sh` with ROW=kill TB=1; this script adds the
# install half, the hook, the probe, the row, and a teardown that fails loudly on any running container. One step per
# call, each logged to tests/am3/raw/kill-<tag>/<step>.log through lf7-day.sh's `scrub`.
#
#   bash tests/am3/hosted/kill-row.sh <tag> <step>
#
# Steps:
#   state | sweep | init | register | keys | infra | front | ops | bundles | roll <R> | abort <R> | view | status | logs
#                  lf7-day.sh's, as ROW=kill TB=1 (`controller` and `bundles` build the `TaskBox` controller with
#                  test-faults; `infra` mints the agents deploy token with Secrets Store Write and Containers Write, A54)
#   taskbox        $K/taskbox.json for `controller`: the App ids, the runner store, the harness image
#                  (tests/am3/hosted/kill/Dockerfile), the install's host, the harness's command, and the secret names
#                  `agents init` gave (OPEN-QUESTIONS A58)
#   controller     lf7-day.sh's, with the `TaskBox` (`--test-faults --taskbox`), then again with AGENTS_FRONT once the
#                  front exists (the two scripts bind each other)
#   install        graftyard-agent and graftyard-workspace installed on m3org, agents enabled in-repo (kill.mjs)
#   hook           graftyard-agent's webhook: the front's URL and the row's webhook secret, through `PATCH
#                  /app/hook/config` from the probe Worker (lm3-probe.js `/hook`), which holds the App's key
#   repo           m3org/kill-<tag>, created with a first commit
#   smoke          kill.mjs smoke: one task through the same path, before the row
#   row            kill.mjs row: 10 tasks, a kill each at its planned point, the late replays, every task to its end
#   teardown       m3org/kill-<tag> deleted; lf7-day.sh's teardown (every graft-agents-m4a- script, secret, token,
#                  bucket); then every container application of graft-agents-m4a- listed with its running instances,
#                  failing loudly while any runs; the install's shard sweep
#   cost           graft-m4a and the runner account's graft-agents-m4a- spend since the row's start
set -uo pipefail
cd "$(dirname "$0")/../../.."
TAG=${1:?tag}; STEP=${2:?step}
export ROW=kill TB=1
O=tests/am3/raw/kill-$TAG; mkdir -p "$O"
W=$HOME/.graft-m4a; K=$W/kill; mkdir -p "$K"; chmod 700 "$K"
P=graft-agents-m4a; CTRL=$P-controller; FRONT=$P-front; PROBE=$P-probe
RSTORE=$(cat "$HOME/.graft-m4a/runner-store")  # the runner account's store id, kept off the branch
EDGE_HOST=graft-m4a-edge.$CF_WORKERS_SUBDOMAIN.workers.dev
FRONT_URL=https://$FRONT.$CF_RUNNER_WORKERS_SUBDOMAIN.workers.dev
PROBE_URL=https://$PROBE.$CF_RUNNER_WORKERS_SUBDOMAIN.workers.dev
RB="https://api.cloudflare.com/client/v4/accounts/$CLOUDFLARE_RUNNER_ACCOUNT_ID"
rapi() { curl -s -H "Authorization: Bearer $CLOUDFLARE_RUNNER_API_TOKEN" "$1" "${@:2}"; }
runner() { CLOUDFLARE_ACCOUNT_ID=$CLOUDFLARE_RUNNER_ACCOUNT_ID CLOUDFLARE_API_TOKEN=$CLOUDFLARE_RUNNER_API_TOKEN "$@"; }
scrub() {
  sed -e "s/$CLOUDFLARE_ACCOUNT_ID/<install-account>/g" -e "s/$CLOUDFLARE_RUNNER_ACCOUNT_ID/<runner-account>/g" \
      -e "s/$CF_WORKERS_SUBDOMAIN/<sub>/g" -e "s/$CF_RUNNER_WORKERS_SUBDOMAIN/<runner-sub>/g" \
      -e "s/$RSTORE/<runner-store>/g" -e "s#$PWD/#<checkout>/#g" -e "s#$HOME/#~/#g"
}
N=$(ls "$O" | grep -c "^$STEP" || true)
L="$O/$STEP$( [ "$N" -gt 0 ] && echo "-$((N + 1))" ).log"
log() { { date -u +%FT%TZ; git rev-parse --short HEAD; cat; } | scrub >"$L"; cat "$L"; }
day() { bash tests/am3/hosted/lf7-day.sh "$TAG" "$@"; }
# Container applications of graft-agents-m4a- and their running instances, as JSON lines.
containers() {
  rapi "$RB/containers/applications" | jq -r --arg p "$P-" '.result[]? | select(.name | startswith($p)) | "\(.id) \(.name)"' |
    while read -r id name; do
      n=$(rapi "$RB/containers/applications/$id/instances?per_page=1000" | jq '[.result.instances[]? // .result[]? | select(.status.state == "running" or .state == "running")] | length')
      echo "{\"app\": \"$name\", \"running\": ${n:-0}}"
    done
}

case $STEP in
state | sweep | init | register | keys | infra | front | ops | bundles | roll | abort | view | status | logs | block | unblock)
  day "$STEP" "${@:3}" ;;
taskbox)
  { read -r app ws <"$K/app-ids"
    # The harness image is built here with the legacy builder and pushed to the runner account's registry: the orb's
    # docker has no buildx, which wrangler's own Dockerfile build needs (`--load`).
    (cd tests/am3/hosted/kill && DOCKER_BUILDKIT=0 docker build -q -t $P-sandbox:kill-$TAG . >/dev/null) &&
      runner npx --prefix tests/m0 wrangler containers push $P-sandbox:kill-$TAG 2>&1 | grep -E "digest|Pushed image" | tail -2
    dig=$(docker image inspect $P-sandbox:kill-$TAG --format '{{index .RepoDigests 0}}' 2>/dev/null | sed 's/.*@//')
    [ -n "$dig" ] || dig=$(docker image inspect $P-sandbox:kill-$TAG --format '{{.Id}}')
    img=registry.cloudflare.com/$CLOUDFLARE_RUNNER_ACCOUNT_ID/$P-sandbox@$dig
    jq -n --arg a "$app" --arg w "$ws" --arg store $RSTORE --arg host "$EDGE_HOST" --arg p "$P-" --arg img "$img" \
      '{agent_app_id: ($a|tonumber), workspace_app_id: ($w|tonumber), store: $store,
        image: $img, host: $host,
        harness: ["python3", "/harness.py", $host, "m3org/kill-'"$TAG"'", "8", "15"],
        secrets: {AGENT_APP_KEY: "\($p)AGENTS_APP_KEY_A", WORKSPACE_APP_KEY: "\($p)WORKSPACE_APP_KEY_A", MODEL_KEY: null}}' \
      >"$K/taskbox.json"
    [ -n "${FRONTB:-}" ] && jq --arg f $FRONT '. + {agents_front: $f}' "$K/taskbox.json" >"$K/tb.tmp" && mv "$K/tb.tmp" "$K/taskbox.json"
    jq -c . "$K/taskbox.json"
  } 2>&1 | log ;;
controller)
  # The image dir is the build context: deploy.mjs resolves the Dockerfile against the repository root.
  day controller ;;
install)
  { node tests/am3/hosted/kill.mjs install "$K/agents" "$K/app-ids"; echo "install exit $?"; } 2>&1 | log ;;
hook)
  { read -r app _ <"$K/app-ids"
    ( umask 077; jq -Rc --arg u "$FRONT_URL/" '{url: $u, secret: ., content_type: "json"}' <"$K/webhook.secret" >"$K/hook.body" )
    curl -s -m 60 -X POST -H "x-probe-token: $(cat "$K/probe.token")" -H 'content-type: application/json' \
      --data-binary @"$K/hook.body" "$PROBE_URL/hook" | jq -c '{status, url: .body.url, secret: .body.secret, events: .body.events}'
    shred -u "$K/hook.body"
  } 2>&1 | log ;;
probe-deploy)
  day probe-deploy ;;
repo)
  { node tests/am3/hosted/kill.mjs repo "kill-$TAG"; echo "repo exit $?"; } 2>&1 | log ;;
row | smoke)
  { [ -s "$O/row.from" ] || date -u +%s >"$O/row.from"
    node tests/am3/hosted/kill.mjs "$STEP" "$O" "kill-$TAG" "$K/ctrl.token" "$FRONT_URL"
    echo "row exit $?"
  } 2>&1 | log ;;
teardown)
  { node tests/am3/hosted/kill.mjs delete "kill-$TAG"
    day teardown | tail -6
    for i in 1 2 3 4 5 6; do
      left=$(containers | jq -s '[.[] | select(.running > 0)]')
      [ "$left" = "[]" ] && break
      sleep 10
    done
    echo "agents container applications: $(containers | jq -sc .)"
    rapi "$RB/containers/applications" | jq -r --arg p "$P-" '.result[]? | select(.name | startswith($p)) | .id' |
      while read -r id; do rapi "$RB/containers/applications/$id" -X DELETE | jq -c '{delete_app: .success, errors: [.errors[]?.code]}'; done
    if [ "$left" != "[]" ]; then echo "TEARDOWN FAILED: agents containers still running: $left"; fi
    python3 tests/fanout/teardown.py --sweep 2>&1 | tail -2
    echo "agents apps after: $(rapi "$RB/containers/applications" | jq -c --arg p "$P-" '[.result[]? | select(.name | startswith($p)) | .name]')"
  } 2>&1 | log
  grep -q "TEARDOWN FAILED\|SWEEP FAILED" "$L" && { echo "FAIL: containers left running" >&2; exit 2; } ;;
cost)
  { from=$(date -u -d @"$(cat "$O/cost.from")" +%FT%TZ)
    echo "graft-m4a from $from: $(PREFIX=graft-m4a python3 tests/fanout/acctcost.py "$from" | head -1 | jq -c '{prefix_usd}')"
    echo "graft-m4a day: $(PREFIX=graft-m4a python3 tests/fanout/acctcost.py "$(date -u +%Y-%m-%dT00:00:00Z)" | head -1 | jq -c '{prefix_usd}')"
    echo "runner $P- from $from: $(runner env PREFIX=$P python3 tests/fanout/acctcost.py "$from" | head -1 | jq -c '{prefix_usd}')"
  } 2>&1 | log ;;
*) echo "step: state sweep init register keys infra taskbox controller front ops bundles roll abort install probe-deploy hook repo row teardown cost"; exit 2 ;;
esac
