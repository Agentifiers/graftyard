#!/bin/bash
# LM-3's row A on graft-m4a (`tests/am3/results/lm3-gate.md`, "Row A"; the lead's GO, 2026-11-26): L0-02's containment
# through the product router, then L0-03's token scopes. A copy of `kill-row.sh`: the agents half (`init` to `bundles`)
# is `lf7-day.sh`'s with ROW=gate TB=1 (state under ~/.graft-m4a/gate), `install` and `hook` are kill-row.sh's, and this
# script adds the stand-ins, the two images, the two phases, and the teardown with its container sweeps. One step per
# call, each logged to tests/am3/raw/gate-<tag>/<step>.log through `scrub`. Spend stops at $2.00 (STOP_USD) across
# graft-m4a and the runner account's `graft-agents-m4a-`, read before every step that starts anything.
#
#   bash tests/am3/hosted/gate-row.sh <tag> <step>
#
# Steps:
#   state | sweep | init | register | keys | infra | front | ops | bundles | probe-deploy | view | status | logs
#                   lf7-day.sh's, with ROW=gate TB=1 (the agents half and the probe, AM2's `POST /mint` included)
#   install | hook  kill-row.sh's: both Apps installed on m3org (kill.mjs), graftyard-agent's webhook to the front
#   standins        graft-agents-m4a-canary, -model, -registry on the runner account (tests/m0/src/wp14's scripts,
#                   renamed), each counting every arrival; MODEL_KEY a random value for the row, put in the runner store
#                   and on -model; M0_TOKEN for the counters, in a 0600 file
#   repos           m3org/gate-<tag>, m3org/gate-other-<tag>, and m3-admin's pull request (gate.mjs repos)
#   image           tests/am3/hosted/gate/gate.json (no secret) and the L0-02 image, pushed to the runner registry;
#                   $K/taskbox.json with HARNESS = the adapted harness and ROUTE_OTHERS = the model and registry arms
#   controller      lf7-day.sh's controller with that taskbox (`--test-faults --taskbox`); again with FRONTB=1 once
#                   the front exists
#   l002            the stand-ins' counters zeroed; one prompt-only task on m3org/gate-<tag>; polls the task's
#                   `results` branch for the harness's class files (gate.mjs results); reads each counter; writes
#                   $O/l002.json with the verdict; completes the task
#   l003-setup      the controller again with kill-row's harness image in hold mode (AM2's `harness.py`, which holds
#                   while refs/heads/hold/<task> exists), no model arm; tasks A, B and C, their holds; C's workspace
#                   token minted while C lives; C completed
#   l003 <pass>     gate.mjs l003 for `shipped` or `widened`
#   l003-release    the holds deleted, so A and B report Finished
#   teardown        both repositories deleted; both installations removed; the stand-ins deleted; lf7-day.sh's teardown
#                   (every graft-agents-m4a- script, the probe included, secret, token, bucket); every container
#                   application of graft-agents-m4a- listed, failing loudly while any instance runs, then deleted; the
#                   install's shard sweep and shard kill (teardown.py --sweep --shardkill); the row's token files shredded
#   cost            graft-m4a's and the runner account's graft-agents-m4a- spend since the row's start
set -uo pipefail
cd "$(dirname "$0")/../../.."
TAG=${1:?tag}; STEP=${2:?step}
: "${CF_SECRETS_STORE_ID:?export CF_SECRETS_STORE_ID first (lf7-day.sh reads it)}"
export ROW=gate TB=1
O=tests/am3/raw/gate-$TAG; mkdir -p "$O"
W=$HOME/.graft-m4a; K=$W/gate; mkdir -p "$K"; chmod 700 "$K"
P=graft-agents-m4a; CTRL=$P-controller; FRONT=$P-front; PROBE=$P-probe
RSTORE=$(cat "$HOME/.graft-m4a/runner-store")  # the runner account's store id, kept off the branch
EDGE_HOST=graft-m4a-edge.$CF_WORKERS_SUBDOMAIN.workers.dev
FRONT_URL=https://$FRONT.$CF_RUNNER_WORKERS_SUBDOMAIN.workers.dev
PROBE_URL=https://$PROBE.$CF_RUNNER_WORKERS_SUBDOMAIN.workers.dev
STAND=https://$P-STANDIN.$CF_RUNNER_WORKERS_SUBDOMAIN.workers.dev
RB="https://api.cloudflare.com/client/v4/accounts/$CLOUDFLARE_RUNNER_ACCOUNT_ID"
STOP_USD=${STOP_USD:-2.00}
rapi() { curl -s -H "Authorization: Bearer $CLOUDFLARE_RUNNER_API_TOKEN" "$1" "${@:2}"; }
runner() { CLOUDFLARE_ACCOUNT_ID=$CLOUDFLARE_RUNNER_ACCOUNT_ID CLOUDFLARE_API_TOKEN=$CLOUDFLARE_RUNNER_API_TOKEN "$@"; }
scrub() {
  sed -e "s/$CLOUDFLARE_ACCOUNT_ID/<install-account>/g" -e "s/$CLOUDFLARE_RUNNER_ACCOUNT_ID/<runner-account>/g" \
      -e "s/$CF_WORKERS_SUBDOMAIN/<sub>/g" -e "s/$CF_RUNNER_WORKERS_SUBDOMAIN/<runner-sub>/g" \
      -e "s/$RSTORE/<runner-store>/g" -e "s/$CF_SECRETS_STORE_ID/<install-store>/g" -e "s#$PWD/#<checkout>/#g" -e "s#$HOME/#~/#g"
}
N=$(ls "$O" | grep -c "^$STEP" || true)
L="$O/$STEP$( [ "$N" -gt 0 ] && echo "-$((N + 1))" ).log"
log() { { date -u +%FT%TZ; git rev-parse --short HEAD; cat; } | scrub >"$L"; cat "$L"; }
day() { bash tests/am3/hosted/lf7-day.sh "$TAG" "$@"; }
gate() { node tests/am3/hosted/gate.mjs "$@"; }
# Spend since the row's start, both accounts; fails past STOP_USD.
spend() {
  [ -s "$O/cost.from" ] || date -u +%s >"$O/cost.from"
  local from a r
  from=$(date -u -d @"$(cat "$O/cost.from")" +%FT%TZ)
  a=$(PREFIX=graft-m4a python3 tests/fanout/acctcost.py "$from" | head -1 | jq -r '.prefix_usd // 0')
  r=$(runner env PREFIX=$P python3 tests/fanout/acctcost.py "$from" | head -1 | jq -r '.prefix_usd // 0')
  echo "spend since $from: graft-m4a \$$a, runner $P- \$$r"
  if python3 -c "import sys; sys.exit(0 if $a + $r < $STOP_USD else 1)"; then return 0; fi
  echo "STOP: spend \$$(python3 -c "print(round($a + $r, 2))") is past \$$STOP_USD; run teardown"; return 1
}
# Container applications of graft-agents-m4a- and their running instances, as JSON lines (kill-row.sh's).
containers() {
  rapi "$RB/containers/applications" | jq -r --arg p "$P-" '.result[]? | select(.name | startswith($p)) | "\(.id) \(.name)"' |
    while read -r id name; do
      n=$(rapi "$RB/containers/applications/$id/instances?per_page=1000" | jq '[.result.instances[]? // .result[]? | select(.status.state == "running" or .state == "running")] | length')
      echo "{\"app\": \"$name\", \"running\": ${n:-0}}"
    done
}
# A stand-in's counters (`GET /m0/hits`).
hits() { curl -s -m 30 -H "authorization: Bearer $(cat "$K/m0.token")" "${STAND/STANDIN/$1}/m0/hits"; }
# Builds <dir>'s image as graft-agents-m4a-sandbox:<name>-<tag>, pushes it to the runner registry, and writes
# $K/taskbox.json with the harness <harness json>, the arms <route_others json> and MODEL_KEY's secret (or null), as
# kill-row.sh's `taskbox` (the orb's docker has no buildx, so the legacy builder). FRONTB=1 adds AGENTS_FRONT.
taskbox() {
  local dir=$1 name=$2 harness=$3 others=$4 model=$5 app ws dig img
  read -r app ws <"$K/app-ids"
  (cd "$dir" && DOCKER_BUILDKIT=0 docker build -q -t $P-sandbox:$name-$TAG . >/dev/null) &&
    runner npx --prefix tests/m0 wrangler containers push $P-sandbox:$name-$TAG 2>&1 | grep -E "digest|Pushed image" | tail -2
  dig=$(docker image inspect $P-sandbox:$name-$TAG --format '{{index .RepoDigests 0}}' 2>/dev/null | sed 's/.*@//')
  [ -n "$dig" ] || dig=$(docker image inspect $P-sandbox:$name-$TAG --format '{{.Id}}')
  img=registry.cloudflare.com/$CLOUDFLARE_RUNNER_ACCOUNT_ID/$P-sandbox@$dig
  jq -n --arg a "$app" --arg w "$ws" --arg store $RSTORE --arg host "$EDGE_HOST" --arg p "$P-" --arg img "$img" \
    --argjson harness "$harness" --argjson others "$others" --argjson model "$model" \
    '{agent_app_id: ($a|tonumber), workspace_app_id: ($w|tonumber), store: $store, image: $img, host: $host,
      harness: $harness, route_others: $others,
      secrets: {AGENT_APP_KEY: "\($p)AGENTS_APP_KEY_A", WORKSPACE_APP_KEY: "\($p)WORKSPACE_APP_KEY_A", MODEL_KEY: $model}}' \
    >"$K/taskbox.json"
  [ -n "${FRONTB:-}" ] && jq --arg f $FRONT '. + {agents_front: $f}' "$K/taskbox.json" >"$K/tb.tmp" && mv "$K/tb.tmp" "$K/taskbox.json"
  jq -c '{image, harness, route_others, agents_front}' "$K/taskbox.json"
}

