#!/bin/bash
# LF-7 hosted on graft-m4a (`build/BUILD-PLAN.md`, AM3-WP1, LF-7): a bad `graft-controller` (`test-bad`) at 1% under a
# `ControllerRoll` with the agents account's deploy token blocked; the pre-signed `ControllerPointer` undo posted by
# hand; then `GET release/entries` blackholed for 10 minutes while signed webhooks flow through `graft-agents-front`.
# The runner account stands in as the agents account (`FRAME.md`, "Operator decision, 2026-11-24, AM1 closed and the
# runner account stands in as the agents account"); every resource there is named `graft-agents-m4a-`. The blackhole
# is the bed's fault hook on the install's Release members (site 255, chains release), as BF-8 used it
# (`tests/m5/wp2-hosted.sh`), not a WAF rule: the lead's ruling, 2026-11-24. One step per call, each logged to
# tests/am3/raw/lf7-<tag>/<step>.log.
#
#   bash tests/am3/hosted/lf7-day.sh <tag> <step> [args]
#
# Needs `source tests/am1/lf13/env-m4a.sh`, CLOUDFLARE_* and CLOUDFLARE_RUNNER_*, CF_WORKERS_SUBDOMAIN and
# CF_RUNNER_WORKERS_SUBDOMAIN, ~/.graft-m4a, G (the graftyard binary) and RELVIEW. Token and secret values are read
# only from 0600 files and sent in bodies or headers; every log line goes through `scrub`.
#
# Steps:
#   state            the group (count, current, rolled), the install's edge and ops, the runner account's scripts; spend
#   sweep            teardown.py --sweep: no shard container may run
#   a54              OPEN-QUESTIONS A54: a fresh Workers-Scripts-Write-only token uploads a script with a Secrets Store
#                    binding, in the install account and the runner account (a54-probe.mjs; everything deleted after)
#   edge             edge day from this checkout (test-faults, the live vars kept), the R57 vars diff, the read probe
#   init             `agents init` into the runner account's store, prefix graft-agents-m4a-; the webhook secret
#                    replaced by one this row holds in a 0600 file
#   register         the two slug claims, `agents init --app-ids`, and the registration, through apps.mjs
#   keys             a customer-signed Keys adding agents_executor
#   infra            bucket graft-agents-m4a-releases, token graft-agents-m4a-deploy into the store
#   controller       graft-agents-m4a-controller c0 (the script `graft-ops` uploads beside) and its STATUS_TOKEN
#   front            graft-agents-m4a-front and its FRONT_TOKEN
#   ops              graft-agents-m4a-ops (`deploy.mjs --agents`), GATE unset, 1-minute cron, then cron-fired.py waits
#                    up to 15 minutes for its first scheduled run, resetting once only if none comes (R52)
#   bundles          c1 (this checkout's controller, marked) and c2 (`test-bad`), signed, under the bucket
#   roll <R>         `release roll R` (a ControllerRoll), its pre-signed undos
#   abort <R>        the step-0 check, then the customer's Abort to R
#   block|unblock    the agents deploy token set to an invalid value, or back
#   pointer          the current ControllerRoll's pre-signed ControllerPointer, posted by hand
#   fault on|off     the bed's fault plan: site 255, chains release, hang 10 s, for 10 minutes; or cleared
#   sampler start|stop [conc]   lf7-sample.mjs through graft-agents-front into $O/hooks.jsonl
#   status           graft-agents-front's and graft-controller's status, and the front's CONTROLLER_DEFAULT
#   view             the group's entries since the row's start, the controller's deployment
#   logs <from> [to] the agents graft-ops's lines in the window
#   teardown         every graft-agents-m4a- script, secret, token and bucket; the token files shredded
#   cost [from]      graft-m4a's and the runner account's spend
#
# ROW=lm3 runs LM-3's "a bad controller at 1% swaps back" with the same steps, GATE unset, so the agents `graft-ops`
# judges the roll on graft-agents-front's counts (DECISIONS "`controller-errors`"); its logs go to
# tests/am3/raw/lm3-<tag>/ and its state to ~/.graft-m4a/lm3. It adds:
#   probe-deploy     graft-agents-m4a-probe (lm3-probe.js), bound to the graftyard-agent key, for the refusal probes
#   probe            through it: GET release/errors, then a flush naming a version no upload committed and one over
#                    FLUSH_CALLS_MAX, each followed by a read, so a refusal is seen to store nothing
#   errors [since]   the install's stored controller counts since <ms> (default: the roll's start), through the probe
# TB=1 (ROW=kill, LM-3's kill row, `tests/am3/hosted/kill-row.sh`): `controller` deploys the `TaskBox` build
# (`--test-faults --taskbox $K/taskbox.json`) and `bundles` builds only c1, that build with `entry.mjs`.
# The probes run after the verdict (the ControllerPointer committed): an over-cap flush the edge wrongly stored would
# add 10,001 clean N+1 answers to the gate's input mid-row.
set -uo pipefail
cd "$(dirname "$0")/../../.."
TAG=${1:?tag}; STEP=${2:?step}
ROW=${ROW:-lf7}
O=tests/am3/raw/$ROW-$TAG; mkdir -p "$O"
W=$HOME/.graft-m4a; K=$W/$ROW; mkdir -p "$K"; chmod 700 "$K"
P=graft-agents-m4a
EDGE_NAME=graft-m4a-edge OPS=graft-m4a-ops
CTRL=$P-controller FRONT=$P-front AOPS=$P-ops BUCKET=$P-releases PROBE=$P-probe
RSTORE=2b564b906c40496ea9544799904bc93b ISTORE=${CF_SECRETS_STORE_ID:?}
EDGE_HOST=$EDGE_NAME.$CF_WORKERS_SUBDOMAIN.workers.dev
FRONT_URL=https://$FRONT.$CF_RUNNER_WORKERS_SUBDOMAIN.workers.dev
PROBE_URL=https://$PROBE.$CF_RUNNER_WORKERS_SUBDOMAIN.workers.dev
B="https://api.cloudflare.com/client/v4/accounts/$CLOUDFLARE_ACCOUNT_ID"
RB="https://api.cloudflare.com/client/v4/accounts/$CLOUDFLARE_RUNNER_ACCOUNT_ID"
api() { curl -s -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" "$1" "${@:2}"; }
rapi() { curl -s -H "Authorization: Bearer $CLOUDFLARE_RUNNER_API_TOKEN" "$1" "${@:2}"; }
# The runner account as the agents account, for wrangler, deploy.mjs and the CLI.
runner() { CLOUDFLARE_ACCOUNT_ID=$CLOUDFLARE_RUNNER_ACCOUNT_ID CLOUDFLARE_API_TOKEN=$CLOUDFLARE_RUNNER_API_TOKEN "$@"; }
scrub() {
  sed -e "s/$CLOUDFLARE_ACCOUNT_ID/<install-account>/g" -e "s/$CLOUDFLARE_RUNNER_ACCOUNT_ID/<runner-account>/g" \
      -e "s/$CF_WORKERS_SUBDOMAIN/<sub>/g" -e "s/$CF_RUNNER_WORKERS_SUBDOMAIN/<runner-sub>/g" \
      -e "s/$RSTORE/<runner-store>/g" -e "s/$ISTORE/<install-store>/g" -e "s#$PWD/#<checkout>/#g" -e "s#$HOME/#~/#g"
}
N=$(ls "$O" | grep -c "^$STEP" || true)
L="$O/$STEP$( [ "$N" -gt 0 ] && echo "-$((N + 1))" ).log"
log() { { date -u +%FT%TZ; git rev-parse --short HEAD; cat; } | scrub >"$L"; cat "$L"; }
rsecret_id() { rapi "$RB/secrets_store/stores/$RSTORE/secrets?per_page=100" | jq -r --arg n "$1" '.result[] | select(.name == $n) | .id'; }
# Sets a store secret this row created to the value in file $2 ({"value": ...}).
rpatch() { rapi "$RB/secrets_store/stores/$RSTORE/secrets/$(rsecret_id "$1")" -X PATCH -H 'content-type: application/json' --data-binary @"$2"; }
deployment() { ${3:-api} "${2:-$B}/workers/scripts/$1/deployments" | jq -c --arg n "$1" '.result.deployments[0] | {script: $n, created_on, versions: [.versions[]? | "\(.version_id[0:8]) \(.percentage)"]}'; }
rsplit() { rapi "$RB/workers/scripts/$1/deployments" | jq -c '[.result.deployments[0].versions[]? | [.version_id, .percentage]]'; }
cron() { # <script> <cron or "">: the yearly cron first, then the one wanted (R52: one PUT after a deploy may not take)
  rapi "$RB/workers/scripts/$1/schedules" -X PUT -H 'content-type: application/json' --data '[{"cron":"0 0 1 1 *"}]' >/dev/null; sleep 2
  local body='[]'; [ -n "$2" ] && body="[{\"cron\":\"$2\"}]"
  rapi "$RB/workers/scripts/$1/schedules" -X PUT -H 'content-type: application/json' --data "$body" | jq -c --arg n "$1" '{($n): [.result.schedules[]?.cron]}'
}
vars() { # <script> <version id>: the plain-text and JSON vars of one version, sorted, values hashed (R57)
  api "$B/workers/scripts/$1/versions/$2" | jq -c '[.result.resources.bindings[] | select(.type == "plain_text" or .type == "json") | {name, v: ((.text // (.json|tostring)) | @base64)}] | sort_by(.name)' |
    python3 -c 'import sys,json,hashlib; print(json.dumps({b["name"]: hashlib.sha256(b["v"].encode()).hexdigest()[:12] for b in json.load(sys.stdin)}, sort_keys=True))'
}
ANCHOR=$(grep -E '^(install|customer_key) ' "$W/install.toml" | sed -E 's/.*= *"([^"]*)"/\1/' | xargs)
view() { node tests/m5/bf46/view.mjs pages "$K/pages.json" >/dev/null && "$RELVIEW" $ANCHOR "${1:-0}" <"$K/pages.json"; }
fault() { # <query>: the bed's fault plan on every site's Registry, tried up to 5 times, as BF-8's driver does
  local i a
  for i in 1 2 3 4 5; do
    a=$(curl -sf -m 20 -X POST -H "x-bed-token: $(cat "$W/bed.token")" "https://$EDGE_HOST/_bed/fault?$1") && break
    echo "fault $1: try $i failed"; sleep 2
  done
  echo "$a" | jq -c '{now, plan, sites: [.sites[] | {site, ok: (.plan.error == null)}]}'
}
post() { # <signed file>: posted to the install's edge, prints the answer
  "$G" --config "$W/install.toml" release post "$1" --edge "$GRAFT_EDGE" 2>&1
}

case $STEP in
state)
  { view 0 | jq -c '{count, current, drained, rolled}'
    deployment $EDGE_NAME; deployment $OPS; api "$B/workers/scripts/$OPS/schedules" | jq -c '{ops_schedules: [.result.schedules[]?.cron]}'
    echo "runner scripts: $(rapi "$RB/workers/scripts" | jq -c '[.result[].id]')"
    echo "runner store secrets named $P-: $(rapi "$RB/secrets_store/stores/$RSTORE/secrets?per_page=100" | jq -c --arg p "$P-" '[.result[] | select(.name | startswith($p)) | .name]')"
    echo "runner buckets: $(rapi "$RB/r2/buckets" | jq -c '[.result.buckets[]?.name]')"
  } 2>&1 | log ;;
