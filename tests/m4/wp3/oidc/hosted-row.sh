#!/bin/bash
# BF-70 on graft-m4a (`build/BUILD-PLAN.md`, M4-WP3): Actions OIDC to a relying party across one `Rotate` of
# `ActionsOidcKey`. The relying party is a MinIO STS on the runner's host (the orb), which fetches the install's
# discovery document and JWKS over the internet at start; an unmodified actions/runner 2.338.2 runs `hosted.yml`,
# whose granted job takes configure-aws-credentials v6.2.0 (served from `m3org/bf70-cac`, F90) to it and reads a
# bucket with the credentials, and verifies a second token against the published JWKS itself. Then graft-m4a-ops,
# test-faults with OPS_OIDC_PUBLISHED_MS lowered, executes `Rotate { ActionsOidcKey, slot: 1 }`, and the same job runs
# again: its token must carry slot B's kid, MinIO (which read the JWKS once, before the switch) must take it, and no
# script version may be new. A fork-PR job declaring the grant gets no URL and 403.
#
#   bash tests/m4/wp3/oidc/hosted-row.sh <tag> <step> [args]
#
# Steps, in order (one per call, each logged to tests/m4/raw/wp3/oidc-hosted/<tag>/<step>.log through `scrub`):
#   state       the edge's discovery and JWKS, every site's deployment, version counts, ops schedules, the view's
#               ActionsOidcKey slot, and the install day's cost; the stop is $5 for the row (COST_FROM)
#   minio       a local MinIO in claim mode (`repository_owner` -> policy `m3org`, bucket `probe` only), pointed at
#               the edge's openid-configuration
#   action      `hosted.mjs action`: v6.2.0 on the install as m3org/$BF70_REPO-cac
#   precheck    `hosted.mjs precheck`: one pull_request job that prints only whether ACTIONS_ID_TOKEN_REQUEST_URL is
#               set; the row stops unless it is (r2, the lead's ruling)
#   job <t>     `hosted.mjs job`: the granted and ungranted jobs from a same-repository pull request, tag t (before,
#               after)
#   fork        `hosted.mjs job ... fork`: the fork-PR job
#   token       a row deploy token (scripts and containers write), into graft-m4a-ops-deploy-token; revoked at teardown
#   ops         graft-m4a-ops from REF (default origin/main), test-faults, OPS_OIDC_PUBLISHED_MS=PUBLISHED_MS (default
#               300000), GATE=pending, 1-minute cron
#   fired       waits up to 3 minutes for graft-ops' first scheduled run (`tests/am3/hosted/cron-fired.py`)
#   cron        by hand only if `fired` saw none: r45-row.sh's re-PUT of the 1-minute cron (R52), then `fired` again
#   rotate      sign and post `Rotate { ActionsOidcKey, slot: 1 }` with the customer key
#   view        the `Release` entries since `ops`, and the active slots
#   logs        graft-ops' Rotate lines from Workers Logs since `ops`
#   versions    every script's deployment and version count, to compare with `state`
#   teardown    MinIO removed; m3org/$BF70_REPO, -fork and -cac deleted; graft-m4a-ops parked by version id on the one `ops` found live
#               (OPS_PARKED overrides), cron []; the row token revoked and
#               its files shredded. ActionsOidcKey stays on slot B: both slots stay written and published, and the
#               next rotation (to A) needs no new secret.
#
# Needs CLOUDFLARE_* (the install account), CF_WORKERS_SUBDOMAIN, `source tests/am1/lf13/env-m4a.sh`, ~/.graft-m4a,
# G (the graftyard binary), RELVIEW (tests/m3/hosted/tools), RUNNER (an unpacked actions/runner 2.338.2), CAC (a
# configure-aws-credentials clone with tag v6.2.0), `sudo -n docker`, the aws CLI, and target/release/examples/
# jitconfig (`cargo build --release -p graftyard-actions --example jitconfig`). No token or key is printed.
set -uo pipefail
cd "$(dirname "$0")/../../../.."
TAG=${1:?tag}; STEP=${2:?step}
O=tests/m4/raw/wp3/oidc-hosted/$TAG; mkdir -p "$O"
W=$HOME/.graft-m4a; K=$W/bf70; mkdir -p "$K"; chmod 700 "$K"
STORE=${CF_SECRETS_STORE_ID:?}
# Each tag's own repositories (m3org/bf70-<tag>, -cac, -fork): a deleted name stays taken until its retain_until, 90
# days, so r1's m3org/bf70 and bf70-cac cannot be created again (422 "name already exists"). r1 ran as bf70.
export BF70_REPO=${BF70_REPO:-bf70-$TAG}
OPS=graft-m4a-ops
SCRIPTS="graft-m4a-site-a graft-m4a-site-b graft-m4a-site-c graft-m4a-edge $OPS"
B="https://api.cloudflare.com/client/v4/accounts/$CLOUDFLARE_ACCOUNT_ID"
api() { curl -s -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" "$1" "${@:2}"; }
scrub() {
  sed -e "s/$CLOUDFLARE_ACCOUNT_ID/<install-account>/g" -e "s/$CF_SECRETS_STORE_ID/<store>/g" -e "s/$CF_WORKERS_SUBDOMAIN/<sub>/g" \
      -e "s#$PWD/#<checkout>/#g" -e "s#$HOME/#~/#g"
}
N=$(ls "$O" | grep -c "^$STEP" || true)
L="$O/$STEP$( [ "$N" -gt 0 ] && echo "-$((N + 1))" ).log"
log() { { date -u +%FT%TZ; git rev-parse --short HEAD; cat; } | scrub >"$L"; cat "$L"; }
deployment() { api "$B/workers/scripts/$1/deployments" | jq -c --arg n "$1" '.result.deployments[0] | {script: $n, created_on, versions: [.versions[]? | "\(.version_id[0:8]) \(.percentage)"]}'; }
nversions() { api "$B/workers/scripts/$1/versions?per_page=100" | jq -r '.result.items | length'; }
cost() { PREFIX=graft-m4a python3 tests/fanout/acctcost.py "${1:-$(date -u +%Y-%m-%dT00:00:00Z)}" | head -1 | jq -c '{prefix_usd}'; }
reset_cron() {
  api "$B/workers/scripts/$OPS/schedules" -X PUT -H 'content-type: application/json' --data '[{"cron":"0 0 1 1 *"}]' >/dev/null
  sleep 2
  api "$B/workers/scripts/$OPS/schedules" -X PUT -H 'content-type: application/json' --data '[]' | jq -c '{schedules: .result.schedules}'
}
CONFIG="$GRAFT_EDGE/_services/token/.well-known/openid-configuration"

case $STEP in
state)
  { echo "discovery: $(curl -s -m 20 "$CONFIG" | jq -c '{issuer, jwks_uri}')"
    echo "jwks kids: $(curl -s -m 20 "$GRAFT_EDGE/_services/token/.well-known/jwks" | jq -c '[.keys[].kid]')"
    for n in $SCRIPTS; do echo "$(deployment "$n") versions $(nversions "$n")"; done
    api "$B/workers/scripts/$OPS/schedules" | jq -c '{ops_schedules: [.result.schedules[]?.cron]}'
    node tests/m5/bf46/view.mjs slots | jq -c '{count, current, ActionsOidcKey: (.slots.ActionsOidcKey // 0)}'
    echo "install day: $(cost)"
    [ ! -s "$O/cost.from" ] || echo "row so far: $(cost "$(date -u -d @"$(cat "$O/cost.from")" +%FT%TZ)") (stop at \$5)"
  } 2>&1 | log ;;