case $STEP in
state)
  { spend || true; day state | tail -4; } 2>&1 | log ;;
sweep | init | register | keys | infra | front | ops | bundles | view | status | logs | probe-deploy)
  spend >/dev/null || exit 3
  day "$STEP" "${@:3}" ;;
install)
  spend >/dev/null || exit 3
  { node tests/am3/hosted/kill.mjs install "$K/agents" "$K/app-ids"; echo "install exit $?"; } 2>&1 | log ;;
hook)
  { ( umask 077; jq -Rc --arg u "$FRONT_URL/" '{url: $u, secret: ., content_type: "json"}' <"$K/webhook.secret" >"$K/hook.body" )
    curl -s -m 60 -X POST -H "x-probe-token: $(cat "$K/probe.token")" -H 'content-type: application/json' \
      --data-binary @"$K/hook.body" "$PROBE_URL/hook" | jq -c '{status, url: .body.url, secret: .body.secret, events: .body.events}'
    shred -u "$K/hook.body"
  } 2>&1 | log ;;
standins)
  spend || exit 3
  { ( umask 077; [ -s "$K/m0.token" ] || python3 -c 'import os; print(os.urandom(24).hex())' >"$K/m0.token"
      [ -s "$K/model.key" ] || python3 -c 'import os; print("rowa-" + os.urandom(24).hex())' >"$K/model.key" )
    for s in canary model registry; do
      mkdir -p "$K/$s"
      sed -e "s/graft-wp14-$s/$P-$s/" -e "s#\"\\.\\./src/wp14/$s.ts\"#\"$PWD/tests/m0/src/wp14/$s.ts\"#" \
        tests/m0/wp14/graft-wp14-$s.jsonc >"$K/$s/wrangler.jsonc"
      (cd "$K/$s" && runner npx --prefix "$OLDPWD/tests/m0" wrangler deploy -c wrangler.jsonc 2>&1 | grep -E "Current Version|rror" | tail -2)
      runner npx --prefix tests/m0 wrangler secret put M0_TOKEN --name $P-$s <"$K/m0.token" 2>&1 | tail -1
    done
    runner npx --prefix tests/m0 wrangler secret put MODEL_KEY --name $P-model <"$K/model.key" 2>&1 | tail -1
    # The row's model key in the runner store, where the TaskBox's MODEL_KEY binding reads it.
    ( umask 077; jq -Rc '{name: "'$P'-MODEL_KEY_A", value: ., scopes: ["workers"]}' <"$K/model.key" >"$K/model.body" )
    rapi "$RB/secrets_store/stores/$RSTORE/secrets" -X POST -H 'content-type: application/json' --data-binary "[$(cat "$K/model.body")]" |
      jq -c '{store_put: .success, errors: [.errors[]?.code]}'
    shred -u "$K/model.body"
    for s in canary model registry; do
      curl -s -m 30 -X POST -H "authorization: Bearer $(cat "$K/m0.token")" "${STAND/STANDIN/$s}/m0/hits/reset" >/dev/null
      echo "$s hits: $(hits $s)"
    done
  } 2>&1 | log ;;
