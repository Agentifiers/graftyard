#!/bin/bash
# F137's hosted row on graft-m4a (`tests/am3/results/f137-row.md`): the plain-client baseline, one site day, one edge
# day adding 32.35, 32.36 and 32.37 (whichever are off), the gh and Octokit lines (`f137.mjs`), a teardown that fails
# loudly on any running shard container, and the cost. One step per call, each logged to
# tests/am3/raw/f137-<tag>/<step>.log; the days log under tests/m4/raw/wp3/f137/<tag>/.
#
#   bash tests/am3/hosted/f137-row.sh <tag> <step>
#
# Steps, in order:
#   state     every script's deployment, the edge's FORMATS_ENABLED (count, and which of 32.34 to 32.37 it has), site
#             a's GRAFTYARD_* flags, and the row's spend against the $1.50 stop (STOP printed past it)
#   repo      `f137.mjs repo`: m3org/f137-<tag> with pull request A, issue I, a comment, the label `plain`; the cost
#             clock starts here
#   base      `f137.mjs base`: before any day, every line's request answers as main does and changes nothing. A
#             difference is a FAIL, and the row stops
#   site      a site day from TREE (main b12a07a0, test-faults build done, BUILD_LOG ending "build exit 0"), every live
#             GRAFTYARD_* flag of site a kept, nothing added: the readers of 32.35 to 32.37 and F150's 0x15.15 on every
#             member. Each site's plain-text vars before and after are diffed; any change, or GRAFTYARD_SECRETS_BRANCH
#             set, is SITE DIFF (and the day refuses to start if site a already has that flag)
#   edge      an edge day from TREE, after `site` passed on all three sites: the live FORMATS_ENABLED (all of it) plus
#             whichever of 32.35, 32.36 and 32.37 are off; never 32.34. The binding diff may show only FORMATS_ENABLED
#             gaining exactly those formats (EDGE DIFF otherwise), and edgever must exit 0. WEB is the web build's
#             client directory, copied to /tmp/bf71web (the old copy kept as .prev)
#   octokit | gh   `f137.mjs <line>`, refused past the stop
#   teardown  the repository deleted; the install's shard sweep with --shardkill, failing loudly on any running shard
#             container. 32.35 to 32.37 stay enabled: gated product state, not row residue
#   cost      graft-m4a and the runner account since `repo`
#
# Needs `source tests/am1/lf13/env-m4a.sh`, CF_SECRETS_STORE_ID, CLOUDFLARE_* and CLOUDFLARE_RUNNER_*,
# CF_WORKERS_SUBDOMAIN, ~/.graft-m4a, TREE (a worktree at the main of the row), BUILD_LOG and WEB.
set -uo pipefail
: "${CF_SECRETS_STORE_ID:?export CF_SECRETS_STORE_ID, as M4-WP4 requires}"
TAG=${1:?tag}; STEP=${2:?step}
REPO=$(cd "$(dirname "$0")/../../.." && pwd); cd "$REPO"
O=$REPO/tests/am3/raw/f137-$TAG; mkdir -p "$O"
D=$REPO/tests/m4/raw/wp3/f137/$TAG; mkdir -p "$D"
WD=graft-m4a-edge.$CF_WORKERS_SUBDOMAIN.workers.dev
B="https://api.cloudflare.com/client/v4/accounts/$CLOUDFLARE_ACCOUNT_ID"
STOP=1.50
WANT="32.35 32.36 32.37"
SITES="graft-m4a-site-a graft-m4a-site-b graft-m4a-site-c"
api() { curl -s -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" "$1" "${@:2}"; }
scrub() { sed -e "s/$CLOUDFLARE_ACCOUNT_ID/<install-account>/g" -e "s/$CLOUDFLARE_RUNNER_ACCOUNT_ID/<runner-account>/g" \
  -e "s/$CF_WORKERS_SUBDOMAIN/<sub>/g" -e "s/$CF_SECRETS_STORE_ID/<install-store>/g" -e "s#$HOME/#~/#g" -e "s#$REPO/#<checkout>/#g"; }
N=$(ls "$O" | grep -c "^$STEP\(-[0-9]*\)\?\.log$" || true)
L="$O/$STEP$( [ "$N" -gt 0 ] && echo "-$((N + 1))" ).log"
log() { { date -u +%FT%TZ; git rev-parse --short HEAD; cat; } | scrub >"$L"; cat "$L"; }
deployment() { api "$B/workers/scripts/$1/deployments" | jq -c --arg n "$1" '.result.deployments[0] | {script: $n, created_on, versions: [.versions[]? | "\(.version_id[0:8]) \(.percentage)"]}'; }
plain() { api "$B/workers/scripts/$1/settings" | jq -S '[.result.bindings[] | select(.type == "plain_text") | {(.name): .text}] | add' | scrub; }
formats() { api "$B/workers/scripts/graft-m4a-edge/settings" | jq -r '.result.bindings[] | select(.name == "FORMATS_ENABLED") | .text'; }
has() { case ",$1," in *",$2,"*) echo 1 ;; *) echo 0 ;; esac; }
spend() {  # graft-m4a, and the runner account's graft-agents-m4a- and graft-runners, since `repo`
  [ -s "$O/cost.from" ] || { echo "row not started (no cost.from)"; return 0; }
  local from; from=$(date -u -d @"$(cat "$O/cost.from")" +%FT%TZ)
  a=$(PREFIX=graft-m4a python3 tests/fanout/acctcost.py "$from" | head -1 | jq -r '.prefix_usd // 0')
  r=$(CLOUDFLARE_ACCOUNT_ID=$CLOUDFLARE_RUNNER_ACCOUNT_ID CLOUDFLARE_API_TOKEN=$CLOUDFLARE_RUNNER_API_TOKEN PREFIX=graft-agents-m4a- \
    python3 tests/fanout/acctcost.py "$from" | head -1 | jq -r '.prefix_usd // 0')
  h=$(CLOUDFLARE_ACCOUNT_ID=$CLOUDFLARE_RUNNER_ACCOUNT_ID CLOUDFLARE_API_TOKEN=$CLOUDFLARE_RUNNER_API_TOKEN PREFIX=graft-runners \
    python3 tests/fanout/acctcost.py "$from" | head -1 | jq -r '.prefix_usd // 0')
  echo "row since $from: graft-m4a \$$a, graft-agents-m4a- \$$r, graft-runners \$$h, total \$$(python3 -c "print(round($a + $r + $h, 3))") (stop \$$STOP)"
  python3 -c "import sys; sys.exit(0 if $a + $r + $h < $STOP else 1)" || echo "STOP: the row passed its \$$STOP stop"
}
past_stop() { spend | tee /dev/stderr | grep -q "^STOP"; }
# BF-76's site day: every live GRAFTYARD_* flag of site a that BF-73's day does not set itself is passed on as it is.
site_day() {
  local sets='^GRAFTYARD_(HOST|ACTIVITY|AUDIT|APP_HOOKS|PR_TRIGGER|ACTIONS_CACHE|JOB_TOKENS|OIDC_HEAD_REF|OIDC_CLAIMS|PUSH_TRIGGER|SITES|INSTALL|CUSTOMER_KEY|GENESIS.*)$'
  local extra
  extra=$(api "$B/workers/scripts/graft-m4a-site-a/settings" |
    jq -r --arg s "$sets" '[.result.bindings[] | select(.type == "plain_text" and (.name | test("^GRAFTYARD_")) and (.name | test($s) | not)) | "--var \(.name)=\(.text)"] | join(" ")')
  echo "kept as live: ${extra:-none}; added: none"
  # F150's 0x15.15 reader ships on this day; its op is sent only under GRAFTYARD_SECRETS_BRANCH, which this row never sets.
  case " $extra " in *GRAFTYARD_SECRETS_BRANCH*) echo "STOP: site a has GRAFTYARD_SECRETS_BRANCH; the row must not carry it"; exit 1 ;; esac
  grep -q "build exit 0" "${BUILD_LOG:?}" || { echo "the site build did not finish"; exit 1; }
  mkdir -p "$HOME/bf71" && cp "$BUILD_LOG" "$HOME/bf71/site-build.log"
  ROW=f137 CLAIMS=1 EXTRA="$extra" HOSTV=$WD bash "$REPO/tests/m4/wp3/bf74/row.sh" "$TAG/site" site
}
line() {
  past_stop && { echo "not run: past the stop"; exit 3; }
  node tests/am3/hosted/f137.mjs "$1" "$O" "f137-$TAG"; echo "$1 exit $?"
}