minio)
  { unset HTTP_PROXY HTTPS_PROXY http_proxy https_proxy
    [ -s "$K/minio.root" ] || ( umask 077; python3 -c 'import os; print(os.urandom(18).hex())' >"$K/minio.root" )
    sudo -n docker rm -f bf70minio >/dev/null 2>&1
    # MinIO fetches the issuer's documents from the edge's public origin directly, as any relying party would
    # (the orb reaches workers.dev without its proxy: the JWKS answered 200 with --noproxy '*').
    sudo -n docker run -d --name bf70minio --network host -e MINIO_ROOT_USER=root -e "MINIO_ROOT_PASSWORD=$(cat "$K/minio.root")" \
      -e "MINIO_IDENTITY_OPENID_CONFIG_URL=$CONFIG" \
      -e MINIO_IDENTITY_OPENID_CLIENT_ID=sts.amazonaws.com -e MINIO_IDENTITY_OPENID_CLAIM_NAME=repository_owner \
      cgr.dev/chainguard/minio:latest server /data --address 127.0.0.1:9000 >/dev/null || exit 1
    for _ in $(seq 30); do curl -sf http://127.0.0.1:9000/minio/health/live >/dev/null && break; sleep 1; done
    export AWS_ACCESS_KEY_ID=root AWS_SECRET_ACCESS_KEY=$(cat "$K/minio.root")
    aws --endpoint-url http://127.0.0.1:9000 --region us-east-1 s3 mb s3://probe >/dev/null
    echo probe | aws --endpoint-url http://127.0.0.1:9000 --region us-east-1 s3 cp - s3://probe/hello.txt >/dev/null
    unset AWS_ACCESS_KEY_ID AWS_SECRET_ACCESS_KEY
    # The policy the token's `repository_owner` names: the row's repositories are m3org's.
    echo '{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["s3:*"],"Resource":["arn:aws:s3:::probe","arn:aws:s3:::probe/*"]}]}' >"$O/m3org-policy.json"
    mcdir=$(mktemp -d) && chmod 777 "$mcdir"
    mc() { sudo -n docker run --rm --network host -v "$PWD/$O/m3org-policy.json:/p.json" -v "$mcdir:/mc" -e MC_CONFIG_DIR=/mc \
      cgr.dev/chainguard/minio-client:latest "$@"; }
    mc alias set l http://127.0.0.1:9000 root "$(cat "$K/minio.root")" >/dev/null && mc admin policy create l m3org /p.json
    sudo -n rm -rf "$mcdir"
    sudo -n docker logs bf70minio 2>&1 | grep -i -E "openid|error" | head -5
    echo "minio: $(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:9000/minio/health/live)"
  } 2>&1 | log ;;