sweep)
  { python3 tests/fanout/teardown.py --sweep 2>&1 | tail -3; echo "exit ${PIPESTATUS[0]}"; } 2>&1 | log ;;
a54)
  { echo "install account (graft-m4a- names):"
    CF_TOKEN=$CLOUDFLARE_API_TOKEN node tests/am3/hosted/a54-probe.mjs "$CLOUDFLARE_ACCOUNT_ID" "$ISTORE" graft-m4a-
    echo "runner account ($P- names):"
    CF_TOKEN=$CLOUDFLARE_RUNNER_API_TOKEN node tests/am3/hosted/a54-probe.mjs "$CLOUDFLARE_RUNNER_ACCOUNT_ID" "$RSTORE" $P-
  } 2>&1 | log ;;
edge)
  # The edge day: the live version's vars kept (FORMATS_ENABLED, RUN_SERVICE_PUBLIC, HOST), the ASSETS build of
  # AM2-WP3's day, test-faults so `/_bed/fault` stays, and R57's diff of every var before and after.
  { live=$(api "$B/workers/scripts/$EDGE_NAME/deployments" | jq -r '.result.deployments[0].versions | max_by(.percentage) | .version_id')
    newest=$(api "$B/workers/scripts/$EDGE_NAME/versions?per_page=1" | jq -r '.result.items[0].id')
    echo "deployed $live, newest upload $newest"
    vars $EDGE_NAME "$live" >"$K/vars-before"; vars $EDGE_NAME "$newest" >"$K/vars-newest"
    echo "R57: newest vs deployed: $(python3 -c 'import json,sys; a,b=(json.load(open(f)) for f in sys.argv[1:]); print({k: [a.get(k), b.get(k)] for k in sorted(set(a)|set(b)) if a.get(k)!=b.get(k)})' "$K/vars-before" "$K/vars-newest")"
    fmt=$(api "$B/workers/scripts/$EDGE_NAME/versions/$live" | jq -r '.result.resources.bindings[] | select(.name == "FORMATS_ENABLED") | .text')
    rsp=$(api "$B/workers/scripts/$EDGE_NAME/versions/$live" | jq -r '.result.resources.bindings[] | select(.name == "RUN_SERVICE_PUBLIC") | .text')
    echo "start $(date -u +%FT%TZ)"
    (export PATH=$HOME/.cargo/bin:$PATH && unset CARGO_TARGET_DIR &&
      node tests/m1/integ/deploy-edge.mjs "$W/sites.json" $EDGE_NAME --test-faults --anchor "$W/install.toml" --ops $OPS \
        --web /tmp/wp3web --var "FORMATS_ENABLED=$fmt" --var "RUN_SERVICE_PUBLIC=$rsp" 2>&1 |
        grep -E "Uploaded|Deployed|Current Version|rror" | tail -4)
    echo "exit at $(date -u +%FT%TZ)"
    now=$(api "$B/workers/scripts/$EDGE_NAME/deployments" | jq -r '.result.deployments[0].versions | max_by(.percentage) | .version_id')
    deployment $EDGE_NAME
    vars $EDGE_NAME "$now" >"$K/vars-after"
    echo "R57: after vs before: $(python3 -c 'import json,sys; a,b=(json.load(open(f)) for f in sys.argv[1:]); print({k: [a.get(k), b.get(k)] for k in sorted(set(a)|set(b)) if a.get(k)!=b.get(k)})' "$K/vars-before" "$K/vars-after")"
    sleep 5
    echo "GET release/entries, no JWT: $(curl -s -o /dev/null -w '%{http_code}' "https://$EDGE_HOST/api/graftyard/v1/release/entries?after=0&per_page=1")"
    echo "fault hook, clear: $(fault off=1)"
  } 2>&1 | log ;;