case $STEP in
state)
  { for n in $SITES graft-m4a-edge graft-m4a-ops; do echo "$(deployment $n)"; done
    f=$(formats)
    echo "FORMATS_ENABLED: $(echo "$f" | tr ',' '\n' | wc -l) formats; 32.34 $(has "$f" 32.34), 32.35 $(has "$f" 32.35), 32.36 $(has "$f" 32.36), 32.37 $(has "$f" 32.37); 21.14 $(has "$f" 21.14), 32.32 $(has "$f" 32.32), 32.33 $(has "$f" 32.33)"
    echo "site a flags: $(plain graft-m4a-site-a | jq -c 'to_entries | map(select(.key | test("^GRAFTYARD_") and (test("SITES|INSTALL|CUSTOMER_KEY|GENESIS") | not))) | map("\(.key)=\(.value)")')"
    spend
  } 2>&1 | log ;;
repo) { [ -s "$O/cost.from" ] || date -u +%s >"$O/cost.from"; line repo; } 2>&1 | log ;;
base | octokit | gh) { line "$STEP"; } 2>&1 | log ;;
site)
  { past_stop && { echo "not run: past the stop"; exit 3; }
    grep -q "^VERDICT base PASS" "$O"/base*.log 2>/dev/null || { echo "not run: no passing base"; exit 3; }
    for n in $SITES; do plain $n >"$O/$n-before.json"; done
    site_day
    for n in $SITES; do
      plain $n >"$O/$n-after.json"
      d=$(diff "$O/$n-before.json" "$O/$n-after.json" | grep '^[<>]' | grep -v GRAFTYARD_SITES)
      [ -z "$d" ] && echo "$n: plain-text vars unchanged" || { echo "SITE DIFF $n:"; echo "$d" | cut -c1-120; }
      jq -e 'has("GRAFTYARD_SECRETS_BRANCH")' "$O/$n-after.json" >/dev/null && echo "SITE DIFF $n: GRAFTYARD_SECRETS_BRANCH is set"
    done
  } 2>&1 | log ;;