repos)
  spend >/dev/null || exit 3
  { gate repos "$TAG"; echo "repos exit $?"; } 2>&1 | log ;;
image)
  spend >/dev/null || exit 3
  { canary=$P-canary.$CF_RUNNER_WORKERS_SUBDOMAIN.workers.dev
    model=$P-model.$CF_RUNNER_WORKERS_SUBDOMAIN.workers.dev
    reg=$P-registry.$CF_RUNNER_WORKERS_SUBDOMAIN.workers.dev
    # The harness's config, as tests/m0/run/wp14.ts builds it; no secret. The in-repo push target is the base, so
    # "push to another repository" targets m3org/gate-other-<tag>; `prefix` is the stem the harness adds its task to.
    python3 - "$canary" "$EDGE_HOST" "$model" "$reg" "$TAG" >tests/am3/hosted/gate/gate.json <<'PY'
import json, socket, sys
canary, host, model, reg, tag = sys.argv[1:]
v4 = sorted({a[4][0] for a in socket.getaddrinfo(canary, 443, socket.AF_INET)})
v6 = sorted({a[4][0] for a in socket.getaddrinfo(canary, 443, socket.AF_INET6)})
hv4 = sorted({a[4][0] for a in socket.getaddrinfo(host, 443, socket.AF_INET)})
print(json.dumps({"ca": "/etc/cloudflare/certs/cloudflare-containers-ca.crt", "hostName": host, "modelHost": model,
    "registryHost": reg, "canaryHost": canary, "base": f"m3org/gate-{tag}", "pushTarget": f"m3org/gate-{tag}",
    "other": f"m3org/gate-other-{tag}", "modelPath": "/v1/chat/completions",
    "knownGoModule": "github.com/graftyard/known", "prefix": "refs/heads/graftyard/",
    "canaryV4": v4[0], "canaryV4All": v4, "canaryV6": v6[0] if v6 else "2001:db8::2", "hostV4": hv4[0], "n": 100}, indent=1))
PY
    taskbox tests/am3/hosted/gate gate '["python3", "/attempts.py", "/gate.json"]' \
      "$(jq -nc --arg m "$model" --arg r "$reg" '[{ModelInference: {host: $m, path: "/v1/chat/completions"}},
        {Registry: {host: $r, go_modules: ["github.com/graftyard/known"]}}]')" "\"$P-MODEL_KEY_A\""
  } 2>&1 | log ;;
