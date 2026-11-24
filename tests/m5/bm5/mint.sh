#!/usr/bin/env bash
# BM-5 clause 3 hosted on graft-m4a (build/BUILD-PLAN.md, M5-WP7): "A bad `graft-ops` at 1% swaps back by `OpsPointer`
# with the API blocked." Stop $3 (the lead's ruling, 2026-11-24).
#
# graft-ops from this tree uploads two ops-only releases under seq tags: N, the plain test-faults build, rolled to
# 100%; then N+1, the `test-mint-fail` build, whose every mint answers 503. `release roll N+1` starts its `OpsRoll`
# at 1% with the `OpsPointer` to N pre-signed. Once the 1% split is live, graft-ops' deploy token is replaced by an
# invalid value, so graft-ops can read and deploy nothing. Installation-token mints through an edge built from this
# tree (`MINT_ERROR_RATIO`, `MintWatch`) then fail on N+1. Pass: an edge isolate commits the `OpsPointer` with the API
# blocked, and once unblocked graft-ops returns to N at 100% and ends the intent.
#
#   bash tests/m5/bm5/mint.sh <tag> <phase> [args]
#
# Phases, in order:
#   bundles     N (ops/ from $OUT/ops-n) and N+1 (ops/ from $OUT/ops-n1), each with clause 1's formats.txt, signed
#   setup       graft-ops' deploy token minted and PATCHed in; graft-m4a-ops from $DEPLOY (test-faults, GATE=pass,
#               60 s step dwell, FIRST_DAY_MS for the 1% step, 1-minute cron, three sites and ops); N put under bm5m-n/
#   roll n      supervised `release roll N` until done
#   put         N+1 put under bm5m-n1/, once N's ReleaseUploaded committed (so N+1 takes the next seq)
#   edge        a version of graft-m4a-edge from $OUT/edge with every binding kept, uploaded and deployed at 100%
#   start       `release roll N+1` once, graft-ops' deployment watched until N+1 runs at 1%, then `block`
#   block       DEPLOY_TOKEN set to an invalid value, with a probe under it
#   load <n> <c>  up to n installation-token mints through the edge, c at a time, 100 a batch: each batch's statuses
#               and the group's view, until the `OpsPointer` committed or the $3 stop
#   unblock     DEPLOY_TOKEN back to the row token
#   watch       until IntentDone: the group's state and graft-ops' deployment each minute
#   checks      the group's entries from `FROM`, graft-ops' deployment, the edge's "mint undo" and MINT_ERROR_RATIO
#               lines (Workers Logs)
#   edgeback    graft-m4a-edge back to eef95142 at 100% and f4e193b9 at 0%, by version id
#   park        graft-ops' cron off, graft-ops back to ac4cd9a5 by version id, its deploy token revoked and shredded
#
# Needs the graft-m4a env (tests/am1/lf13/env-m4a.sh), OUT (ops-n/, ops-n1/, edge/ worker-build outputs, TREE),
# DEPLOY (the worktree OUT was built from), G (graftyard), RELVIEW, NODE_MODULES. Every log goes through scrub; token
# values stay in 0600 files.
set -uo pipefail
cd "$(dirname "$0")/../../.."; ROOT=$PWD
TAG=${1:?tag}; PH=${2:?phase}
O=tests/m5/results/raw/bm5/$TAG; mkdir -p "$O"
W=$HOME/.graft-m4a; K=$W/bm5m; mkdir -p "$K"; chmod 700 "$K"
OUT=${OUT:?the builds}; DEPLOY=${DEPLOY:?the worktree OUT was built from}; G=${G:?graftyard}
export RELVIEW=${RELVIEW:?relview}
STORE=bfaa0c1573de4775b1b92a12fb89acb4
OPS=graft-m4a-ops; EDGE=graft-m4a-edge; PARKED=ac4cd9a5-0fc2-400c-8dc8-fdf9391470ef; BUCKET=graft-m4a-releases
EDGE_N=eef95142-d0a1-441a-9e8f-695d412ec51a; EDGE_KEPT=f4e193b9-9e8a-4f2b-a761-786ad041966f
B="https://api.cloudflare.com/client/v4/accounts/$CLOUDFLARE_ACCOUNT_ID"
api() { curl -s -m 120 -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" "$B$1" "${@:2}"; }
scrub() {
  sed -e "s/$CLOUDFLARE_ACCOUNT_ID/<install-account>/g" -e "s/$CF_WORKERS_SUBDOMAIN/<install-subdomain>/g" \
      -e "s/$CF_ZONE_ID/<zone-id>/g" -e "s#$PWD/#<checkout>/#g" -e "s#$HOME/#~/#g"
}
N=$(ls "$O" | grep -c "^$PH" || true)
L="$O/$PH$( [ "$N" -gt 0 ] && echo "-$((N + 1))" ).log"
log() { { date -u +%FT%TZ; cat; } | scrub >"$L"; cat "$L"; }
[ -e tests/m0/node_modules ] || ln -sfn "${NODE_MODULES:?}" tests/m0/node_modules
wrangler() { npx --prefix "$ROOT/tests/m0" wrangler "$@"; }
SCRIPTS_WRITE=e086da7e2179491d91ee5f35b3ca210a
CONTAINERS_WRITE=bdbcd690c763475a985e8641dddc09f7
mint() {
  local file=$1 name=$2; shift 2
  local groups; groups=$(printf '{"id":"%s"},' "$@"); groups="[${groups%,}]"
  ( umask 077
    jq -nc --arg n "$name" --arg r "com.cloudflare.api.account.$CLOUDFLARE_ACCOUNT_ID" --argjson g "$groups" \
      '{name: $n, policies: [{effect: "allow", resources: {($r): "*"}, permission_groups: $g}]}' |
      api /tokens -X POST -H 'content-type: application/json' --data-binary @- >"$file" )
  jq -c --arg n "$name" '{mint: $n, ok: .success, errors: [.errors[]?.code]}' "$file"
}
revoke() {
  local id; id=$(api "/tokens?per_page=50" | jq -r --arg n "$1" '.result[] | select(.name == $n) | .id')
  [ -n "$id" ] && api "/tokens/$id" -X DELETE | jq -c --arg n "$1" '{revoke: $n, ok: .success, errors: [.errors[]?.code]}' \
    || echo "{\"revoke\": \"$1\", \"found\": false}"
}
deployment() { api "/workers/scripts/$1/deployments" | jq -c --arg s "$1" '.result.deployments[0] | {script: $s, created_on, versions: [.versions[] | "\(.version_id[0:8]) \(.percentage)"]}'; }
tag_of() {
  local d; d=$(api "/workers/scripts/$1/deployments" | jq -c '.result.deployments[0].versions')
  for v in $(echo "$d" | jq -r '.[] | "\(.version_id)/\(.percentage)"'); do
    api "/workers/scripts/$1/versions/${v%/*}" | jq -c --arg s "$1" --arg p "${v#*/}" \
      '(.result.annotations // {}) as $a | {script: $s, version: .result.id[0:8], percentage: ($p|tonumber), tag: (($a["workers/tag"] // "") | if test("^[0-9]+-") then .[0:12] else .[0:8] end)}'
  done
}
spend() { PREFIX=graft-m4a python3 tests/fanout/acctcost.py "$(cat "$O/setup.iso")" 2>/dev/null | head -1 | jq -r .prefix_usd; }
rid() { sha256sum "$1/release.json" | cut -c1-64; }
F=$(grep -E '^(install|customer_key) ' "$W/install.toml" | sed -E 's/.*= *"([^"]*)"/\1/')
view() { node tests/m5/bf46/view.mjs pages "$K/pages.json" >/dev/null && "$RELVIEW" $F "${1:-0}" <"$K/pages.json"; }
roll() {
  node tests/m5/bf46/view.mjs pages "$K/pages.json" >/dev/null
  "$G" --config "$W/install.toml" release roll "$1" --entries "$K/pages.json" --key "$W/customer.key" --edge "$GRAFT_EDGE" 2>&1
}
put() {  # put <dir> <prefix>
  (cd "$1" && find . -type f | sed 's#^\./##' | sort | while read -r f; do
    wrangler r2 object put "$BUCKET/$2/$f" --file "$f" --remote >/dev/null 2>&1 && echo "put $2/$f" || echo "PUT FAILED $2/$f"
  done)
}
BLOCKED=bm5m-blocked-invalid-token
secret_id() { api "/secrets_store/stores/$STORE/secrets?per_page=100" | jq -r --arg n "$1" '.result[] | select(.name == $n) | .id'; }
ops_split() { api "/workers/scripts/$OPS/deployments" | jq -c '[.result.deployments[0].versions[] | "\(.version_id[0:8]) \(.percentage)"]'; }
sign_bundle() {  # sign_bundle <dir> <label>
  python3 - "$1" "$2" <<'PY'
import hashlib, json, os, sys
d, label = sys.argv[1], sys.argv[2]
files = []
for root, _, names in os.walk(d):
    for n in names:
        p = os.path.join(root, n); rel = os.path.relpath(p, d)
        if rel not in ("release.json", "release.sig"):
            files.append({"name": rel, "sha256": hashlib.sha256(open(p, "rb").read()).hexdigest()})
rec = {"files": sorted(files, key=lambda f: f["name"]), "label": label}
open(f"{d}/release.json", "wb").write(json.dumps(rec, sort_keys=True).encode())
PY
  openssl pkeyutl -sign -rawin -inkey "$W/release.key" -in "$1/release.json" -out "$1/release.sig"
  openssl pkeyutl -verify -rawin -pubin -inkey "$W/release.pub" -in "$1/release.json" -sigfile "$1/release.sig" >/dev/null
}
over_stop() { python3 -c "import sys; sys.exit(0 if float('${1:-0}') >= ${STOP_USD:-3} else 1)"; }

case $PH in
bundles)
  { for r in n n1; do
      d=$K/$r; rm -rf "$d"; mkdir -p "$d/ops"
      cp "$W/bm5f/n/formats.txt" "$d/"
      cp "$OUT/ops-$r/index.js" "$OUT/ops-$r/index_bg.wasm" "$d/ops/"
      sign_bundle "$d" "bm5m-$r" && echo "$r $(rid "$d") signed and verified; ops wasm $(sha256sum "$d/ops/index_bg.wasm" | cut -c1-16)"
    done
    echo "builds of $(cat "$OUT/TREE")"
  } 2>&1 | log ;;
