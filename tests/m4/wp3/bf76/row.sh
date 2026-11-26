#!/bin/bash
# BF-76 on graft-m4a (`build/BUILD-PLAN.md`, M4-WP3; `design/SHARED.md`, "Toolkit hostname heuristics and SHA pins";
# `decisions/DECISIONS.md`, "`pin-map`"): `upload-artifact@v4` and `download-artifact@v4` through the
# `graftyard-actions` forks, the vendor's signed pin map, an unapproved SHA pin failing as on GHES with the pin-map
# annotation, and the same pin passing once the org admin approves the map. The plan is
# `tests/m4/results/m4-wp3-bf76.md`.
#
#   bash tests/m4/wp3/bf76/row.sh <tag> <step>
#
# Steps, in order (one per call, each logged to tests/m4/raw/wp3/bf76/<tag>/<step>.log; the stop is $3, read by
# `state` before each step):
#   state      BF-74's (`tests/m4/wp3/bf74/row.sh state`) with ROW=bf76
#   site       a site day from TREE: BF-73's day (CLAIMS=1, the workers.dev host), every live GRAFTYARD_* flag of site a
#              kept, GRAFTYARD_ARTIFACTS_V4=1 added, no GRAFTYARD_PUSH_TRIGGER
#   edge       an edge day from TREE adding 21.10 and 18.15 to the live FORMATS_ENABLED (`bf74/row.sh edge`)
#   setup      the forks built (`build-forks.sh`, MIR the upstream bare clones), mirrored as `graftyard-actions/*`
#              (GA_ID, default 7100013), upstream v4.6.2 and v4.3.0 mirrored into `actions`, and the pin map signed
#              with ~/.graft-m4a/release.key and stored (`bf76.mjs setup`)
#   tag        `bf76.mjs tag`, `pin` `bf76.mjs pin`, `approve` `bf76.mjs approve` (RUNNER, an unpacked runner 2.338.2);
#              FROM=<tag> takes an earlier tag's signed map and digest when `setup` is not rerun
#   withdraw   `bf76.mjs withdraw`: `{tags: false, pin_map: null}`, which must leave no approval (r2's first line)
#   teardown   `bf76.mjs teardown`, then the shard-container sweep
#
# Needs `source tests/am1/lf13/env-m4a.sh` (the tools token for the mirrors), what BF-74's row needs, RUNNER, target/release/examples/jitconfig, and TREE (a worktree at the row's main, with
# `worker-build --release --features test-faults` done in workers/site).
set -uo pipefail
unset HTTP_PROXY HTTPS_PROXY http_proxy https_proxy
TAG=${1:?tag}; STEP=${2:?step}
REPO=$(cd "$(dirname "$0")/../../../.." && pwd)
O=$REPO/tests/m4/raw/wp3/bf76/$TAG; mkdir -p "$O"
export BF76_REPO=${BF76_REPO:-bf76-$TAG}
WD=graft-m4a-edge.$CF_WORKERS_SUBDOMAIN.workers.dev
export GRAFT_EDGE=https://$WD
# A worktree without tests/m0's install takes the main checkout's (`git rev-parse --git-common-dir`).
W0=$REPO/tests/m0/node_modules/wrangler/bin/wrangler.js
[ -e "$W0" ] || W0=$(cd "$(git -C "$REPO" rev-parse --git-common-dir)/.." && pwd)/tests/m0/node_modules/wrangler/bin/wrangler.js
export WRANGLER=${WRANGLER:-$W0}
scrub() { sed -e "s/$CLOUDFLARE_ACCOUNT_ID/<install-account>/g" -e "s/$CF_WORKERS_SUBDOMAIN/<sub>/g" -e "s#$HOME/#~/#g" -e "s#$REPO/#<checkout>/#g"; }
log() { { date -u +%FT%TZ; git -C "$REPO" rev-parse --short HEAD; cat; } | scrub >"$O/$STEP.log"; cat "$O/$STEP.log"; }
# BF-78's site day: every live `GRAFTYARD_*` flag of site a that BF-73's day does not set itself is passed on as it is.
site_day() {
  local sets='^GRAFTYARD_(HOST|ACTIVITY|AUDIT|APP_HOOKS|PR_TRIGGER|ACTIONS_CACHE|JOB_TOKENS|OIDC_HEAD_REF|OIDC_CLAIMS|PUSH_TRIGGER|ARTIFACTS_V4|SITES|INSTALL|CUSTOMER_KEY|GENESIS.*)$'
  local extra
  extra=$(curl -s -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" \
    "https://api.cloudflare.com/client/v4/accounts/$CLOUDFLARE_ACCOUNT_ID/workers/scripts/graft-m4a-site-a/settings" |
    jq -r --arg s "$sets" '[.result.bindings[] | select(.type == "plain_text" and (.name | test("^GRAFTYARD_")) and (.name | test($s) | not)) | "--var \(.name)=\(.text)"] | join(" ")')
  echo "kept as live: ${extra:-none}"
  ROW=bf76 CLAIMS=1 EXTRA="$extra --var GRAFTYARD_ARTIFACTS_V4=1" HOSTV=$WD bash "$REPO/tests/m4/wp3/bf74/row.sh" "$TAG/$STEP" site
}

