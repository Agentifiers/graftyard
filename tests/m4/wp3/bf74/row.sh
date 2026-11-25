#!/bin/bash
# BF-74 on graft-m4a (`build/BUILD-PLAN.md`, M4-WP3; `decisions/OPEN-QUESTIONS.md`, F130): the install served on a
# custom hostname in graftyard.dev (CUSTOM, default `m4a.graftyard.dev`) as a product install is, with its HOST and the
# sites' GRAFTYARD_HOST on it, so a token's `iss` names it. Then `graftyard oidc check` before the rule, `graftyard oidc
# bic-rule` on that hostname only, the check again, and a token verified by a Python relying party (`rp.py`) that
# discovers the keys from the token's `iss` with urllib's defaults. BF-73's row (`tests/m4/wp3/bf73/row.sh`) runs its
# `state`, `site` and `edge` steps from here.
#
#   bash tests/m4/wp3/bf74/row.sh <tag> <step> [args]
#
# Steps, in order (one per call, each logged to tests/m4/raw/wp3/bf74/<tag>/<step>.log through `scrub`):
#   state      every script's deployment and plain-text vars, ops schedules, the zone's custom domains, DNS records,
#              certificate packs and http_config_settings rules, the install day's cost, and the row's since `domain`
#              (`cost.from`); the stop is $3, read before each step
#   domain     a Workers Custom Domain CUSTOM -> graft-m4a-edge (`PUT /workers/domains`), and waits for its
#              certificate; the edge's workers.dev name stays
#   site       a site day: BF-71's flags (`tests/m4/wp3/bf71/days.sh site`) with GRAFTYARD_HOST=CUSTOM, and with CLAIMS=1
#              also GRAFTYARD_OIDC_HEAD_REF=1 and GRAFTYARD_OIDC_CLAIMS=1 (BF-73), and with PUSH=1 also
#              GRAFTYARD_PUSH_TRIGGER=1 (BF-78); EXTRA (`--var K=V ...`) adds more vars as they are
#   edge       an edge day: the live vars, with HOST=CUSTOM, and ADDFMT's formats (BF-73: 21.7,21.8) added to
#              FORMATS_ENABLED; nothing else may change
# HOSTV replaces CUSTOM in `site` and `edge` (BF-73's days and `back` keep the workers.dev host), and ROW names the
# raw directory (BF-73's row passes bf73).
#   before     `graftyard oidc check --edge https://CUSTOM` (wants 403 1010), discovery's `issuer` read with curl
#   rule       `graftyard oidc bic-rule --zone <graftyard.dev> --host CUSTOM`, then the zone's rules read back
#   after      `graftyard oidc check --edge https://CUSTOM` (wants 200)
#   token      one workflow_dispatch of `token.yml` in m3org/$BF74_REPO through CUSTOM: its job writes one ID token for
#              audience `bf74` to the runner's host (the orb), never to a log
#   rp         `rp.py <token> --iss https://CUSTOM/_services/token --aud bf74`
#   back       the site day (with BF-73's flags) and the edge day again with the workers.dev host, logged under
#              <tag>/back/
#   teardown   the repository deleted, the token file shredded, the rule deleted and the zone's rules read back, and
#              the custom domain deleted unless KEEP=1
#
# Needs CLOUDFLARE_* (the install account, with Zone DNS, SSL and Config Rules edit on graftyard.dev),
# CF_WORKERS_SUBDOMAIN, `source tests/am1/lf13/env-m4a.sh`, ~/.graft-m4a, G (the graftyard binary), RUNNER (an unpacked
# actions/runner 2.338.2) for `token`, and for `site` and
# `edge` the builds `tests/m4/wp3/bf71/days.sh` needs.
set -uo pipefail
# No proxy: the orb's proxy answers CONNECT to a new hostname 502 (BF-74 r1, m4a.graftyard.dev), and Python's
# urllib, the runner and ureq would all take it; BF-70's row ran its runner the same way.
unset HTTP_PROXY HTTPS_PROXY http_proxy https_proxy
TAG=${1:?tag}; STEP=${2:?step}
REPO=$(cd "$(dirname "$0")/../../../.." && pwd)
O=$REPO/tests/m4/raw/wp3/${ROW:-bf74}/$TAG; mkdir -p "$O"
W=$HOME/.graft-m4a; K=$W/bf74; mkdir -p "$K"; chmod 700 "$K"
ZONE=graftyard.dev
# Each tag's own repository: a deleted name stays taken until its retain_until, 90 days (`tests/m4/wp3/oidc/hosted-row.sh`).
export BF74_REPO=${BF74_REPO:-bf74-${TAG%%/*}}
CUSTOM=${CUSTOM:-m4a.$ZONE}
WD=graft-m4a-edge.$CF_WORKERS_SUBDOMAIN.workers.dev
G=${G:-$REPO/target/release/graftyard}
B="https://api.cloudflare.com/client/v4/accounts/$CLOUDFLARE_ACCOUNT_ID"
api() { curl -s -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" "$1" "${@:2}"; }
scrub() { sed -e "s/$CLOUDFLARE_ACCOUNT_ID/<install-account>/g" -e "s/$CF_WORKERS_SUBDOMAIN/<sub>/g" -e "s#$HOME/#~/#g"; }
log() { { date -u +%FT%TZ; git -C "$REPO" rev-parse --short HEAD; cat; } | scrub >"$O/$STEP.log"; cat "$O/$STEP.log"; }
zone_id() { api "https://api.cloudflare.com/client/v4/zones?name=$ZONE" | jq -r '.result[0].id'; }
deployment() { api "$B/workers/scripts/$1/deployments" | jq -c --arg n "$1" '.result.deployments[0] | {script: $n, created_on, versions: [.versions[]? | "\(.version_id[0:8]) \(.percentage)"]}'; }
rules() { api "https://api.cloudflare.com/client/v4/zones/$1/rulesets/phases/http_config_settings/entrypoint" |
  jq -c '{rules: [.result.rules[]? | {ref, expression, action_parameters}], errors: [.errors[]?.code]}'; }
SITES="graft-m4a-site-a graft-m4a-site-b graft-m4a-site-c"
cost() { PREFIX=graft-m4a python3 "$REPO/tests/fanout/acctcost.py" "${1:-$(date -u +%Y-%m-%dT00:00:00Z)}" | head -1 | jq -c '{prefix_usd}'; }

case $STEP in
state)
  { for n in $SITES graft-m4a-edge graft-m4a-ops; do
      echo "$(deployment $n) $(api "$B/workers/scripts/$n/settings" | jq -c '[.result.bindings[] | select(.type == "plain_text" and (.name | test("HOST|FORMATS_ENABLED"))) | "\(.name)=\(.text)"]')"
    done
    api "$B/workers/scripts/graft-m4a-ops/schedules" | jq -c '{ops: [.result.schedules[]?.cron]}'
    Z=$(zone_id)
    echo "custom domains: $(api "$B/workers/domains?zone_name=$ZONE" | jq -c '[.result[] | {hostname, service}]')"
    echo "dns: $(api "https://api.cloudflare.com/client/v4/zones/$Z/dns_records?per_page=100" | jq -c '[.result[] | "\(.type) \(.name)"]')"
    echo "packs: $(api "https://api.cloudflare.com/client/v4/zones/$Z/ssl/certificate_packs?status=all" | jq -c '[.result[]? | {type, hosts, status}]')"
    echo "config rules: $(rules "$Z")"
    echo "install day: $(cost)"
    [ ! -s "$O/cost.from" ] || echo "row so far: $(cost "$(date -u -d @"$(cat "$O/cost.from")" +%FT%TZ)") (stop at \$3)"
  } 2>&1 | log ;;
domain)
  { [ -s "$O/cost.from" ] || date -u +%s >"$O/cost.from"
    Z=$(zone_id)
    jq -nc --arg h "$CUSTOM" --arg z "$Z" '{hostname: $h, service: "graft-m4a-edge", zone_id: $z, environment: "production"}' |
      api "$B/workers/domains" -X PUT -H 'content-type: application/json' --data-binary @- |
      jq -c '{ok: .success, hostname: .result.hostname, service: .result.service, errors: [.errors[]?.code]}'
    # `/_services/` routes by path before HOST is read; `/status` answers 404 here until the edge day moves HOST.
    for i in $(seq 1 60); do
      s=$(curl -s -o /dev/null -w '%{http_code}' -A curl/8 "https://$CUSTOM/_services/token/.well-known/jwks"); [ "$s" = 200 ] && break; sleep 10
    done
    echo "GET https://$CUSTOM/_services/token/.well-known/jwks (curl agent): $s after ${i}0 s"
    echo "GET https://$CUSTOM/status before the edge day: $(curl -s -o /dev/null -w '%{http_code}' -A curl/8 "https://$CUSTOM/status")"
    echo "workers.dev stays: $(curl -s -o /dev/null -w '%{http_code}' -A curl/8 "https://$WD/status")"
  } 2>&1 | log ;;
site)
  # BF-71's days.sh with only the host changed, its REPO pinned to this checkout and its logs written under this row.
  { sed -e "s#GRAFTYARD_HOST=graft-m4a-edge.\$CF_WORKERS_SUBDOMAIN.workers.dev#GRAFTYARD_HOST=${HOSTV:-$CUSTOM}#" \
      -e "s#^REPO=.*#REPO=$REPO#" -e "s#^O=\$REPO/tests/m4/raw/wp3/[^;]*#O=$O#" \
      -e "s#--var GRAFTYARD_JOB_TOKENS=1#--var GRAFTYARD_JOB_TOKENS=1${CLAIMS:+ --var GRAFTYARD_OIDC_HEAD_REF=1 --var GRAFTYARD_OIDC_CLAIMS=1}${PUSH:+ --var GRAFTYARD_PUSH_TRIGGER=1}${EXTRA:+ $EXTRA}#" \
      -e 's#(GRAFTYARD_(JOB|ACTIONS#(GRAFTYARD_(REUSABLE|PUSH|OIDC|JOB|ACTIONS#' "$REPO/tests/m4/wp3/bf71/days.sh" >"$K/days.sh"
    diff "$REPO/tests/m4/wp3/bf71/days.sh" "$K/days.sh" | grep -c '^>'   # 5 with CLAIMS=1 or PUSH=1, else 4: host, flags, REPO, O, vars()
    grep -c "GRAFTYARD_HOST=${HOSTV:-$CUSTOM}" "$K/days.sh"
    bash "$K/days.sh" "${TREE:?TREE, the worktree at the main of the row}" site
  } 2>&1 | log ;;
edge)
  { before=$(api "$B/workers/scripts/graft-m4a-edge/settings" | jq -c '[.result.bindings[] | select(.type == "plain_text") | {(.name): .text}] | add')
    echo "$before" | jq -S . >"$O/edge-before.json"
    fmt=$(echo "$before" | jq -r .FORMATS_ENABLED); rsp=$(echo "$before" | jq -r .RUN_SERVICE_PUBLIC)
    lim=$(echo "$before" | jq -r .GRAFTYARD_CACHE_LIMIT); art=$(echo "$before" | jq -r '.GRAFTYARD_ARTIFACTS // empty')
    for f in ${ADDFMT:+${ADDFMT//,/ }}; do case ",$fmt," in *",$f,"*) ;; *) fmt="$fmt,$f" ;; esac; done
    echo "before $(deployment graft-m4a-edge)"
    (cd "${TREE:?}" && export PATH=$HOME/.cargo/bin:$PATH && unset CARGO_TARGET_DIR && CARGO_INCREMENTAL=0 \
      node tests/m1/integ/deploy-edge.mjs "$W/sites.json" graft-m4a-edge --test-faults --anchor "$W/install.toml" --ops graft-m4a-ops \
        --web /tmp/bf71web --var "FORMATS_ENABLED=$fmt" --var "RUN_SERVICE_PUBLIC=$rsp" --var "GRAFTYARD_CACHE_LIMIT=$lim" \
        ${art:+--var GRAFTYARD_ARTIFACTS=$art} --var "HOST=${HOSTV:-$CUSTOM}" >"$O/edge-deploy.raw" 2>&1); echo "deploy exit $?"
    grep -E "Current Version|rror" "$O/edge-deploy.raw" | tail -4
    api "$B/workers/scripts/graft-m4a-edge/settings" | jq -c '[.result.bindings[] | select(.type == "plain_text") | {(.name): .text}] | add' | jq -S . >"$O/edge-after.json"
    echo "binding diff (plain text; only HOST and FORMATS_ENABLED by ADDFMT may change):"; diff "$O/edge-before.json" "$O/edge-after.json" | grep '^[<>]' | grep -v GRAFTYARD_SITES | cut -c1-80
    echo "after $(deployment graft-m4a-edge)"
    (cd "$TREE" && python3 tests/fanout/edgever.py "$O/edge-deploy.raw" graft-m4a-edge); echo "edgever exit $?"
  } 2>&1 | log ;;
before | after)
  { "$G" oidc check --edge "https://$CUSTOM"; echo "check exit $?"
    echo "issuer: $(curl -s -A curl/8 "https://$CUSTOM/_services/token/.well-known/openid-configuration" | jq -r .issuer)"
  } 2>&1 | log ;;
rule)
  { Z=$(zone_id); echo "rules before: $(rules "$Z")"
    "$G" oidc bic-rule --zone "$Z" --host "$CUSTOM"; echo "bic-rule exit $?"
    "$G" oidc bic-rule --zone "$Z" --host "$CUSTOM"; echo "again exit $? (no write)"
    echo "rules after: $(rules "$Z")"
  } 2>&1 | log ;;
token)
  { GRAFT_EDGE=https://$CUSTOM node "$REPO/tests/m4/wp3/bf74/token.mjs" "$K/token.jwt"; echo "token exit $?"
    [ -s "$K/token.jwt" ] && echo "token written to ~/.graft-m4a/bf74/token.jwt ($(wc -c <"$K/token.jwt") bytes)"
  } 2>&1 | log ;;
rp)
  { python3 "$REPO/tests/m4/wp3/bf74/rp.py" "$K/token.jwt" --iss "https://$CUSTOM/_services/token" --aud bf74
    echo "rp exit $?"
  } 2>&1 | log ;;
back)
  { CLAIMS=1 HOSTV=$WD bash "$0" "$TAG/back" site; HOSTV=$WD bash "$0" "$TAG/back" edge
    echo "issuer: $(curl -s -A curl/8 "https://$WD/_services/token/.well-known/openid-configuration" | jq -r .issuer)"
  } 2>&1 | log ;;
teardown)
  # After `back`, HOST is the workers.dev name again, so the API answers only there (`Host::endpoint`).
  { node "$REPO/tests/m4/wp3/bf74/token.mjs" --delete; echo "repo delete exit $?"
    if [ "${KEEP:-0}" = 0 ]; then
      id=$(api "$B/workers/domains?hostname=$CUSTOM" | jq -r '.result[0].id // empty')
      # The DELETE answers an empty body; the zone's custom domains read again are the record.
      [ -n "$id" ] && api "$B/workers/domains/$id" -X DELETE -o /dev/null -w 'domain DELETE %{http_code}\n'
      echo "custom domains after: $(api "$B/workers/domains?zone_name=$ZONE" | jq -c '[.result[]?.hostname]')"
    else
      echo "the domain $CUSTOM stays (KEEP=1)"
    fi
    # The lead's F130 ruling: no vendor zone keeps a rule. Ours is deleted by its ref; the read-back must hold none.
    Z=$(zone_id); e=$(api "https://api.cloudflare.com/client/v4/zones/$Z/rulesets/phases/http_config_settings/entrypoint")
    rs=$(echo "$e" | jq -r '.result.id // empty'); rid=$(echo "$e" | jq -r '.result.rules[]? | select(.ref == "graftyard-oidc-bic") | .id')
    [ -n "$rid" ] && api "https://api.cloudflare.com/client/v4/zones/$Z/rulesets/$rs/rules/$rid" -X DELETE | jq -c '{rule_deleted: .success, errors: [.errors[]?.code]}'
    echo "rules after: $(rules "$Z")"
    shred -u "$K/token.jwt" 2>/dev/null; echo "token file shredded"
  } 2>&1 | log ;;
*) echo "step: state domain site edge before rule after token rp back teardown"; exit 2 ;;
esac
