#!/bin/bash
# BF-78 on graft-m4a (`build/BUILD-PLAN.md`, M4-WP3; `decisions/DECISIONS.md`, "`push-trigger`"): `on: push` with
# branch, tag, `branches-ignore` and `paths` filters, pushes that match and pushes that do not, a tag push, a force
# push, and a branch delete; one job with `id-token: write`. The plan is `tests/m4/results/m4-wp3-bf78.md`.
#
#   bash tests/m4/wp3/bf78/row.sh <tag> <step>
#
# Steps, in order (one per call, each logged to tests/m4/raw/wp3/bf78/<tag>/<step>.log; the stop is $3, read by
# `state` before each step):
#   state      BF-74's (`tests/m4/wp3/bf74/row.sh state`) with ROW=bf78
#   readers    a site day from the row's main (TREE) that ships the 0x15.12 reader, every flag as it is, and no
#              GRAFTYARD_PUSH_TRIGGER: BF-73's day (`bf74/row.sh site`, CLAIMS=1, HOSTV the workers.dev host)
#   edge       optional: an edge day adding 21.12 to FORMATS_ENABLED (`bf74/row.sh edge`, ADDFMT=21.12, HOSTV the
#              workers.dev host). No edge code reads 0x15.12 and the edge sends no push op, so it records F124's enable
#              only; the plan rides AM2's next edge day for it instead
#   flag       a site day from the same TREE with GRAFTYARD_PUSH_TRIGGER=1 added and nothing else changed
#   push       `push.mjs`: the repository, its five workflows, nine pushes, every job, and the checks
#   off        a site day from the same TREE without GRAFTYARD_PUSH_TRIGGER, so no other row's push starts runs
#   teardown   the repository deleted
#
# Needs what BF-74's row needs, RUNNER (an unpacked actions/runner 2.338.2), target/release/examples/jitconfig, and
# TREE (a worktree at the row's main, with `worker-build --release --features test-faults` done in workers/site).
set -uo pipefail
unset HTTP_PROXY HTTPS_PROXY http_proxy https_proxy
TAG=${1:?tag}; STEP=${2:?step}
REPO=$(cd "$(dirname "$0")/../../../.." && pwd)
O=$REPO/tests/m4/raw/wp3/bf78/$TAG; mkdir -p "$O"
export BF78_REPO=${BF78_REPO:-bf78-$TAG}
WD=graft-m4a-edge.$CF_WORKERS_SUBDOMAIN.workers.dev
scrub() { sed -e "s/$CLOUDFLARE_ACCOUNT_ID/<install-account>/g" -e "s/$CF_WORKERS_SUBDOMAIN/<sub>/g" -e "s#$HOME/#~/#g" -e "s#$REPO/#<checkout>/#g"; }
log() { { date -u +%FT%TZ; git -C "$REPO" rev-parse --short HEAD; cat; } | scrub >"$O/$STEP.log"; cat "$O/$STEP.log"; }
# BF-73's site day (its flags, the workers.dev host), with PUSH=1 adding GRAFTYARD_PUSH_TRIGGER=1. `deploy.mjs` writes
# only the vars it is given, so a day without PUSH removes the flag. Every other live `GRAFTYARD_*` flag of site a that
# the day does not set itself (BF-72's GRAFTYARD_REUSABLE) is passed on as it is.
site_day() {
  local sets='^GRAFTYARD_(HOST|ACTIVITY|AUDIT|APP_HOOKS|PR_TRIGGER|ACTIONS_CACHE|JOB_TOKENS|OIDC_HEAD_REF|OIDC_CLAIMS|PUSH_TRIGGER|SITES|INSTALL|CUSTOMER_KEY|GENESIS.*)$'
  local extra
  extra=$(curl -s -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" \
    "https://api.cloudflare.com/client/v4/accounts/$CLOUDFLARE_ACCOUNT_ID/workers/scripts/graft-m4a-site-a/settings" |
    jq -r --arg s "$sets" '[.result.bindings[] | select(.type == "plain_text" and (.name | test("^GRAFTYARD_")) and (.name | test($s) | not)) | "--var \(.name)=\(.text)"] | join(" ")')
  echo "kept as live: ${extra:-none}"
  ROW=bf78 CLAIMS=1 PUSH=$1 EXTRA=$extra HOSTV=$WD bash "$REPO/tests/m4/wp3/bf74/row.sh" "$TAG/$STEP" site
}

case $STEP in
state) ROW=bf78 bash "$REPO/tests/m4/wp3/bf74/row.sh" "$TAG" state ;;
readers) { [ -s "$O/cost.from" ] || date -u +%s >"$O/cost.from"; site_day ""; } 2>&1 | log ;;
edge) { ROW=bf78 ADDFMT=21.12 HOSTV=$WD bash "$REPO/tests/m4/wp3/bf74/row.sh" "$TAG" edge; } 2>&1 | log ;;
flag) { site_day 1; } 2>&1 | log ;;
off) { site_day ""; } 2>&1 | log ;;
push) { node "$REPO/tests/m4/wp3/bf78/push.mjs" "$O/push"; echo "push exit $?"; } 2>&1 | log ;;
teardown) { node "$REPO/tests/m4/wp3/bf78/push.mjs" delete "$O"; echo "delete exit $?"; } 2>&1 | log ;;
*) echo "step: state readers edge flag push off teardown"; exit 2 ;;
esac