init)
  { ( cd "$K" && rm -rf agents && mkdir agents
      runner "$G" --config "$W/install.toml" agents init --host "$EDGE_HOST" --account "$CLOUDFLARE_RUNNER_ACCOUNT_ID" \
        --store $RSTORE --secret-prefix $P- --out "$K/agents" )
    # The controller checks HMACs with the secret's string, so the sampler must hold it: a value drawn here, kept 0600.
    ( umask 077; python3 -c 'import os; print(os.urandom(32).hex())' >"$K/webhook.secret"
      jq -Rc '{value: .}' <"$K/webhook.secret" >"$K/webhook.value" )
    rpatch $P-AGENTS_WEBHOOK_SECRET_A "$K/webhook.value" | jq -c '{replace: "AGENTS_WEBHOOK_SECRET_A", ok: .success}'
    jq -c '{host, secret_prefix, apps: [.apps[] | .slug], agents_executor}' "$K/agents/agents.json"
  } 2>&1 | log ;;
register)
  { out=$(cd tests/am2/hosted && node apps.mjs claims "$K/agents" agents); echo "$out"
    a=$(echo "$out" | awk '$1 == "graftyard-agent" {print $2}'); w=$(echo "$out" | awk '$1 == "graftyard-workspace" {print $2}')
    echo "$a $w" >"$K/app-ids"
    "$G" agents init --app-ids "$a,$w" --out "$K/agents" && (cd tests/am2/hosted && node apps.mjs register "$K/agents" agents)
    sleep 3
    echo "GET release/entries, no JWT: $(curl -s -o /dev/null -w '%{http_code}' "https://$EDGE_HOST/api/graftyard/v1/release/entries?after=0&per_page=1")"
  } 2>&1 | log ;;
