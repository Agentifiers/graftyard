#!/usr/bin/env bash
# B0-07 r2 on graft-m4a (build/BUILD-PLAN.md, Milestone 0 status, B0-07; the lead's go, 2026-11-25): a gray site c
# during site b's day at 10%, with `graft-ops`'s automatic drain, on ops from main with `calm-one` (R53's fix). M5-WP4
# row B ran this shape and left two gaps (tests/m5/results/m5-wp4.md, "Row B"): c's undrain deadlocked, and its 12 s
# leader poll could not judge the per-group 20 s line. Pass, every line, all on Cloudflare time:
#   - never two sites drained; graft-ops undrains b under the lead limit and drains c;
#   - c undrains on its own after the lift, with no hand Normal (this driver posts no SiteMode);
#   - the day reaches IntentDone;
#   - the result names how many c-led chains were active (served or committed a request) in the 50 s before the
#     drain, and at least one was: the bench writes to every c-homed chain through the drain;
#   - every chain led on c at the drain moves within 20 s: an active one of the drain, an idle one of its next
#     request, each timed from its own members' election traces (`/elections`, test-faults), not a poll.
# `judge.py` reads the raw records and prints the verdict. Stop $6 (the lead's), counted from `setup`.
#
#   bash tests/m5/b07r2/row.sh <tag> <phase>
#
# Phases, in order:
#   state      the group (count, current, drained, site_release), every script's deployment and schedules, the day's
#              spend, and the clock offset (Cloudflare's Date header against the orb)
#   build      from $TREE (merged main at run time): the site built test-faults into $OUT/site, graft-ops' deploy
#              worktree; RELVIEW and G built if missing
#   siteday    the site day from $TREE: the three sites with BF-70 r2's flags (tests/m4/wp3/oidc/days.sh), the
#              crons reset to [], reretire. Writes $O/siteday.versions
#   bundles    N and N+1 for site b under ~/.graft-m4a/b07r2: the site day's build plus a marker each (N's and N+1's
#              differ only by it), the group's last formats.txt, signed by m4a's release key
#   setup      the deploy token minted and PATCHed in; both bundles put; the scratch Worker deployed with a fresh
#              token; 160 c-homed and 40 a-homed chains created
#   ops <ms>   graft-m4a-ops from $TREE (test-faults, GATE=pass, STEP_DWELL <ms>, 60 s first day,
#              OPS_STEP_MIN_ENTRIES 50, 1-minute cron, the three sites), cron removed and re-added (R52)
#   roll       `ops 60000` first: SiteDay{b, N} with no undo, so N is b's group record and the measured day's N is
#              this tree's code, not an older release's
#              (then `ops 420000` before `run`)
#   run        the measured day, supervised (amp orb service b07r2-run): load, loggers, the fault at b's 10% step,
#              the lift after c's drain, and the wait for c's own undrain and IntentDone, with the $6 stop
#   judge      the raw records into results: entries.jsonl, then judge.py
#   teardown   load and loggers stopped, the fault off; graft-ops' cron reset, parked on ac4cd9a5 by version id, cron
#              [] again; the deploy token revoked and shredded; the scratch Worker and its token deleted
#   lists      tokens named b07r2, ops' deployment and schedules, the scratch Worker's absence, the group's state
#
# Needs: the graft-m4a env (tests/am1/lf13/env-m4a.sh) with CLOUDFLARE_ACCOUNT_ID, CLOUDFLARE_API_TOKEN and
# CF_WORKERS_SUBDOMAIN; ~/.graft-m4a; TREE (a worktree at merged main), RELVIEW, G (the graftyard binary from TREE),
# NODE_MODULES. Token values stay in 0600 files and go only into headers or bodies; every log goes through scrub.
set -uo pipefail
cd "$(dirname "$0")/../../.."; ROOT=$PWD
TAG=${1:?tag}; PH=${2:?phase}
O=tests/m5/results/raw/b07r2/$TAG; mkdir -p "$O"
W=$HOME/.graft-m4a; K=$W/b07r2; mkdir -p "$K"; chmod 700 "$K"
TREE=${TREE:?a worktree at merged main}; OUT=$K/out
STORE=bfaa0c1573de4775b1b92a12fb89acb4
OPS=graft-m4a-ops; PARKED=ac4cd9a5-0fc2-400c-8dc8-fdf9391470ef; BUCKET=graft-m4a-releases
SCRATCH=graft-m4a-b07r2-edge; DEPLOY_TOKEN=graft-m4a-b07r2-deploy
IMG=registry.cloudflare.com/$CLOUDFLARE_ACCOUNT_ID/graft-m1-site-gitshard-git@sha256:e55ab2ad178a4d2b639bf53a3c4ca08694024e344343b2e251821368b1d21d41
SITES="graft-m4a-site-a graft-m4a-site-b graft-m4a-site-c"
B="https://api.cloudflare.com/client/v4/accounts/$CLOUDFLARE_ACCOUNT_ID"
E="https://$SCRATCH.$CF_WORKERS_SUBDOMAIN.workers.dev"
STOP=${STOP_USD:-6}
api() { curl -s -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" "$B$1" "${@:2}"; }
scrub() {
  sed -e "s/$CLOUDFLARE_ACCOUNT_ID/<install-account>/g" -e "s/$CF_WORKERS_SUBDOMAIN/<sub>/g" \
      -e "s#$ROOT/#<checkout>/#g" -e "s#$HOME/#~/#g"
}
N=$(ls "$O" | grep -c "^$PH" || true)
L="$O/$PH$( [ "$N" -gt 0 ] && echo "-$((N + 1))" ).log"
log() { { date -u +%FT%TZ; cat; } | scrub >"$L"; cat "$L"; }
# The scratch Worker, with its bearer token from a 0600 file.
ck() { curl -s -m "${CK_MS:-70}" -H "authorization: Bearer $(cat "$K/scratch.token")" "$E$1" "${@:2}"; }
# Cloudflare's clock: the Date header of an API answer, in ms (second resolution), and the orb's offset from it.
cf_now() { date -u -d "$(curl -sI https://api.cloudflare.com/client/v4 | tr -d '\r' | sed -n 's/^[Dd]ate: //p')" +%s000; }
deployment() { api "/workers/scripts/$1/deployments" | jq -c --arg n "$1" '.result.deployments[0] | {script: $n, created_on, versions: [.versions[]? | "\(.version_id[0:8]) \(.percentage)"]}'; }
schedules() { api "/workers/scripts/$1/schedules" | jq -c --arg n "$1" '{($n): [.result.schedules[]?.cron]}'; }
reset_cron() {
  api "/workers/scripts/$1/schedules" -X PUT -H 'content-type: application/json' --data '[{"cron":"0 0 1 1 *"}]' >/dev/null
  sleep 2
  api "/workers/scripts/$1/schedules" -X PUT -H 'content-type: application/json' --data '[]' | jq -c --arg n "$1" '{($n): .result.schedules}'
}
secret_id() { api "/secrets_store/stores/$STORE/secrets?per_page=100" | jq -r --arg n "$1" '.result[] | select(.name == $n) | .id'; }
spend() { PREFIX=graft-m4a python3 tests/fanout/acctcost.py "$1" 2>/dev/null | head -1 | jq -r .prefix_usd; }
over() { python3 -c "import sys; sys.exit(0 if float('${1:-0}') >= $STOP else 1)"; }
rid() { sha256sum "$1/release.json" | cut -c1-64; }
F=$(grep -E '^(install|customer_key) ' "$W/install.toml" | sed -E 's/.*= *"([^"]*)"/\1/' | xargs)
view() { node tests/m5/bf46/view.mjs pages "$K/pages.json" >/dev/null && "${RELVIEW:?}" $F "${1:-0}" <"$K/pages.json"; }
cur() { view "${1:-0}" | jq -c '{count, current, drained, site_release, uploads}'; }
# post <label> <answer>: every entry this driver posts, for judge.py's hand-Normal check.
posted() { echo "$1 $2" >>"$O/answers.txt"; }
# day <release> [undo release]: a customer-signed `SiteDay{b, release}` with undo `Rollback{b, undo}`, bound to the
# group's last digest and posted. `release roll` would sign site a's day first (the roll order), so the intent is
# posted directly, as row B's was.
day() {
  local v; v=$(view 0) || return 1
  echo "$v" | jq -r .keys_entry | python3 -c "import sys; sys.stdout.buffer.write(bytes.fromhex(sys.stdin.read().strip()))" >"$K/keys.entry"
  jq -nc --arg r "$1" --arg u "${2:-}" --arg k "$W/customer.key" \
    '{Intent: ({ttl_s: 21600, intent: {SiteDay: {target: {Site: 1}, release: $r}}}
       + (if $u == "" then {} else {undo: [{Rollback: {target: {Site: 1}, release: $u}}], undo_key: $k} end))}' >"$K/day.json"
  "${G:?}" --config "$W/install.toml" release entry "$K/day.json" --out "$K/day.unsigned" \
    --prev "$(echo "$v" | jq -r .prev)" --keys "$K/keys.entry" >/dev/null &&
    "$G" sign "$K/day.unsigned" --out "$K/day.signed" --key "$W/customer.key" >/dev/null &&
    "$G" --config "$W/install.toml" release post "$K/day.signed" --edge "$GRAFT_EDGE" 2>&1 | tr '\n' ' '
}