action)
  { node tests/m4/wp3/oidc/hosted.mjs action "${CAC:?configure-aws-credentials clone}" "$O"; } 2>&1 | log ;;
job)
  # No proxy for the runner: configure-aws-credentials takes HTTP_PROXY for its STS client and ignores NO_PROXY
  # (v6.2.0 `src/index.ts:46,67`), so the call to 127.0.0.1:9000 would go through it.
  { [ -s "$O/cost.from" ] || date -u +%s >"$O/cost.from"
    env -u HTTP_PROXY -u HTTPS_PROXY -u http_proxy -u https_proxy \
      node tests/m4/wp3/oidc/hosted.mjs job "${RUNNER:?runner dir}" "$PWD/$O" "${3:?before|after}"
  } 2>&1 | log ;;
precheck)
  { env -u HTTP_PROXY -u HTTPS_PROXY -u http_proxy -u https_proxy \
      node tests/m4/wp3/oidc/hosted.mjs precheck "${RUNNER:?runner dir}" "$PWD/$O"; } 2>&1 | log ;;
fork)
  { env -u HTTP_PROXY -u HTTPS_PROXY -u http_proxy -u https_proxy \
      node tests/m4/wp3/oidc/hosted.mjs job "${RUNNER:?runner dir}" "$PWD/$O" fork fork; } 2>&1 | log ;;