keys)
  { v=$(view 0); echo "$v" | jq -c '{count, prev}'
    echo "$v" | jq -r .keys_entry | python3 -c "import sys; sys.stdout.buffer.write(bytes.fromhex(sys.stdin.read().strip()))" >"$K/keys.entry"
    echo "{\"Keys\": {\"agents_executor\": \"$(jq -r .agents_executor "$K/agents/agents.json")\"}}" >"$K/keys.json"
    "$G" --config "$W/install.toml" release entry "$K/keys.json" --out "$K/keys.unsigned" --prev "$(echo "$v" | jq -r .prev)" \
      --keys "$K/keys.entry" && "$G" sign "$K/keys.unsigned" --out "$K/keys.signed" --key "$W/customer.key" >/dev/null &&
      post "$K/keys.signed"
    view 0 | jq -c '{count, keys: .keys}'
  } 2>&1 | log ;;
infra)
  { rapi "$RB/r2/buckets" -X POST -H 'content-type: application/json' --data "{\"name\": \"$BUCKET\"}" | jq -c '{bucket: .result.name, ok: .success, errors: [.errors[]?.code]}'
    ( umask 077
      jq -nc --arg a "com.cloudflare.api.account.$CLOUDFLARE_RUNNER_ACCOUNT_ID" --arg n "$P-deploy" \
        '{name: $n, policies: [{effect: "allow", resources: {($a): "*"}, permission_groups: [{id: "e086da7e2179491d91ee5f35b3ca210a"}, {id: "adc8fa2bc6124928a8b3314dc63a1235"}, {id: "bdbcd690c763475a985e8641dddc09f7"}]}]}' |
        rapi "$RB/tokens" -X POST -H 'content-type: application/json' --data-binary @- >"$K/deploy.json"
      jq -c '{value: .result.value}' "$K/deploy.json" >"$K/deploy.value"
      jq -c --arg n "$P-deploy-token" '[{name: $n, value: .result.value, comment: "LF-7 row", scopes: ["workers"]}]' "$K/deploy.json" >"$K/deploy.create" )
    jq -c --arg n "$P-deploy" '{mint: $n, ok: .success, id: (.result.id // "" | .[0:8]), errors: [.errors[]?.code]}' "$K/deploy.json"
    rapi "$RB/secrets_store/stores/$RSTORE/secrets" -X POST -H 'content-type: application/json' --data-binary @"$K/deploy.create" |
      jq -c --arg n "$P-deploy-token" '{create: $n, ok: .success, errors: [.errors[]?.code]}'
    shred -u "$K/deploy.create"
  } 2>&1 | log ;;