controller)
  spend >/dev/null || exit 3
  day controller ;;
l002)
  spend || exit 3
  { [ -s "$O/row.from" ] || date -u +%s >"$O/row.from"
    for s in canary model registry; do
      curl -s -m 30 -X POST -H "authorization: Bearer $(cat "$K/m0.token")" "${STAND/STANDIN/$s}/m0/hits/reset" >/dev/null
    done
    t=$(gate task "gate-$TAG" l002 | tail -1); echo "$t"
    id=$(echo "$t" | jq -r .id)
    # The harness pushes its class files to graftyard/<id>/results once every class is written (M0's took ~25 minutes
    # for 800 attempts); poll for 45 minutes, stopping the step past the spend cap.
    for i in $(seq 1 90); do
      gate results "gate-$TAG" "$id" "$O" 2>&1 | tail -1 && [ -s "$O/l002-classes.json" ] && break
      [ $((i % 10)) -eq 0 ] && { spend || break; }
      sleep 30
    done
    for s in canary model registry; do echo "$s hits: $(hits $s)"; done | tee "$O/l002.hits"
    python3 - "$O" <<'PY'
import json, sys
o = sys.argv[1]
try:
    classes = json.load(open(f"{o}/l002-classes.json"))
except FileNotFoundError:
    classes = {}
pos = classes.pop("_positives", {}).get("positives", [])
hits = {}
for line in open(f"{o}/l002.hits"):
    name, _, v = line.partition(" hits: ")
    try: hits[name] = json.loads(v or "{}")
    except ValueError: hits[name] = {"unread": v.strip()}
rows = {k: {"attempts": v["attempts"], "succeeded": v["succeeded"], "how": v["how"]} for k, v in classes.items()}
succeeded = sum(c["succeeded"] for c in rows.values())
attempts = sum(c["attempts"] for c in rows.values())
canary = sum(n for n in hits.get("canary", {}).values() if isinstance(n, int))
ok = len(rows) == 8 and attempts == 800 and succeeded == 0 and len(pos) == 4 and all(p.get("ok") for p in pos) and canary == 0
v = {"attempts": attempts, "succeeded": succeeded, "classes": rows, "positives": pos, "hits": hits,
     "verdict": "PASS" if ok else "FAIL"}
json.dump(v, open(f"{o}/l002.json", "w"), indent=1)
print(f"L0-02: {v['verdict']}: {succeeded} of {attempts} succeeded over {len(rows)} classes, "
      f"controls ok {[p.get('check') for p in pos if p.get('ok')]}, canary arrivals {canary}")
PY
    gate complete "gate-$TAG" "$id"
  } 2>&1 | log ;;
