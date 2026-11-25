#!/bin/bash
# LF-8 hosted (`build/BUILD-PLAN.md`, AM3-WP1, LF-8): forged `release/entries` served to a deployed, unmodified
# `graft-agents-front`. The lead's ruling, 2026-11-24 (plan A): no install is touched. The forger `graft-lf8-feed` runs
# in the install account (a same-account workers.dev fetch answers 1042, `tests/am3/raw/lf8/reach.log`) and serves a
# synthetic history plus one forgery at a time; the front, a plain controller and their throwaway secrets run in the
# runner account as `graft-agents-lf8-`. `lf8forge` draws the run's keys and builds the 300 forgeries offline into
# ~/.lf8 (0700), never the repository; only its `manifest.tsv` is copied to the raw logs.
#
#   bash tests/am3/hosted/lf8-day.sh <tag> <step>
#
# Steps: state, sweep, forge, secrets, feed, controller, front, run, cold, teardown, cost.
# Needs the LF-7 environment (`source /tmp/lf7env.sh`: CLOUDFLARE_* and CLOUDFLARE_RUNNER_*, both workers.dev
# subdomains) and worker-build. Token and secret values are read only from 0600 files.
set -uo pipefail
cd "$(dirname "$0")/../../.."
TAG=${1:?tag}; STEP=${2:?step}
O=tests/am3/raw/lf8-$TAG; mkdir -p "$O"
K=$HOME/.lf8/$TAG; mkdir -p "$K"; chmod 700 "$HOME/.lf8" "$K"
P=graft-agents-lf8 FEED=graft-lf8-feed
CTRL=$P-controller FRONT=$P-front
RSTORE=2b564b906c40496ea9544799904bc93b
FEED_URL=https://$FEED.$CF_WORKERS_SUBDOMAIN.workers.dev
FRONT_URL=https://$FRONT.$CF_RUNNER_WORKERS_SUBDOMAIN.workers.dev
B="https://api.cloudflare.com/client/v4/accounts/$CLOUDFLARE_ACCOUNT_ID"
RB="https://api.cloudflare.com/client/v4/accounts/$CLOUDFLARE_RUNNER_ACCOUNT_ID"
api() { curl -s -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" "$1" "${@:2}"; }
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
rsecret_id() { rapi "$RB/secrets_store/stores/$RSTORE/secrets?per_page=100" | jq -r --arg n "$1" '.result[] | select(.name == $n) | .id'; }
deployment() { ${3:-api} "${2:-$B}/workers/scripts/$1/deployments" | jq -c --arg n "$1" '.result.deployments[0] | {script: $n, created_on, versions: [.versions[]? | "\(.version_id[0:8]) \(.percentage)"]}'; }
token() { ( umask 077; [ -s "$K/$1" ] || python3 -c 'import os; print(os.urandom(24).hex())' >"$K/$1" ); }

case $STEP in
state)
  { echo "install scripts named graft-lf8-: $(api "$B/workers/scripts" | jq -c '[.result[].id | select(startswith("graft-lf8-"))]')"
    echo "runner scripts: $(rapi "$RB/workers/scripts" | jq -c '[.result[].id]')"
    echo "runner store secrets named $P-: $(rapi "$RB/secrets_store/stores/$RSTORE/secrets?per_page=100" | jq -c --arg p "$P-" '[.result[] | select(.name | startswith($p)) | .name]')"
  } 2>&1 | log ;;
sweep)
  { python3 tests/fanout/teardown.py --sweep 2>&1 | tail -3; echo "exit ${PIPESTATUS[0]}"; } 2>&1 | log ;;
forge)
  { (cd tests/m3/hosted/tools && export PATH=$HOME/.cargo/bin:$PATH && cargo build -q --release --bin lf8forge)
    rm -rf "$K/forge"; ( umask 077; mkdir "$K/forge" )
    tests/m3/hosted/tools/target/release/lf8forge "$K/forge" "$(od -An -tx1 -N32 /dev/urandom | tr -d ' \n')"
    cp "$K/forge/manifest.tsv" "$O/manifest.tsv"
    echo "manifest: $(($(wc -l <"$O/manifest.tsv") - 1)) forgeries, $(cut -f4 "$O/manifest.tsv" | tail -n +2 | sort -u | wc -l) distinct byte strings"
  } 2>&1 | log ;;