controller)
  { (export PATH=$HOME/.cargo/bin:$PATH && unset CARGO_TARGET_DIR &&
      runner node workers/controller/deploy.mjs $CTRL --secret $RSTORE/$P-AGENTS_WEBHOOK_SECRET_A \
        ${TB:+--test-faults --taskbox $K/taskbox.json} 2>&1 | grep -E "Current Version|Uploaded|rror|ERROR" | tail -4)
    ( umask 077; [ -s "$K/ctrl.token" ] || python3 -c 'import os; print(os.urandom(24).hex())' >"$K/ctrl.token" )
    runner npx --prefix tests/m0 wrangler secret put STATUS_TOKEN --name $CTRL <"$K/ctrl.token" 2>&1 | tail -1
    deployment $CTRL "$RB" rapi
  } 2>&1 | log ;;
front)
  { read -r app _ <"$K/app-ids"
    (export PATH=$HOME/.cargo/bin:$PATH && unset CARGO_TARGET_DIR &&
      runner node workers/agents-front/deploy.mjs $FRONT --controller $CTRL --anchor "$W/install.toml" --host "$EDGE_HOST" \
        --app-id "$app" --key $RSTORE/$P-AGENTS_APP_KEY_A 2>&1 | grep -E "Current Version|rror" | tail -2)
    ( umask 077; [ -s "$K/front.token" ] || python3 -c 'import os; print(os.urandom(24).hex())' >"$K/front.token" )
    runner npx --prefix tests/m0 wrangler secret put FRONT_TOKEN --name $FRONT <"$K/front.token" 2>&1 | tail -1
    deployment $FRONT "$RB" rapi
  } 2>&1 | log ;;
ops)
  { read -r app _ <"$K/app-ids"
    jq -n --arg anchor "$W/install.toml" --arg host "$EDGE_HOST" --argjson app "$app" --arg p "$P-" \
      --arg front $FRONT --arg ctrl $CTRL --arg bucket $BUCKET \
      '{anchor: $anchor, host: $host, app_id: $app, app_key: "\($p)AGENTS_APP_KEY_A", executor_key: "\($p)AGENTS_EXECUTOR_KEY",
        deploy_token: "\($p)deploy-token", scripts: {"agents-front": $front, controller: $ctrl}, releases: $bucket,
        step_dwell_ms: 60000, first_day_ms: 300000, cron: "* * * * *"}' >"$K/agents-ops.json"
    jq -c '{scripts, releases, step_dwell_ms, first_day_ms, cron, gate}' "$K/agents-ops.json"
    (export PATH=$HOME/.cargo/bin:$PATH && unset CARGO_TARGET_DIR &&
      runner node workers/ops/deploy.mjs $AOPS $RSTORE $P-AGENTS_WEBHOOK_SECRET_A --agents "$K/agents-ops.json" 2>&1 |
        grep -E "Current Version|rror" | tail -3)
    deployment $AOPS "$RB" rapi
    t=$(date +%s%3N); cron $AOPS "* * * * *"
    # R52: a new trigger's first run comes 4.5 to 10 minutes later, reset or not; wait for it (up to 15 minutes, then one
    # reset and 15 more) before any step that needs the executor.
    runner python3 tests/am3/hosted/cron-fired.py $AOPS "* * * * *" --since "$t" || { echo "STOP: the agents graft-ops cron never fired"; exit 3; }
  } 2>&1 | log ;;
bundles)
  { put() { runner npx --prefix tests/m0 wrangler r2 object put "$BUCKET/$1" --file "$2" --remote 2>&1 | tail -1; }
    fmt=$W/wp3/formats.txt; echo "formats: $(wc -l <"$fmt") lines, AM2-WP3's FMT_FROM"
    ls="c1 c2"; [ -n "${TB:-}" ] && ls=c1
    for l in $ls; do
      f=; [ $l = c2 ] && f="--features test-bad"; [ -n "${TB:-}" ] && f="--features test-faults"
      (cd workers/controller && export PATH=$HOME/.cargo/bin:$PATH && unset CARGO_TARGET_DIR &&
        worker-build --release --out-dir "$K/build-$l" $f >/dev/null 2>&1) && echo "$l built ${f:-plain}"
      d=$K/rel/$l; rm -rf "$d"; p=$ROW$TAG-$l
      python3 tests/am2/hosted/wp3-bundle.py "$d" "$p" --formats "$fmt" --controller "$K/build-$l" --marker \
        ${TB:+--controller-entry workers/controller/entry.mjs} | tee "$K/rel-$l.json"
      openssl pkeyutl -sign -rawin -inkey "$W/release.key" -in "$d/release.json" -out "$d/release.sig"
      (cd "$d" && find . -type f | sed 's#^\./##') | while read -r f; do put "$p/$f" "$d/$f" >/dev/null; done
      echo "$l uploaded under $BUCKET/$p/"
    done
  } 2>&1 | log ;;