setup)
  { date -u +%FT%TZ >"$O/setup.iso"; date -u +%s%3N >"$O/start.ms"
    echo "builds of $(cat "$OUT/TREE")"
    echo "graft-m4a's day to now: \$$(PREFIX=graft-m4a python3 tests/fanout/acctcost.py | head -1 | jq -r .prefix_usd)"
    view 0 | tee "$K/view0.json" | jq -c '{count, current, drained, rolled, uploads: (.uploads | length)}'
    deployment $OPS
    [ -s "$K/deploy-token.json" ] || mint "$K/deploy-token.json" graft-m4a-bm5m-deploy $SCRIPTS_WRITE $CONTAINERS_WRITE
    ( umask 077; jq -c '{value: .result.value}' "$K/deploy-token.json" >"$K/deploy-token.value" )
    api "/secrets_store/stores/$STORE/secrets/$(secret_id graft-m4a-ops-deploy-token)" -X PATCH -H 'content-type: application/json' \
      --data-binary @"$K/deploy-token.value" | jq -c '{patch: "graft-m4a-ops-deploy-token", ok: .success, errors: [.errors[]?.code]}'
    shred -u "$K/deploy-token.value"
    jq --argjson fd "${FIRST_DAY_MS:-120000}" '.scripts += {"ops": "graft-m4a-ops"} | .gate = "pass" | .cron = "* * * * *"
        | .step_dwell_ms = 60000 | .first_day_ms = $fd' "$W/ops/exec-ck-pending.json" >"$W/ops/exec-bm5m.json"
    jq -c '{scripts, gate, cron, step_dwell_ms, first_day_ms, step_min_entries, test_faults, mint_fail}' "$W/ops/exec-bm5m.json"
    (cd "$DEPLOY" && export PATH=$HOME/.cargo/bin:$PATH \
      && ln -sfn "${NODE_MODULES:?}" tests/m0/node_modules \
      && node workers/ops/deploy.mjs $OPS $STORE graft-m4a-mint-wrap-key - graft-m4a-secrets-wrap-key-b 1 \
           --executor "$W/ops/exec-bm5m.json" 2>&1 | grep -E "Current Version|rror" | tail -3)
    deployment $OPS
    # A new cron fires only after its schedule is reset (BF-47).
    api "/workers/scripts/$OPS/schedules" -X PUT -H 'content-type: application/json' --data '[{"cron":"0 0 1 1 *"}]' >/dev/null
    sleep 2
    api "/workers/scripts/$OPS/schedules" -X PUT -H 'content-type: application/json' --data '[{"cron":"* * * * *"}]' |
      jq -c '{ops_schedules: [.result.schedules[]?.cron]}'
    put "$K/n" bm5m-n; echo "N $(rid "$K/n")"
  } 2>&1 | log ;;