token)
  { ( umask 077
      jq -nc --arg a "com.cloudflare.api.account.$CLOUDFLARE_ACCOUNT_ID" \
        '{name: "graft-m4a-bf70-deploy", policies: [{effect: "allow", resources: {($a): "*"},
          permission_groups: [{id: "e086da7e2179491d91ee5f35b3ca210a"}, {id: "bdbcd690c763475a985e8641dddc09f7"}]}]}' |
        api "$B/tokens" -X POST -H 'content-type: application/json' --data-binary @- >"$K/deploy.json"
      jq -c '{value: .result.value}' "$K/deploy.json" >"$K/deploy.value" )
    jq -c '{mint: "graft-m4a-bf70-deploy", ok: .success, id: (.result.id // "" | .[0:8]), errors: [.errors[]?.code]}' "$K/deploy.json"
    id=$(api "$B/secrets_store/stores/$STORE/secrets?per_page=100" | jq -r '.result[] | select(.name == "graft-m4a-ops-deploy-token") | .id')
    api "$B/secrets_store/stores/$STORE/secrets/$id" -X PATCH -H 'content-type: application/json' --data-binary @"$K/deploy.value" |
      jq -c '{patch: "graft-m4a-ops-deploy-token", ok: .success, errors: [.errors[]?.code]}'
  } 2>&1 | log ;;
ops)
  # graft-ops from merged main, test-faults, the Rotate's publish wait lowered, the gate held, a 1-minute cron.
  { jq --argjson p "${PUBLISHED_MS:-300000}" '.cron = "* * * * *" | .gate = "pending" | .oidc_published_ms = $p' \
      "$W/ops/exec-ck-pending.json" >"$W/ops/exec-bf70.json"
    jq -c '{test_faults, gate, cron, step_dwell_ms, oidc_published_ms}' "$W/ops/exec-bf70.json"
    date -u +%s%3N >"$O/start.ms"
    # The version the rows before this one left (LM-3's, which B0-02's row does not change): teardown parks on it.
    [ -s "$O/ops.parked" ] || api "$B/workers/scripts/$OPS/deployments" | jq -r '.result.deployments[0].versions[0].version_id' >"$O/ops.parked"
    echo "graft-ops before the row: $(cut -c1-8 "$O/ops.parked")"
    git fetch -q origin main
    rm -rf /tmp/bf70-deploy && git worktree add -f --detach /tmp/bf70-deploy "${REF:-origin/main}" >/dev/null 2>&1
    echo "graft-ops built from $(git -C /tmp/bf70-deploy rev-parse --short HEAD)"
    nm=${NODE_MODULES:-$PWD/tests/m0/node_modules}
    (cd /tmp/bf70-deploy && unset CARGO_TARGET_DIR && export PATH=$HOME/.cargo/bin:$PATH \
      && ln -sfn "$nm" tests/m0/node_modules \
      && node workers/ops/deploy.mjs $OPS $STORE graft-m4a-mint-wrap-key - graft-m4a-secrets-wrap-key-b 1 \
           --executor "$W/ops/exec-bf70.json" 2>&1 | grep -E "Current Version|rror" | tail -3)
    deployment $OPS
    api "$B/workers/scripts/$OPS/schedules" | jq -c '{schedules: [.result.schedules[]?.cron]}'
  } 2>&1 | log ;;
fired)
  # R52: graft-ops' first scheduled run in Workers Logs since `ops` (or since the last `cron` re-PUT), no reset here.
  { since=$(cat "$O/cron.ms" 2>/dev/null || cat "$O/start.ms")
    python3 tests/am3/hosted/cron-fired.py $OPS "* * * * *" --since "$since" --wait-s 180 --tries 0; echo "exit $?"; } 2>&1 | log ;;