roll)
  R=${3:?release id}
  { node tests/m5/bf46/view.mjs pages "$K/pages.json" >/dev/null
    rsplit $CTRL >"$K/dep-ctrl"; echo "controller at roll: $(cat "$K/dep-ctrl")"
    date +%s%3N >"$O/roll.t0"
    "$G" --config "$W/install.toml" release roll "$R" --entries "$K/pages.json" --key "$W/customer.key" --ttl-s 21600 \
      --edge "$GRAFT_EDGE"
    view 0 | jq -c '{count, current}'
  } 2>&1 | log ;;
abort)
  R=${3:?target release id}
  { v=$(view 0); c=$(echo "$v" | jq -c .current); echo "current: $c"
    now=$(rsplit $CTRL); was=$(cat "$K/dep-ctrl")
    echo "step-0 check: next $(echo "$c" | jq -r .next), controller deployment $now, at roll $was"
    case $(echo "$c" | jq -r .intent) in ControllerRoll*) ;; *) echo "STOP: not a ControllerRoll"; exit 3 ;; esac
    if [ "$(echo "$c" | jq -r .next)" != 0 ] || [ "$now" != "$was" ]; then echo "STOP: the intent has stepped; no Abort posted"; exit 3; fi
    ID=$(echo "$v" | jq -r '[.entries[] | select(.kind == "Intent")][-1].entry' | sed 's/.* id //')
    echo "abort intent $ID to $R"
    echo "$v" | jq -r .keys_entry | python3 -c "import sys; sys.stdout.buffer.write(bytes.fromhex(sys.stdin.read().strip()))" >"$K/keys.entry"
    echo "{\"Abort\": {\"intent\": \"$ID\", \"target\": \"$R\"}}" >"$K/abort.json"
    "$G" --config "$W/install.toml" release entry "$K/abort.json" --out "$K/abort.unsigned" --prev "$(echo "$v" | jq -r .prev)" \
      --keys "$K/keys.entry" >/dev/null && "$G" sign "$K/abort.unsigned" --out "$K/abort.signed" --key "$W/customer.key" >/dev/null &&
      post "$K/abort.signed"
    view 0 | jq -c '{count, current}'
  } 2>&1 | log ;;
block)
  { echo '{"value":"lf7-blocked-invalid-token"}' >"$K/blocked.value"
    rpatch $P-deploy-token "$K/blocked.value" | jq -c '{block: .success, modified: .result.modified}'
    curl -s -o /dev/null -w 'probe %{http_code}\n' -H "Authorization: Bearer lf7-blocked-invalid-token" "$RB/workers/scripts/$CTRL/deployments"
  } 2>&1 | log ;;
unblock)
  { rpatch $P-deploy-token "$K/deploy.value" | jq -c '{unblock: .success, modified: .result.modified}'; } 2>&1 | log ;;
pointer)
  { v=$(view 0); c=$(echo "$v" | jq -c .current); echo "current: $c"
    case $(echo "$c" | jq -r .intent) in ControllerRoll*) ;; *) echo "STOP: not a ControllerRoll"; exit 3 ;; esac
    ID=$(echo "$v" | jq -r '[.entries[] | select(.kind == "Intent")][-1].entry' | sed 's/.* id //')
    PIN=$(jq -r .release "$K/rel-c1.json")
    echo "{\"Undo\": {\"intent\": \"$ID\", \"entry\": {\"ControllerPointer\": {\"pin\": \"$PIN\"}}}}" >"$K/undo.json"
    "$G" --config "$W/install.toml" release entry "$K/undo.json" --out "$K/undo.unsigned" >/dev/null &&
      "$G" sign "$K/undo.unsigned" --out "$K/undo.signed" --key "$W/customer.key" >/dev/null
    t0=$(date +%s%3N); a=$(post "$K/undo.signed"); t1=$(date +%s%3N)
    echo "undo ControllerPointer pin c1 $PIN: $a"
    echo "{\"t0\": $t0, \"t1\": $t1, \"answer\": $a}" >"$O/pointer.json"
    view 0 | jq -c '{count, current}'
  } 2>&1 | log ;;
fault)
  case ${3:?on|off} in
    on) { t0=$(date +%s%3N); fault "site=255&chains=release&hang_ms=10000&for_ms=600000"; echo "{\"on\": $t0}" >>"$O/fault.json"; } 2>&1 | log ;;
    off) { fault off=1; echo "{\"off\": $(date +%s%3N)}" >>"$O/fault.json"; } 2>&1 | log ;;
  esac ;;