put)
  { view 0 | jq -c --arg n "$(rid "$K/n" | cut -c1-8)" '{count, uploads: (.uploads | length), n_uploaded: (.uploads | index($n) != null), rolled}'
    put "$K/n1" bm5m-n1; echo "N+1 $(rid "$K/n1")"
  } 2>&1 | log ;;
roll)
  R=$(rid "$K/${3:?n}"); RL=$O/roll-$3.log
  say() { echo "$(date -u +%FT%TZ) $*" | scrub | tee -a "$RL"; }
  say "release roll $3 $R, stop \$${STOP_USD:-3} from $(cat "$O/setup.iso")"
  i=0
  while true; do
    v=$(view 0 2>&1 | jq -c '{count, current, rolled, uploads: (.uploads | length)}') || { say "view: $v"; sleep 30; continue; }
    if [ $((i % 5)) -eq 0 ]; then
      s=$(spend); say "spend \$$s; $v; ops $(ops_split)"
      over_stop "$s" && { say "STOP at \$$s"; exit 1; }
    fi
    i=$((i + 1))
    if [ "$(echo "$v" | jq -c .current)" = null ]; then
      a=$(roll "$R"); say "roll: $(echo "$a" | tr '\n' ' ')"
      echo "$a" | grep -q '"roll": *"done"' && { say "DONE"; exit 0; }
    fi
    sleep 60
  done ;;