case $PH in
state)
  { echo "orb $(date -u +%s%3N) cloudflare $(cf_now) (Date header, 1 s resolution)"
    cur 0
    for n in $SITES graft-m4a-edge $OPS; do echo "$(deployment $n) $(schedules $n)"; done
    echo "graft-m4a's UTC day: \$$(spend "$(date -u +%Y-%m-%dT00:00:00Z)")"
  } 2>&1 | log ;;
build)
  { echo "tree $(git -C "$TREE" rev-parse --short HEAD)"
    (cd "$TREE/workers/site" && unset CARGO_TARGET_DIR && export PATH=$HOME/.cargo/bin:$PATH &&
       worker-build --release --features test-faults >"$K/site-build.log" 2>&1); echo "site build exit $?"
    rm -rf "$OUT/site"; mkdir -p "$OUT/site"; cp "$TREE/workers/site/build/index.js" "$TREE/workers/site/build/index_bg.wasm" "$OUT/site/"
    git -C "$TREE" rev-parse HEAD >"$OUT/TREE"
    echo "site wasm $(sha256sum "$OUT/site/index_bg.wasm" | cut -c1-16)"
  } 2>&1 | log ;;
siteday)
  # BF-70 r2's site-day flags (tests/m4/wp3/oidc/days.sh), the build from `build`, test-faults.
  { for n in $SITES; do echo "before $(deployment $n)"; done
    (cd "$TREE" && ln -sfn "${NODE_MODULES:?}" tests/m0/node_modules &&
      node workers/site/deploy.mjs "$W/sites.json" --anchor "$W/install.toml" --image "$IMG" --image-prev "$IMG" \
        --vars "$W/site-vars.json" --skip-build --ops $OPS \
        --var GRAFTYARD_HOST=graft-m4a-edge.$CF_WORKERS_SUBDOMAIN.workers.dev --var GRAFTYARD_ACTIVITY=1 --var GRAFTYARD_AUDIT=1 \
        --var GRAFTYARD_APP_HOOKS=1 --var GRAFTYARD_PR_TRIGGER=1 --var GRAFTYARD_ACTIONS_CACHE=1 --var GRAFTYARD_JOB_TOKENS=1 \
        --audit-stream 2a0357004afd44d9a74cf410dd2c715a --run-key $STORE/graft-m4a-run-service-key --oidc $STORE \
        >"$O/siteday-deploy.raw" 2>&1); echo "deploy exit $?"
    grep -E "Current Version|rror" "$O/siteday-deploy.raw" | tail -6
    for n in $SITES; do reset_cron $n; d=$(deployment $n); echo "after $d"; echo "$n $(echo "$d" | jq -r '.versions[0]')" >>"$O/siteday.versions"; done
    RETIRE_LOG=$O/reretire.log bash tests/fanout/reretire.sh tests/fanout/reretire/graft-m4a.txt b07r2-$TAG 2>&1 | tail -1
  } 2>&1 | log ;;
