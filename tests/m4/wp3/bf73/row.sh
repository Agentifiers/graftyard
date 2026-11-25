#!/bin/bash
# BF-73 on graft-m4a (`build/BUILD-PLAN.md`, M4-WP3; `decisions/OPEN-QUESTIONS.md`, F119 to F122): a same-repository
# `pull_request` run and a `workflow_dispatch` run each mint a token that a trust policy naming its run's `sub`, `actor`,
# `actor_id`, `head_ref`, `base_ref`, `run_number` and `job_workflow_ref` accepts, and policies naming another actor or
# branch refuse; then the repository's `sub` customization is put and a third run's `sub` follows it.
#
#   bash tests/m4/wp3/bf73/row.sh <tag> <step>
#
# Steps (one per call, each logged to tests/m4/raw/wp3/bf73/<tag>/<step>.log):
#   state | site | edge   BF-74's (`tests/m4/wp3/bf74/row.sh`) with ROW=bf73: `site` takes CLAIMS=1 for the flags day
#                         and HOSTV for the host, `edge` ADDFMT=21.7,21.8; the stop is $3, read before each step
#   pr                    `claims.mjs pr`: the pull request's run and its three policies
#   dispatch              `claims.mjs dispatch`: the dispatch's run, whose run_number is the pull request's + 1
#   custom                `claims.mjs custom`: PUT and GET the repository's sub customization, then a dispatch whose
#                         `sub` is `repo:m3org/<repo>:ref:refs/heads/main:actor:<login>:run_number:<n>`
#   teardown              the repository deleted (its customization row goes with its shard's rows [inference])
#
# Needs what BF-74's row needs, RUNNER (an unpacked actions/runner 2.338.2) and target/release/examples/jitconfig.
set -uo pipefail
# No proxy: the orb's proxy answers CONNECT to a new hostname 502 (BF-74 r1, m4a.graftyard.dev), and Python's
# urllib, the runner and ureq would all take it; BF-70's row ran its runner the same way.
unset HTTP_PROXY HTTPS_PROXY http_proxy https_proxy
TAG=${1:?tag}; STEP=${2:?step}
REPO=$(cd "$(dirname "$0")/../../../.." && pwd)
O=$REPO/tests/m4/raw/wp3/bf73/$TAG; mkdir -p "$O"
export BF73_REPO=${BF73_REPO:-bf73-$TAG}
scrub() { sed -e "s/$CLOUDFLARE_ACCOUNT_ID/<install-account>/g" -e "s/$CF_WORKERS_SUBDOMAIN/<sub>/g" -e "s#$HOME/#~/#g" -e "s#$REPO/#<checkout>/#g"; }
log() { { date -u +%FT%TZ; git -C "$REPO" rev-parse --short HEAD; cat; } | scrub >"$O/$STEP.log"; cat "$O/$STEP.log"; }

case $STEP in
state | site | edge) ROW=bf73 bash "$REPO/tests/m4/wp3/bf74/row.sh" "$TAG" "$STEP" ;;
pr | dispatch | custom)
  { [ -s "$O/cost.from" ] || date -u +%s >"$O/cost.from"
    node "$REPO/tests/m4/wp3/bf73/claims.mjs" "$STEP" "$O"; echo "$STEP exit $?"
  } 2>&1 | log ;;
teardown) { node "$REPO/tests/m4/wp3/bf73/claims.mjs" delete "$O"; echo "delete exit $?"; } 2>&1 | log ;;
*) echo "step: state site edge pr dispatch custom teardown"; exit 2 ;;
esac