secrets)
  # Throwaway: an RSA key for the front's App JWT (the forger checks none) and the front's tokens.
  { token feed.token; token front.token; token ctrl.token; token webhook.secret
    ( umask 077; openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out "$K/app.pem" 2>/dev/null )
    for pair in "AGENT_APP_KEY:app.pem" "WEBHOOK_SECRET:webhook.secret"; do n=${pair%%:*}; f=${pair#*:}
      ( umask 077; jq -Rsc --arg n "$P-$n" '[{name: $n, value: (. | rtrimstr("\n")), scopes: ["workers"], comment: "LF-8 row"}]' <"$K/$f" >"$K/$n.create" )
      rapi "$RB/secrets_store/stores/$RSTORE/secrets" -X POST -H 'content-type: application/json' --data-binary @"$K/$n.create" | jq -c --arg n "$P-$n" '{create: $n, ok: .success, errors: [.errors[]?.code]}'
      shred -u "$K/$n.create"
    done
  } 2>&1 | log ;;
feed)
  { mkdir -p "$K/feed" && cp tests/am3/hosted/lf8/feed.js "$K/feed/feed.js"
    cat >"$K/feed/wrangler.jsonc" <<J
{"name":"$FEED","main":"feed.js","compatibility_date":"2026-11-01","workers_dev":true,"preview_urls":false,
 "durable_objects":{"bindings":[{"name":"FEED","class_name":"Feed"}]},
 "migrations":[{"tag":"v1","new_sqlite_classes":["Feed"]}],"observability":{"enabled":true}}
J
    (cd "$K/feed" && npx --prefix "$OLDPWD/tests/m0" wrangler deploy -c wrangler.jsonc 2>&1 | grep -E "Current Version|rror" | tail -2)
    npx --prefix tests/m0 wrangler secret put LF8_TOKEN --name $FEED <"$K/feed.token" 2>&1 | tail -1
    deployment $FEED
    sleep 5; echo "entries GET, unloaded: $(curl -s -o /dev/null -w '%{http_code}' "$FEED_URL/api/graftyard/v1/release/entries?after=0")"
  } 2>&1 | log ;;
controller)
  { (export PATH=$HOME/.cargo/bin:$PATH && unset CARGO_TARGET_DIR &&
      runner node workers/controller/deploy.mjs $CTRL --secret $RSTORE/$P-WEBHOOK_SECRET 2>&1 | grep -E "Current Version|rror" | tail -2)
    runner npx --prefix tests/m0 wrangler secret put STATUS_TOKEN --name $CTRL <"$K/ctrl.token" 2>&1 | tail -1
    deployment $CTRL "$RB" rapi
  } 2>&1 | log ;;
front)
  # Main's graft-agents-front, unmodified: only its vars point at the forger and the synthetic anchor.
  { (export PATH=$HOME/.cargo/bin:$PATH && unset CARGO_TARGET_DIR &&
      runner node workers/agents-front/deploy.mjs ${3:-$FRONT} --controller $CTRL --anchor "$K/forge/anchor.txt" \
        --host "$FEED.$CF_WORKERS_SUBDOMAIN.workers.dev" --app-id 1 --key $RSTORE/$P-AGENT_APP_KEY 2>&1 | grep -E "Current Version|rror" | tail -2)
    runner npx --prefix tests/m0 wrangler secret put FRONT_TOKEN --name ${3:-$FRONT} <"$K/front.token" 2>&1 | tail -1
    deployment ${3:-$FRONT} "$RB" rapi
    echo "front built from $(git rev-parse --short HEAD), workers/agents-front unchanged from origin/main: $(git diff --quiet origin/main -- workers/agents-front crates/release && echo yes || echo NO)"
  } 2>&1 | log ;;
run)
  { node tests/am3/hosted/lf8/run.mjs "$K/forge" "$FEED_URL" "$FRONT_URL" "$K" "$O/rows.jsonl" main; } 2>&1 | log ;;
cold)
  { node tests/am3/hosted/lf8/run.mjs "$K/forge" "$FEED_URL" "https://$P-cold.$CF_RUNNER_WORKERS_SUBDOMAIN.workers.dev" "$K" "$O/rows.jsonl" cold; } 2>&1 | log ;;
teardown)
  { for n in $FRONT $P-cold $CTRL; do runner npx --prefix tests/m0 wrangler delete --name $n --force 2>&1 | tail -1; done
    npx --prefix tests/m0 wrangler delete --name $FEED --force 2>&1 | tail -1
    for n in $(rapi "$RB/secrets_store/stores/$RSTORE/secrets?per_page=100" | jq -r --arg p "$P-" '.result[] | select(.name | startswith($p)) | .name'); do
      rapi "$RB/secrets_store/stores/$RSTORE/secrets/$(rsecret_id "$n")" -X DELETE | jq -c --arg n "$n" '{delete: $n, ok: .success}'
    done
    find "$K" -type f -exec shred -u {} + ; rm -rf "$K"
    echo "throwaway keys, forgeries and tokens shredded"
    echo "install scripts named graft-lf8-: $(api "$B/workers/scripts" | jq -c '[.result[].id | select(startswith("graft-lf8-"))]')"
    echo "runner scripts: $(rapi "$RB/workers/scripts" | jq -c '[.result[].id]')"
    echo "runner store secrets named $P-: $(rapi "$RB/secrets_store/stores/$RSTORE/secrets?per_page=100" | jq -c --arg p "$P-" '[.result[] | select(.name | startswith($p)) | .name]')"
  } 2>&1 | log ;;
cost)
  { from=${3:-$(date -u +%Y-%m-%dT00:00:00Z)}
    echo "install graft-lf8- from $from: $(PREFIX=graft-lf8 python3 tests/fanout/acctcost.py "$from" | head -1 | jq -c '{prefix_usd}')"
    echo "runner $P- from $from: $(runner env PREFIX=$P python3 tests/fanout/acctcost.py "$from" | head -1 | jq -c '{prefix_usd}')"
  } 2>&1 | log ;;
*) echo "step: state sweep forge secrets feed controller front run cold teardown cost"; exit 2 ;;
esac