bundles)
  # N and N+1: the same site build, each with a marker, so b's day rolls a new version and N stays real.
  { fmt=$K/formats.txt
    R2_ACCOUNT=$CLOUDFLARE_ACCOUNT_ID R2_TOKEN_VAR=CLOUDFLARE_API_TOKEN \
      python3 tests/m5/vaulthosted/r2.py $BUCKET get bm5f-n/formats.txt "$fmt" >/dev/null
    echo "formats.txt from bm5f-n: $(wc -l <"$fmt") formats"
    for r in n n1; do
      d=$K/$r; rm -rf "$d"; mkdir -p "$d/site"; cp "$fmt" "$d/formats.txt"
      cp "$OUT/site/index.js" "$OUT/site/index_bg.wasm" "$d/site/"
      echo "export const B07R2 = \"b07r2-$TAG-$r\";" >"$d/site/marker.js"
      python3 - "$d" "b07r2-$TAG-$r" <<'PY'
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
      openssl pkeyutl -sign -rawin -inkey "$W/release.key" -in "$d/release.json" -out "$d/release.sig"
      openssl pkeyutl -verify -rawin -pubin -inkey "$W/release.pub" -in "$d/release.json" -sigfile "$d/release.sig" >/dev/null \
        && echo "$r $(rid "$d") signed and verified"
    done
    echo "builds of $(cat "$OUT/TREE")"
  } 2>&1 | log ;;