edge)
  { live=$(api "/workers/scripts/$EDGE/deployments" | jq -r '.result.deployments[0].versions | max_by(.percentage) | .version_id')
    echo "edge live ${live:0:8}"
    # Runtime settings and vars from eef95142, the edge this row returns to, whatever runs now.
    node tests/m5/bm5/upload-edge.mjs "$OUT/edge" "$EDGE" "$EDGE_N" >"$K/edge-upload.json"
    cat "$K/edge-upload.json"; nv=$(jq -r .id "$K/edge-upload.json")
    api "/workers/scripts/$EDGE/versions/$nv" | jq -c '{vars: [.result.resources.bindings[] | select(.type=="plain_text") | .name] | sort, types: ([.result.resources.bindings[].type] | group_by(.) | map({(.[0]): length}) | add)}'
    jq -nc --arg v "$nv" '{strategy: "percentage", versions: [{version_id: $v, percentage: 100}], annotations: {"workers/message": "BM-5 clause 3: MintWatch edge"}}' |
      api "/workers/scripts/$EDGE/deployments" -X POST -H 'content-type: application/json' --data-binary @- | jq -c '{deploy: .success, errors: [.errors[]?.code]}'
    deployment $EDGE
  } 2>&1 | log ;;
start)
  R=$(rid "$K/n1"); RL=$O/start.log
  say() { echo "$(date -u +%FT%TZ) $*" | scrub | tee -a "$RL"; }
  # graft-ops' next cron uploads N+1 and commits its ReleaseUploaded; the roll waits for it.
  for _ in $(seq 1 30); do
    a=$(roll "$R" | tr '\n' ' '); say "release roll n1 $R: $a"
    echo "$a" | grep -q "has no ReleaseUploaded" || break
    sleep 30
  done
  for _ in $(seq 1 60); do
    s=$(ops_split); say "ops $s"
    if echo "$s" | grep -q ' 1"'; then bash "$0" "$TAG" block | tee -a "$RL"; exit 0; fi
    sleep 5
  done
  say "no 1% split in 5 minutes"; exit 1 ;;
block)
  { echo "{\"value\":\"$BLOCKED\"}" |
      api "/secrets_store/stores/$STORE/secrets/$(secret_id graft-m4a-ops-deploy-token)" -X PATCH -H 'content-type: application/json' --data-binary @- |
      jq -c '{block: .success, errors: [.errors[]?.code]}'
    curl -s -m 30 -o /dev/null -w 'probe under the blocked value: %{http_code}\n' -H "Authorization: Bearer $BLOCKED" "$B/workers/scripts/$OPS/deployments"
    echo "ops $(ops_split)"
  } 2>&1 | log ;;