cron)
  # R52, by hand if `fired` saw no run: r45-row.sh's `cron` step, a reset then the 1-minute cron again.
  { reset_cron >/dev/null
    api "$B/workers/scripts/$OPS/schedules" -X PUT -H 'content-type: application/json' --data '[{"cron":"* * * * *"}]' | jq -c '{schedules: .result.schedules}'
    date -u +%s%3N >"$O/cron.ms"; echo "re-PUT at $(cat "$O/cron.ms")"
  } 2>&1 | log ;;
rotate)
  { a=$(node tests/m5/bf46/view.mjs prev); echo "prev $(echo "$a" | jq -r .prev | cut -c1-12) count $(echo "$a" | jq -r .count)"
    echo '{"Intent": {"ttl_s": 21600, "intent": {"Rotate": {"secret": "ActionsOidcKey", "slot": 1}}}}' >"$K/rot.json"
    "$G" --config "$W/install.toml" release entry "$K/rot.json" --out "$K/rot.unsigned" --prev "$(echo "$a" | jq -r .prev)" >/dev/null
    "$G" sign "$K/rot.unsigned" --out "$K/rot.signed" --key "$W/customer.key" >/dev/null
    "$G" --config "$W/install.toml" release post "$K/rot.signed" --edge "$GRAFT_EDGE"
    echo "Rotate ActionsOidcKey to slot 1 posted at $(date -u +%s%3N)"
  } 2>&1 | log ;;
view)
  { node tests/m5/bf46/view.mjs entries "$(cat "$O/start.ms")"; } 2>&1 | log ;;
logs)
  { KEEP="Rotate,IntentStep,IntentDone,PAGE,Committed" python3 tests/m5/bf46/logs.py $OPS "$(cat "$O/start.ms")" "$(date +%s%3N)" | tail -40
  } 2>&1 | log ;;
versions)
  { for n in $SCRIPTS; do echo "$(deployment "$n") versions $(nversions "$n")"; done
    echo "row so far: $(cost "$(date -u -d @"$(cat "$O/cost.from")" +%FT%TZ)") (stop at \$5)"
  } 2>&1 | log ;;
teardown)
  { sudo -n docker rm -f bf70minio >/dev/null 2>&1; echo "minio removed"
    node tests/m4/wp3/oidc/hosted.mjs unmirror - "$O"
    for r in "$BF70_REPO" "$BF70_REPO-fork"; do
      echo "DELETE m3org/$r $(curl -s -o /dev/null -w '%{http_code}' -X DELETE -H "authorization: token $(grep '^admin ' "$M3_STATE/m3-tokens.txt" | cut -d' ' -f2)" "$GRAFT_EDGE/api/v3/repos/m3org/$r")"
    done
    reset_cron
    full=${OPS_PARKED:-$(cat "$O/ops.parked")}
    full=$(api "$B/workers/scripts/$OPS/versions?per_page=100" | jq -r --arg p "$full" '.result.items[] | select(.id | startswith($p)) | .id')
    echo "parking graft-ops on ${full:0:8}"
    npx --prefix tests/m0 wrangler versions deploy "$full@100%" --name $OPS -y 2>&1 | grep -E "Deployed|rror" | tail -2
    reset_cron
    deployment $OPS
    id=$(api "$B/tokens?per_page=50" | jq -r '.result[] | select(.name == "graft-m4a-bf70-deploy") | .id')
    [ -n "$id" ] && api "$B/tokens/$id" -X DELETE | jq -c '{revoke: "graft-m4a-bf70-deploy", ok: .success}'
    for f in "$K"/*.value "$K"/*.json "$K"/minio.root "$K"/rot.*; do [ -f "$f" ] && shred -u "$f"; done
    git worktree remove --force /tmp/bf70-deploy 2>/dev/null; echo "row files shredded"
    node tests/m5/bf46/view.mjs slots | jq -c '{count, current, ActionsOidcKey: (.slots.ActionsOidcKey // 0)}'
  } 2>&1 | log ;;
*) echo "step: state minio action precheck job fork token ops fired cron rotate view logs versions teardown"; exit 2 ;;
esac