setup)
  { date -u +%FT%TZ >"$O/setup.iso"; echo "setup from $(cat "$O/setup.iso"); stop \$$STOP"
    cur 0; deployment $OPS
    # graft-ops' deploy token (Workers Scripts Write, Workers Containers Write, Secrets Store Write: without the last a
    # version upload of a script bound to a Secrets Store secret answers 10021, DECISIONS's A54 amendment), PATCHed in.
    [ -s "$K/deploy.json" ] || ( umask 077
      jq -nc --arg a "com.cloudflare.api.account.$CLOUDFLARE_ACCOUNT_ID" --arg n $DEPLOY_TOKEN \
        '{name: $n, policies: [{effect: "allow", resources: {($a): "*"},
          permission_groups: [{id: "e086da7e2179491d91ee5f35b3ca210a"}, {id: "bdbcd690c763475a985e8641dddc09f7"},
            {id: "adc8fa2bc6124928a8b3314dc63a1235"}]}]}' |
        api /tokens -X POST -H 'content-type: application/json' --data-binary @- >"$K/deploy.json" )
    jq -c --arg n $DEPLOY_TOKEN '{mint: $n, ok: .success, id: (.result.id // "" | .[0:8]), errors: [.errors[]?.code]}' "$K/deploy.json"
    ( umask 077; jq -c '{value: .result.value}' "$K/deploy.json" >"$K/deploy.value" )
    api "/secrets_store/stores/$STORE/secrets/$(secret_id graft-m4a-ops-deploy-token)" -X PATCH -H 'content-type: application/json' \
      --data-binary @"$K/deploy.value" | jq -c '{patch: "graft-m4a-ops-deploy-token", ok: .success, errors: [.errors[]?.code]}'
    shred -u "$K/deploy.value"
    # Both bundles into the release bucket; graft-ops uploads them on its next run.
    for r in n n1; do
      (cd "$K/$r" && find . -type f | sed 's#^\./##' | sort | while read -r f; do
        R2_ACCOUNT=$CLOUDFLARE_ACCOUNT_ID R2_TOKEN_VAR=CLOUDFLARE_API_TOKEN \
          python3 "$ROOT/tests/m5/vaulthosted/r2.py" $BUCKET put "b07r2-$TAG-$r/$f" "$f" >/dev/null && echo "put $r/$f" || echo "PUT FAILED $r/$f"
      done)
    done
    # The scratch Worker and its token.
    (cd tests/m5/b07r2 && "$NODE_MODULES/.bin/wrangler" deploy -c wrangler.jsonc 2>&1 | grep -E "Current Version|rror" | tail -2)
    ( umask 077; python3 -c 'import os; print(os.urandom(24).hex())' >"$K/scratch.token" )
    "$NODE_MODULES/.bin/wrangler" secret put TOKEN --name $SCRATCH <"$K/scratch.token" 2>&1 | grep -E "Success|rror" | tail -1
    sleep 5
    # 160 c-homed chains and 40 a-homed ones, on a fresh base so no earlier row's groups are reused.
    base=$((7500000 + RANDOM % 400 * 1000))
    python3 -c "import json,sys; b=int(sys.argv[1]); json.dump({'c': [f'repo:{b+i}' for i in range(160)], 'ab': [f'repo:{b+500+i}' for i in range(40)]}, open(sys.argv[2], 'w'))" \
      "$base" "$O/chains.json"
    echo "chains from repo:$base"
    for side in c ab; do
      home=$([ $side = c ] && echo 2 || echo 0)
      jq -c ".$side" "$O/chains.json" | python3 -c 'import json,sys; xs=json.load(sys.stdin); [print(json.dumps(xs[i:i+20])) for i in range(0, len(xs), 20)]' |
        while read -r part; do
          ck /create -X POST -m 300 -d "{\"chains\": $part, \"home\": $home}" | jq -c '{ok: ([.[] | select(.ok)] | length), of: length}'
        done
    done
    for side in c ab; do
      home=$([ $side = c ] && echo 2 || echo 0)
      ck /voters -X POST -d "{\"chains\": $(jq -c ".$side" "$O/chains.json"), \"home\": $home}" >"$O/voters-$side.json"
      echo "$side voters: $(jq '[.[] | select(all(. != null))] | length' "$O/voters-$side.json") of $(jq length "$O/voters-$side.json")"
    done
    echo "spend since setup: \$$(spend "$(cat "$O/setup.iso")")"
  } 2>&1 | log ;;