sampler)
  case ${3:?start|stop} in
    start) rm -f "$O/stop"; date -u +%s%3N >>"$O/sampler-starts"
           amp orb service start $ROW-sampler --command "cd $PWD && node tests/am3/hosted/lf7-sample.mjs $FRONT_URL $K/webhook.secret ${4:-4} $PWD/$O/hooks.jsonl $PWD/$O/stop" 2>&1 | head -1 | log ;;
    stop) touch "$O/stop"; sleep 3; amp orb service stop $ROW-sampler 2>&1 | tail -1 | log ;;
  esac ;;
status)
  { curl -s -H "x-front-token: $(cat "$K/front.token")" "$FRONT_URL/_agents_front/status" | jq -c .
    deployment $CTRL "$RB" rapi; deployment $FRONT "$RB" rapi
    live=$(rapi "$RB/workers/scripts/$FRONT/deployments" | jq -r '.result.deployments[0].versions | max_by(.percentage) | .version_id')
    echo "front $live CONTROLLER_DEFAULT: $(rapi "$RB/workers/scripts/$FRONT/versions/$live" | jq -c '[.result.resources.bindings[] | select(.name == "CONTROLLER_DEFAULT") | .text]')"
  } 2>&1 | log ;;
view)
  { deployment $CTRL "$RB" rapi; deployment $FRONT "$RB" rapi; deployment $AOPS "$RB" rapi
    view "${3:-0}" | jq -c '{count, current, rolled, entries}'
  } 2>&1 | log ;;
logs)
  { runner env MATCH=graft-agents KEEP="graft-agents,PAGE,IntentStep,IntentDone,Committed,Paged,Error,ALARM" \
      python3 tests/m5/bf46/logs.py "${5:-$AOPS}" "${3:?from ms}" "${4:-$(date +%s%3N)}" | tail -${6:-60}
  } 2>&1 | log ;;
teardown)
  { touch "$O/stop"; amp orb service stop $ROW-sampler >/dev/null 2>&1
    cron $AOPS ""
    for n in $AOPS $FRONT $CTRL $PROBE; do runner npx --prefix tests/m0 wrangler delete --name $n --force 2>&1 | tail -1; done
    # KEEP_SECRETS names secrets the next row needs, such as LM-3's real-harness model key ($P-MODEL_KEY_A), which the lead
    # stores once and the teardown must not take with the row's own secrets.
    for n in $(rapi "$RB/secrets_store/stores/$RSTORE/secrets?per_page=100" | jq -r --arg p "$P-" --arg keep "${KEEP_SECRETS:-}" '.result[] | select((.name | startswith($p)) and ((.name | IN($keep | split(" ")[])) | not)) | .name'); do
      rapi "$RB/secrets_store/stores/$RSTORE/secrets/$(rsecret_id "$n")" -X DELETE | jq -c --arg n "$n" '{delete: $n, ok: .success}'
    done
    id=$(rapi "$RB/tokens?per_page=50" | jq -r --arg n "$P-deploy" '.result[] | select(.name == $n) | .id')
    [ -n "$id" ] && rapi "$RB/tokens/$id" -X DELETE | jq -c --arg n "$P-deploy" '{revoke: $n, ok: .success}'
    keys=$(rapi "$RB/r2/buckets/$BUCKET/objects?per_page=1000" | jq -r '.result[]?.key')
    for k in $keys; do runner npx --prefix tests/m0 wrangler r2 object delete "$BUCKET/$k" --remote >/dev/null 2>&1; done
    echo "objects deleted: $(echo "$keys" | grep -c .)"
    rapi "$RB/r2/buckets/$BUCKET" -X DELETE | jq -c --arg n $BUCKET '{delete: $n, ok: .success, errors: [.errors[]?.code]}'
    for f in "$K"/deploy.json "$K"/deploy.value "$K"/front.token "$K"/ctrl.token "$K"/webhook.secret "$K"/webhook.value "$K"/probe.token; do [ -f "$f" ] && shred -u "$f"; done
    echo "row token and secret files shredded"
    echo "runner scripts: $(rapi "$RB/workers/scripts" | jq -c '[.result[].id]')"
    echo "runner store secrets named $P-: $(rapi "$RB/secrets_store/stores/$RSTORE/secrets?per_page=100" | jq -c --arg p "$P-" '[.result[] | select(.name | startswith($p)) | .name]')"
    echo "runner buckets: $(rapi "$RB/r2/buckets" | jq -c '[.result.buckets[]?.name]')"
  } 2>&1 | log ;;