case $STEP in
state) ROW=bf76 bash "$REPO/tests/m4/wp3/bf74/row.sh" "$TAG" state ;;
site) { [ -s "$O/cost.from" ] || date -u +%s >"$O/cost.from"; site_day; } 2>&1 | log ;;
edge) { ROW=bf76 ADDFMT=21.10,18.15 HOSTV=$WD bash "$REPO/tests/m4/wp3/bf74/row.sh" "$TAG" edge; } 2>&1 | log ;;
setup)
  { F=${FORKS:-$HOME/tmp/forks}
    bash "$REPO/tests/m4/wp3/bf76/build-forks.sh" "$F" ${MIR:+"$MIR"}; echo "build-forks exit $?"
    MIRROR_ORG=graftyard-actions MIRROR_ORG_ID=${GA_ID:-7100013} node "$REPO/tests/m4/wp3/bf69/mirror.mjs" "$F/upload-artifact.git" upload-artifact v4 v4.6.2; echo "fork upload exit $?"
    MIRROR_ORG=graftyard-actions MIRROR_ORG_ID=${GA_ID:-7100013} node "$REPO/tests/m4/wp3/bf69/mirror.mjs" "$F/download-artifact.git" download-artifact v4 v4.3.0; echo "fork download exit $?"
    node "$REPO/tests/m4/wp3/bf69/mirror.mjs" "$F/upload-artifact.src.git" upload-artifact v4.6.2; echo "upstream upload exit $?"
    node "$REPO/tests/m4/wp3/bf69/mirror.mjs" "$F/download-artifact.src.git" download-artifact v4.3.0; echo "upstream download exit $?"
    node "$REPO/tests/m4/wp3/bf76/bf76.mjs" setup "$O/run"; echo "setup exit $?"
  } 2>&1 | log ;;
tag | pin | approve)
  # FROM=<tag>: a rerun that skips `setup` takes that tag's signed map and digest (the install still holds them).
  [ -n "${FROM:-}" ] && mkdir -p "$O/run" && for f in digest.txt pin-map.json pin-map.sig; do
    [ -e "$O/run/$f" ] || cp "$REPO/tests/m4/raw/wp3/bf76/$FROM/run/$f" "$O/run/$f"; done
  { node "$REPO/tests/m4/wp3/bf76/bf76.mjs" "$STEP" "$O/run" "${RUNNER:?RUNNER}"; echo "$STEP exit $?"; } 2>&1 | log ;;
withdraw) { node "$REPO/tests/m4/wp3/bf76/bf76.mjs" withdraw "$O/run"; echo "withdraw exit $?"; } 2>&1 | log ;;
teardown)
  { node "$REPO/tests/m4/wp3/bf76/bf76.mjs" teardown "$O/run"; echo "teardown exit $?"
    python3 "$REPO/tests/fanout/teardown.py" --sweep --shardkill; echo "sweep exit $?"
  } 2>&1 | log ;;
*) echo "step: state site edge setup withdraw tag pin approve teardown"; exit 2 ;;
esac