ops)
  # graft-ops from merged main, calm-one in: test-faults, the gate passed by override (b's own gate is not under
  # test), OPS_STEP_MIN_ENTRIES 50, a 60 s first day, a 1-minute cron, and STEP_DWELL <dwell ms>: 60000 for N's day,
  # 420000 for the measured one, so c is drained inside b's 10% step (row B: 286.7 s after the fault).
  D=${3:?dwell ms}
  { jq --argjson d "$D" '.gate = "pass" | .cron = "* * * * *" | .step_dwell_ms = $d | .first_day_ms = 60000 | .step_min_entries = 50' \
      "$W/ops/exec-ck-pending.json" >"$W/ops/exec-b07r2.json"
    jq -c '{scripts, gate, cron, step_dwell_ms, first_day_ms, step_min_entries, test_faults}' "$W/ops/exec-b07r2.json"
    (cd "$TREE" && export PATH=$HOME/.cargo/bin:$PATH && unset CARGO_TARGET_DIR && ln -sfn "${NODE_MODULES:?}" tests/m0/node_modules &&
      node workers/ops/deploy.mjs $OPS $STORE graft-m4a-mint-wrap-key - graft-m4a-secrets-wrap-key-b 1 \
        --executor "$W/ops/exec-b07r2.json" 2>&1 | grep -E "Current Version|rror" | tail -3)
    # A redeployed cron may not fire until it is removed and added again (R52): do that every time.
    api "/workers/scripts/$OPS/schedules" -X PUT -H 'content-type: application/json' --data '[]' >/dev/null; sleep 3
    api "/workers/scripts/$OPS/schedules" -X PUT -H 'content-type: application/json' --data '[{"cron":"* * * * *"}]' |
      jq -c '{ops_schedules: [.result.schedules[]?.cron]}'
    deployment $OPS
  } 2>&1 | log ;;
roll)
  # N on b first: a gated day passed by the override (GATE=pass) that makes N b's group record, so the measured day
  # has a real N. With no undo: b's current version is a site day's, not a release the group knows.
  RL=$O/roll.log; say() { echo "$(date -u +%FT%TZ) $*" | scrub | tee -a "$RL"; }
  R=$(rid "$K/n"); say "SiteDay{b, N ${R:0:8}} until site b reports it, stop \$$STOP"
  while true; do
    s=$(spend "$(cat "$O/setup.iso")"); over "$s" && { say "STOP at \$$s"; exit 1; }
    v=$(cur 0) || { sleep 30; continue; }
    say "spend \$$s; $v"
    if [ "$(echo "$v" | jq -c .current)" = null ]; then
      [ "$(echo "$v" | jq -r '.site_release["1"]')" = "${R:0:8}" ] && { say "site b reports N"; break; }
      # Both bundles uploaded first (graft-ops' next run after setup): N+1's day needs N's version too.
      up=$(echo "$v" | jq --arg n "${R:0:8}" --arg n1 "$(rid "$K/n1" | cut -c1-8)" '[.uploads | index($n), index($n1)] | all(. != null)')
      [ "$up" = true ] || { say "waiting for graft-ops to upload N and N+1"; sleep 60; continue; }
      a=$(day "$R"); say "SiteDay{b, N}: $a"; posted day-n "$a"
    fi
    sleep 60
  done ;;