l003-setup)
  spend || exit 3
  { # kill-row's harness in hold mode (AM2's `harness.py` from am3-rest: it holds while refs/heads/hold/<task> exists).
    taskbox tests/am3/hosted/kill hold "[\"python3\", \"/harness.py\", \"$EDGE_HOST\", \"m3org/gate-$TAG\", \"1\", \"15\"]" '[]' null
    day controller | tail -2
    for l in A B C; do gate task "gate-$TAG" "$l" | tail -1; done | tee "$O/tasks.json.lines"
    jq -s . "$O/tasks.json.lines" >"$O/tasks.json"
    for id in $(jq -r '.[].id' "$O/tasks.json"); do gate hold "gate-$TAG" "$id"; done
    c=$(jq -r '.[] | select(.label == "C") | .id' "$O/tasks.json")
    # C's workspace token while C lives, through the probe's /mint, kept 0600 for gate.mjs and shredded at teardown.
    ( umask 077; jq -nc --arg r "gate-$TAG" --arg t "$c" \
        '{app: "workspace", installation: 7190004, body: {repositories: [$r], graftyard_task: $t}}' |
      curl -s -m 60 -X POST -H "x-probe-token: $(cat "$K/probe.token")" \
        -H 'content-type: application/json' --data-binary @- "$PROBE_URL/mint" | jq -r '.body.token // empty' >"$K/c.token" )
    [ -s "$K/c.token" ] && echo "C's token held" || echo "STOP: C's token did not mint"
    gate release "gate-$TAG" "$c"; gate complete "gate-$TAG" "$c"
  } 2>&1 | log ;;
l003)
  spend || exit 3
  pass=${3:?shipped or widened}
  { gate l003 "$O" "$TAG" "$pass" "$PROBE_URL" "$K/probe.token" "$O/tasks.json"
    echo "l003 $pass exit $?"
  } 2>&1 | log ;;
l003-release)
  { for id in $(jq -r '.[] | select(.label != "C") | .id' "$O/tasks.json"); do gate release "gate-$TAG" "$id"; done; } 2>&1 | log ;;
teardown)
  { gate delete "$TAG"
    node tests/am3/hosted/kill.mjs uninstall
    for s in canary model registry; do runner npx --prefix tests/m0 wrangler delete --name $P-$s --force 2>&1 | tail -1; done
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
    python3 tests/fanout/teardown.py --sweep --shardkill 2>&1 | tail -2
    for f in "$K"/m0.token "$K"/model.key "$K"/c.token; do [ -f "$f" ] && shred -u "$f"; done
    echo "row token files shredded"
    echo "agents apps after: $(rapi "$RB/containers/applications" | jq -c --arg p "$P-" '[.result[]? | select(.name | startswith($p)) | .name]')"
  } 2>&1 | log
  if grep -q "TEARDOWN FAILED\|SWEEP FAILED" "$L"; then echo "FAIL: containers left running" >&2; exit 2; fi ;;
cost)
  { from=$(date -u -d @"$(cat "$O/cost.from")" +%FT%TZ)
    echo "graft-m4a from $from: $(PREFIX=graft-m4a python3 tests/fanout/acctcost.py "$from" | head -1 | jq -c '{prefix_usd}')"
    echo "runner $P- from $from: $(runner env PREFIX=$P python3 tests/fanout/acctcost.py "$from" | head -1 | jq -c '{prefix_usd}')"
  } 2>&1 | log ;;
*) echo "step: state sweep init register keys infra front ops bundles probe-deploy install hook standins repos image controller l002 l003-setup l003 l003-release teardown cost"; exit 2 ;;
esac