unblock)
  { ( umask 077; jq -c '{value: .result.value}' "$K/deploy-token.json" >"$K/deploy-token.value" )
    api "/secrets_store/stores/$STORE/secrets/$(secret_id graft-m4a-ops-deploy-token)" -X PATCH -H 'content-type: application/json' \
      --data-binary @"$K/deploy-token.value" | jq -c '{unblock: .success, errors: [.errors[]?.code]}'
    shred -u "$K/deploy-token.value"
  } 2>&1 | log ;;
load)
  RL=$O/load.log; TOTAL=${3:-1000}; CONC=${4:-16}
  say() { echo "$(date -u +%FT%TZ) $*" | scrub | tee -a "$RL"; }
  say "load: up to $TOTAL mints, $CONC at a time; ops $(ops_split); spend \$$(spend)"
  n=0
  while [ "$n" -lt "$TOTAL" ]; do
    out=$(node tests/m5/bm5/mints.mjs 100 "$CONC" 2>&1 | tail -1)
    n=$((n + 100))
    v=$(view 0 | jq -c '{count, current: (if .current then (.current | {state, next}) else null end), pointer: ([.entries[] | select(.kind == "OpsPointer")] | length)}')
    say "mints $n: $out; $v"
    [ "$(echo "$v" | jq .pointer)" != 0 ] && { say "OpsPointer committed"; break; }
    s=$(spend); over_stop "$s" && { say "STOP at \$$s"; break; }
  done
  say "DONE" ;;
watch)
  RL=$O/watch.log
  say() { echo "$(date -u +%FT%TZ) $*" | scrub | tee -a "$RL"; }
  for _ in $(seq 1 20); do
    v=$(view 0 | jq -c '{count, current: (if .current then (.current | {state, next}) else null end), rolled}')
    say "$v; ops $(ops_split); spend \$$(spend)"
    [ "$(echo "$v" | jq -c .current)" = null ] && { say "DONE"; exit 0; }
    sleep 60
  done ;;
checks)
  { from=$(jq -r .count "$K/view0.json")
    view "$from" | jq -c '{count, current, rolled}'
    view "$from" | jq -c '.entries[] | {index, at: (.at / 1000 | floor | todate), kind, sigs, entry: (.entry | .[0:90])}'
    echo "ops $(ops_split)"
    python3 tests/m5/bm5/edgelogs.py "$EDGE" "$(cat "$O/start.ms")" "$(date -u +%s%3N)"
  } 2>&1 | log ;;
edgeback)
  { jq -nc --arg a "$EDGE_N" --arg b "$EDGE_KEPT" '{strategy: "percentage", versions: [{version_id: $a, percentage: 100}, {version_id: $b, percentage: 0}], annotations: {"workers/message": "BM-5 clause 3: edge back"}}' |
      api "/workers/scripts/$EDGE/deployments" -X POST -H 'content-type: application/json' --data-binary @- | jq -c '{deploy: .success, errors: [.errors[]?.code]}'
    deployment $EDGE
  } 2>&1 | log ;;
park)
  { api "/workers/scripts/$OPS/schedules" -X PUT -H 'content-type: application/json' --data '[{"cron":"0 0 1 1 *"}]' >/dev/null
    sleep 2
    api "/workers/scripts/$OPS/schedules" -X PUT -H 'content-type: application/json' --data '[]' | jq -c '{ops_schedules: .result.schedules}'
    jq -nc --arg v "$PARKED" '{strategy: "percentage", versions: [{version_id: $v, percentage: 100}], annotations: {"workers/message": "BM-5 clause 3: parked"}}' |
      api "/workers/scripts/$OPS/deployments" -X POST -H 'content-type: application/json' --data-binary @- | jq -c '{deploy: .success, errors: [.errors[]?.code]}'
    deployment $OPS
    revoke graft-m4a-bm5m-deploy; shred -u "$K/deploy-token.json" 2>/dev/null; echo "token file shredded"
    echo "spend since setup: \$$(spend)"
  } 2>&1 | log ;;
*) echo "unknown phase $PH"; exit 2 ;;
esac