run)
  # The measured day: SiteDay{b, N+1} with undo Rollback{b, N}. Supervised: amp orb service start b07r2-run --command
  # "bash tests/m5/b07r2/row.sh <tag> run > /tmp/b07r2-run.log 2>&1".
  RL=$O/run.log; say() { echo "$(date -u +%FT%TZ) $*" | scrub | tee -a "$RL"; }
  rm -f "$K"/stop-*
  N1=$(rid "$K/n1"); say "B0-07 r2: SiteDay{b, N+1 ${N1:0:8}}, stop \$$STOP from $(cat "$O/setup.iso")"
  say "clock: orb $(date -u +%s%3N), cloudflare $(cf_now), scratch $(ck /now | jq -c .)"
  # Loggers. Each writes JSON lines; every time in them is Cloudflare's.
  names_c=$(jq -c '.' "$O/voters-c.json")
  ( while [ ! -f "$K/stop-log" ]; do
      ck /status -X POST -d "{\"names\": $names_c, \"sites\": [0, 1, 2]}" >>"$O/status.jsonl" && echo >>"$O/status.jsonl"; sleep 30
    done ) &
  ( since=$(ck /now | jq .now); while [ ! -f "$K/stop-log" ]; do
      r=$(CK_MS=120 ck /elections -X POST -d "{\"names\": $names_c, \"since\": $((since - 60000))}")
      echo "$r" | jq -c . >>"$O/elections.jsonl" 2>/dev/null && since=$(echo "$r" | jq .at)
      sleep 20
    done ) &
  # Load: every c-homed chain written through the drain, so each c-led chain is active in the 50 s before it, plus
  # the a-homed chains, as row B's 1x. Each bench call is one 55 s invocation; its writes go to ops.jsonl.
  bench() {  # bench <side> <workers> <rate> [stop file]
    while [ ! -f "$K/stop-bench" ] && [ ! -f "$K/${4:-stop-bench}" ]; do
      ck /bench -X POST -m 90 -d "{\"chains\": $(jq -c ".$1" "$O/chains.json"), \"seconds\": 55, \"workers\": $2, \"rate\": $3}" |
        jq -c '.ops[]? | {chain: .[0], t: .[1], ms: .[2], ok: .[3]}' >>"$O/ops.jsonl"
    done
  }
  bench c 6 5 & bench c 6 5 stop-half & bench ab 3 4 &
  say "load on: 2 c-benches (6 x 5/s), 1 a-bench (3 x 4/s); loggers on"
  # The day.
  while true; do
    v=$(cur 0) && break; sleep 20
  done
  if [ "$(echo "$v" | jq -c .current)" = null ]; then
    a=$(day "$N1" "$(rid "$K/n")"); say "SiteDay{b, N+1} undo Rollback{b, N}: $a"; posted day-n1 "$a"
    view 0 | jq -r '[.entries[] | select(.kind == "Intent")][-1].entry' | sed 's/.* id //' >"$O/intent.id"
    say "measured intent $(cat "$O/intent.id")"
  fi
  fault=0; lifted=0; i=0
  while true; do
    v=$(cur 0) || { sleep 20; continue; }
    if [ $((i % 3)) -eq 0 ]; then
      s=$(spend "$(cat "$O/setup.iso")"); say "spend \$$s; $v"
      if over "$s"; then say "STOP at \$$s: load off, fault off"; touch "$K/stop-bench"; ck /fault -X POST -d '{"off": true}' >/dev/null; break; fi
    fi
    i=$((i + 1))
    cnext=$(echo "$v" | jq -r '.current.next // empty'); drained=$(echo "$v" | jq -c .drained)
    # The fault goes on in b's 10% step (next 2: the 1% step committed and 10% deployed), b drained, as row B.
    if [ $fault = 0 ] && [ "${cnext:-0}" = 2 ] && [ "$drained" = "[1]" ]; then
      f=$(ck /fault -X POST -d '{"site": 2, "rate": 0.3, "hang_ms": 3000, "for_ms": 3600000}')
      say "GRAY FAULT on c at cloudflare $(echo "$f" | jq .now): rate 0.3, hang_ms 3000; drained $drained"; fault=1
    fi
    # The lift: 60 s after c's drain, so the member path has moved every chain first.
    if [ $fault = 1 ] && [ $lifted = 0 ] && [ "$drained" = "[2]" ]; then
      [ -z "${cd_at:-}" ] && { cd_at=$(date +%s); say "c drained: $v"; }
      if [ $(( $(date +%s) - cd_at )) -ge 60 ]; then
        f=$(ck /fault -X POST -d '{"off": true}'); say "FAULT LIFTED at cloudflare $(echo "$f" | jq .now)"; lifted=1
        # Half the c load for the 15 min calm wait (UNDRAIN_AFTER), as row B: observer a's calls to c stay far over
        # MIN_CALLS while c's members follow.
        touch "$K/stop-half"; say "load cut to one c-bench for the undrain wait"
      fi
    fi
    if [ $lifted = 1 ] && [ "$drained" = "[]" ] && [ -z "${undrained:-}" ]; then undrained=1; say "c UNDRAINED by graft-ops: $v"; fi
    if [ $fault = 1 ] && [ "$(echo "$v" | jq -c .current)" = null ]; then say "IntentDone: $v"; break; fi
    sleep 20
  done
  touch "$K/stop-bench"; sleep 70; touch "$K/stop-log"; wait
  say "DONE: $(cur 0)" ;;