edge)
  { past_stop && { echo "not run: past the stop"; exit 3; }
    s=$(ls -t "$O"/site*.log 2>/dev/null | head -1)
    { [ -n "$s" ] && grep -q "deploy exit 0" "$s" && [ "$(grep -c "plain-text vars unchanged" "$s")" = 3 ] && ! grep -q "SITE DIFF" "$s"; } \
      || { echo "not run: the last site day did not deploy all three sites with their vars unchanged"; exit 3; }
    [ -d "${WEB:?WEB, the web build}" ] || { echo "no web build at $WEB"; exit 1; }
    before=$(formats); add=""
    for f in $WANT; do [ "$(has "$before" "$f")" = 1 ] || add="${add:+$add,}$f"; done
    echo "already on: $(for f in $WANT; do [ "$(has "$before" "$f")" = 1 ] && printf '%s ' "$f"; done)"; echo "adding: ${add:-none}"
    echo "edge from TREE $(git -C "${TREE:?}" rev-parse --short HEAD)"
    [ -d /tmp/bf71web ] && { rm -rf /tmp/bf71web.prev; mv /tmp/bf71web /tmp/bf71web.prev; }
    cp -r "$WEB" /tmp/bf71web
    [ -n "$add" ] && ROW=f137 ADDFMT=$add HOSTV=$WD bash "$REPO/tests/m4/wp3/bf74/row.sh" "$TAG" edge
    after=$(formats)
    python3 - "$before" "$after" "$add" "$D/edge-before.json" "$D/edge-after.json" <<'PY'
import json, sys
before, after, add, fb, fa = sys.argv[1:]
b, a = set(before.split(",")), set(after.split(","))
want = set(filter(None, add.split(",")))
ok = a - b == want and not b - a and "32.34" not in a - b
print(f"FORMATS_ENABLED {len(b)} -> {len(a)}: added {sorted(a - b)}, removed {sorted(b - a)}")
try:
    vb, va = json.load(open(fb)), json.load(open(fa))
    moved = sorted(k for k in set(vb) | set(va) if vb.get(k) != va.get(k) and k not in ("FORMATS_ENABLED", "GRAFTYARD_SITES"))
except FileNotFoundError:
    moved = []
print(f"other plain-text vars changed: {moved or 'none'}")
print("edge diff ok" if ok and not moved else "EDGE DIFF")
PY
  } 2>&1 | log ;;
teardown)
  { node tests/am3/hosted/f137.mjs delete "$O" "f137-$TAG"; echo "delete exit $?"
    python3 tests/fanout/teardown.py --sweep --shardkill 2>&1 | tail -4; echo "sweep exit ${PIPESTATUS[0]}"
  } 2>&1 | log
  # The day scripts' raw deploy logs and var dumps under $D hold account values; scrub them as `log` does.
  for f in "$D"/*.json "$D"/*.raw "$D"/*/*.raw "$D"/*/*.log; do [ -f "$f" ] && { scrub <"$f" >"$f.s" && mv "$f.s" "$f"; }; done
  if grep -q "SWEEP FAILED\|delete exit [^0]\|sweep exit [^0]" "$L"; then echo "FAIL: teardown failed or containers left running" >&2; exit 2; fi ;;
cost) spend 2>&1 | log ;;
*) echo "step: state repo base site edge octokit gh teardown cost"; exit 2 ;;
esac