probe-deploy)
  { read -r app ws <"$K/app-ids"
    ( umask 077; [ -s "$K/probe.token" ] || python3 -c 'import os; print(os.urandom(24).hex())' >"$K/probe.token" )
    mkdir -p "$K/probe" && cp tests/am3/hosted/lm3-probe.js "$K/probe/index.js"
    # graftyard-workspace's key too, for `/mint` (LM-3's in-repo row; the kill row's app-ids hold both Apps).
    jq -n --arg n $PROBE --arg host "$EDGE_HOST" --arg app "$app" --arg ws "${ws:-0}" --arg store $RSTORE --arg key "$P-AGENTS_APP_KEY_A" \
      --arg wkey "$P-WORKSPACE_APP_KEY_A" \
      '{name: $n, main: "index.js", compatibility_date: "2026-10-01", workers_dev: true, preview_urls: false,
        vars: {GRAFTYARD_HOST: $host, AGENT_APP_ID: $app, WORKSPACE_APP_ID: $ws},
        secrets_store_secrets: ([{binding: "AGENT_APP_KEY", store_id: $store, secret_name: $key}]
          + (if $ws == "0" then [] else [{binding: "WORKSPACE_APP_KEY", store_id: $store, secret_name: $wkey}] end))}' >"$K/probe/wrangler.json"
    (cd "$K/probe" && runner npx --prefix "$OLDPWD/tests/m0" wrangler deploy -c wrangler.json 2>&1 | grep -E "Current Version|rror" | tail -2)
    runner npx --prefix tests/m0 wrangler secret put PROBE_TOKEN --name $PROBE <"$K/probe.token" 2>&1 | tail -1
    deployment $PROBE "$RB" rapi
  } 2>&1 | log ;;
probe)
  # A flush naming a version no upload committed (all zeros but one byte, so it parses), then one over the cap
  # (FLUSH_CALLS_MAX + 1 answers of N+1, no 5xx): each must answer 422 and leave the read unchanged.
  { since=${3:-$(cat "$O/roll.t0" 2>/dev/null || echo 0)}
    # N+1 is the version at the smallest share of the controller's split: an upload the group committed, so the
    # over-cap flush is refused for the cap, not for its version.
    n1=$(rsplit $CTRL | jq -r 'min_by(.[1]) | .[0] // empty')
    [ -n "$n1" ] || { echo "STOP: no controller split to name N+1"; exit 3; }
    echo "N+1 for the over-cap flush: ${n1:0:8}"
    max=$(grep -oE 'FLUSH_CALLS_MAX: u64 = [0-9_]+' crates/release/src/gate.rs | grep -oE '[0-9_]+$' | tr -d _)
    jq -nc --argjson since "$since" --arg n1 "$n1" --argjson over "$((max + 1))" \
      '{since: $since, flushes: [
         {name: "unknown version", counts: {versions: {"00000000-0000-0000-0000-00000000005a": {answers: 1, fivexx: 0}}, threw: 0, unversioned: {answers: 0, fivexx: 0}}},
         {name: "over FLUSH_CALLS_MAX", counts: {versions: {($n1): {answers: $over, fivexx: 0}}, threw: 0, unversioned: {answers: 0, fivexx: 0}}}]}' |
      curl -s -m 60 -X POST -H "x-probe-token: $(cat "$K/probe.token")" -H 'content-type: application/json' --data-binary @- "$PROBE_URL/probe" |
      tee "$O/probe.json" | jq -c '.before.body.counts as $b | {before: .before.status, sum: $b,
        flushes: [.flushes[] | {name, post: .post.status, why: .post.body.message, after: .after.status, unchanged: (.after.body.counts == $b)}]}'
  } 2>&1 | log ;;
errors)
  { since=${3:-$(cat "$O/roll.t0" 2>/dev/null || echo 0)}
    jq -nc --argjson since "$since" '{since: $since, flushes: []}' |
      curl -s -m 60 -X POST -H "x-probe-token: $(cat "$K/probe.token")" -H 'content-type: application/json' --data-binary @- "$PROBE_URL/probe" |
      jq -c '{status: .before.status, sites: .before.body.sites, of: .before.body.of, counts: .before.body.counts}'
  } 2>&1 | log ;;
cost)
  { from=${3:-$(date -u +%Y-%m-%dT00:00:00Z)}
    echo "graft-m4a from $from: $(PREFIX=graft-m4a python3 tests/fanout/acctcost.py "$from" | head -1 | jq -c '{prefix_usd}')"
    echo "graft-m4a day: $(PREFIX=graft-m4a python3 tests/fanout/acctcost.py "$(date -u +%Y-%m-%dT00:00:00Z)" | head -1 | jq -c '{prefix_usd}')"
    echo "runner $P- from $from: $(runner env PREFIX=$P python3 tests/fanout/acctcost.py "$from" | head -1 | jq -c '{prefix_usd}')"
  } 2>&1 | log ;;
*) echo "step: state sweep a54 edge init register keys infra controller front ops bundles roll abort block unblock pointer fault sampler status view logs probe-deploy probe errors teardown cost"; exit 2 ;;
esac