judge)
  # The group's entries and graft-ops' log lines since setup, then judge.py. relview must be built from this branch
  # (its SiteMode and IntentDone text name the site and the intent).
  { view 0 | jq -c '.entries[]' >"$O/entries.jsonl"
    from=$(date -u -d "$(cat "$O/setup.iso")" +%s000)
    KEEP="Committed,PAGE,graft-ops health,graft-ops calm,Wait" python3 tests/m5/bf46/logs.py $OPS "$from" "$(date -u +%s%3N)" \
      >"$O/ops-log.jsonl"
    echo "graft-ops lines: $(wc -l <"$O/ops-log.jsonl")"
    intent=$(cat "$O/intent.id")
    echo "measured intent $intent"
    python3 tests/m5/b07r2/judge.py "$O" "$intent" | tee "$O/verdict.jsonl"
  } 2>&1 | log ;;
teardown)
  { touch "$K/stop-bench" "$K/stop-log"; amp orb service stop b07r2-run >/dev/null 2>&1
    [ -s "$K/scratch.token" ] && ck /fault -X POST -d '{"off": true}' | jq -c '{fault_off: [.sites[]?.status]}'
    reset_cron $OPS
    jq -nc --arg v "$PARKED" '{strategy: "percentage", versions: [{version_id: $v, percentage: 100}], annotations: {"workers/message": "B0-07 r2 teardown: back to the parked version"}}' |
      api "/workers/scripts/$OPS/deployments" -X POST -H 'content-type: application/json' --data-binary @- | jq -c '{park: .success, errors: [.errors[]?.code]}'
    reset_cron $OPS; deployment $OPS
    id=$(api "/tokens?per_page=50" | jq -r --arg n $DEPLOY_TOKEN '.result[] | select(.name == $n) | .id')
    [ -n "$id" ] && api "/tokens/$id" -X DELETE | jq -c --arg n $DEPLOY_TOKEN '{revoke: $n, ok: .success}'
    "$NODE_MODULES/.bin/wrangler" delete --name $SCRATCH --force 2>&1 | tail -1
    for f in "$K/deploy.json" "$K/scratch.token"; do [ -f "$f" ] && shred -u "$f"; done; echo "row token files shredded"
    cur 0
    echo "spend since setup: \$$(spend "$(cat "$O/setup.iso")")"
  } 2>&1 | log ;;
lists)
  { api "/tokens?per_page=50" | jq -c '{b07r2_tokens: [.result[] | select(.name|test("b07r2")) | {name, status}]}'
    deployment $OPS; schedules $OPS
    echo "scratch Worker: $(api "/workers/scripts/$SCRATCH" -o /dev/null -w '%{http_code}')"
    cur 0
  } 2>&1 | log ;;
*) echo "phase: state build siteday bundles setup roll run judge teardown lists"; exit 2 ;;
esac
