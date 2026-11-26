#!/usr/bin/env bash
# BM-6's journey 5 drill on graft-m6a, prep day (`build/BUILD-PLAN.md`, M6-WP4; the combined drill plan, section 3,
# day 1): every new script version of the drill lands here, before the span. One phase per call, each logging to
# tests/m6/raw/m6a/bm6/<phase>.log (a phase run again logs to <phase>-2.log) through bf50.sh's `scrub`. Keys and
# tokens stay in 0600 files under ~/.graft-m6a and reach tools only as files, headers or a credential helper.
#
#   bash tests/m6/hosted/bm6.sh <phase> [args]
#
# Phases, in order:
#   start      pre-checks from a clean merged-main checkout: the spend (row, day, total), the Release view (nothing
#              current or drained), graft-ops' deploy token revoked and its cron on, 0 shard containers; the row starts
#   image      containers/git from this tree, pushed as graft-m6a-gitshard:<sha>; its digest-pinned name to bm6/image
#   day        the site day (every m6a site, spares included) and the edge day from this tree: the install's own flags
#              plus --export-bucket graft-m6a-export, and the edge's live FORMATS_ENABLED plus 17.5. This is what puts
#              restore-gate-from (265), warm-lapse (266) and cold-view-rows (267) on every site
#   bundles    R1 and R2 under ~/.graft-m6a/bm6: site/, workflows/, edge/ and ops/ from the builds `day` made (ops built
#              here), formats.txt the edge's live formats in `FormatSet` order; R2 = R1 plus a marker in each folder;
#              both signed by the release key
#   ops        graft-ops' deploy token minted into graft-m6a-ops-deploy-token; graft-m6a-releases created; graft-m6a-ops
#              from this tree with the release bucket, GATE=pass and 60 s dwells; R1 put under bm6-r1/
#   roll <r1|r2>  `release roll` after each IntentDone until done, stopping at any spend stop or a step held 30 min
#   put r2     R2 put under bm6-r2/, once R1's ReleaseUploaded committed (so R2 takes the next seq)
#   tags       every script's deployed version and its workers/tag, and the view's site_release
#   enable     `FormatsEnable 17.5`, signed by the customer key and the release key, once every site reports R2
#   revoke     graft-ops' deploy token deleted; it answers 401 (it answered 200 just before)
#   seed       the owner and PAT through the bench's test-only /op (no product route, R14), `bm6` through
#              POST /api/v3/user/repos, real git pushes (two 3 MiB stored, one inline), branch f, PR #1 by gh
#   init       `lifeboat init` (LifeboatAdd) into a fresh 0700 folder, for the Lifeboat thread
#   xfer <seed|boat|clean>  copies the Lifeboat thread's files to .amp/in/xfer at 0600 for upload, or removes them
#   containers every graft-m6a shard application's active count (warm-lapse: 0 about 10 min after a warm)
#   pushes <seconds> [every_s]  real pushes to bm6's main, each answer and tip to pushes.jsonl (the span's load)
#
# Stops, read on acctcost before each phase and in the roll loop: the row $10 (from `start`), the UTC day $20, the
# install's total $70.
set -uo pipefail
PH=${1:?phase}
G=${G:-$HOME/.cache/cli-target/release/graftyard}
# shellcheck source=/dev/null
source "$(dirname "${BASH_SOURCE[0]}")/bf50.sh"
cd "$ROOT"
O=$ROOT/tests/m6/raw/m6a/bm6; mkdir -p "$O"
B=$W/bm6; mkdir -p "$B"; chmod 700 "$B"
EXPORT=graft-m6a-export; RELEASES=graft-m6a-releases; DEPLOY_NAME=graft-m6a-bm6-deploy
SITES="graft-m6a-site-a graft-m6a-site-b graft-m6a-site-c graft-m6a-spare-wnam"
N=$(ls "$O" | grep -c "^$PH\(-[0-9]*\)\?\.log$" || true)
L="$O/$PH$( [ "$N" -gt 0 ] && echo "-$((N + 1))" ).log"
logto() { { date -u +%FT%TZ; git rev-parse --short HEAD; cat; date -u +%FT%TZ; } | scrub >"$L"; cat "$L"; }
spent() { PREFIX=graft-m6a python3 "$ROOT/tests/fanout/acctcost.py" "$1" "$(date -u +%FT%TZ)" 2>/dev/null | head -1 | jq -r .prefix_usd; }
# stops: prints the row, day and total spend; fails once any reaches its stop.
stops() {
  local row day tot
  row=$(spent "$(cat "$O/start.iso")"); day=$(spent "$(date -u +%F)T00:00:00Z")
  tot=$(spent "$(date -u -d @"$(cat "$W/t0")" +%FT%TZ)")
  echo "spend: row \$$row, day \$$day, total \$$tot (stops \$10, \$20, \$70)"
  python3 -c "import sys; r, d, t = map(float, sys.argv[1:]); sys.exit(1 if r >= 10 or d >= 20 or t >= 70 else 0)" \
    "${row:-0}" "${day:-0}" "${tot:-0}"
}
live_formats() { api /workers/scripts/graft-m6a-edge/settings | jq -r '.result.bindings[] | select(.name == "FORMATS_ENABLED") | .text'; }
rid() { sha256sum "$1/release.json" | cut -c1-64; }
deployment() { api "/workers/scripts/$1/deployments" | jq -c --arg s "$1" '.result.deployments[0] | {script: $s, created_on, versions: [.versions[] | "\(.version_id[0:8]) \(.percentage)"]}'; }
tag_of() {
  local d; d=$(api "/workers/scripts/$1/deployments" | jq -c '.result.deployments[0].versions')
  for v in $(echo "$d" | jq -r '.[] | "\(.version_id)/\(.percentage)"'); do
    api "/workers/scripts/$1/versions/${v%/*}" | jq -c --arg s "$1" --arg p "${v#*/}" \
      '(.result.annotations // {}) as $a | {script: $s, version: .result.id[0:8], percentage: ($p|tonumber), tag: (($a["workers/tag"] // "") | .[0:14])}'
  done
}
bindings() { api "/workers/scripts/$1/settings" | jq -c --arg s "$1" '{script: $s, bindings: [.result.bindings[] | "\(.name):\(.type)"] | sort}'; }
shards() { api /containers/applications | jq -c '[.result[] | select(.name | startswith("graft-m6a")) | {(.name): .health.instances.active}] | add'; }
summary() { view 0 | jq -c '{count, current, drained, site_release, uploads, enabled}'; }
gitc() { git -c credential.helper= -c "credential.helper=!f() { echo username=x; echo password=\$(cat $1); }; f" \
  -c user.name=bm6 -c user.email=bm6@graftyard.test "${@:2}"; }
PAT=$K/bm6.probe.pat
seed() { jq -r ".$1" "$O/probe-seed.json"; }
[ "$PH" = start ] || [ -s "$O/start.iso" ] || { echo "run start first"; exit 2; }

case $PH in
start)
  { [ "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)" ] && [ -z "$(git status --short -- workers crates containers cli tests/m6/hosted)" ] \
      && echo "checkout: origin/main $(git rev-parse --short HEAD), clean" || { echo "STOP: not a clean origin/main checkout"; exit 1; }
    [ -s "$O/start.iso" ] || date -u +%FT%TZ >"$O/start.iso"
    # The CLI and relview from this tree: `release entry` needs FormatsEnable, and relview must decode every entry kind.
    CARGO_TARGET_DIR=${G%/release/graftyard} cargo build -q --release -p graftyard || { echo "STOP: the CLI build failed"; exit 1; }
    (cd tests/m3/hosted/tools && cargo build -q --release --bin relview) || { echo "STOP: the relview build failed"; exit 1; }
    echo "row start $(cat "$O/start.iso"); CLI $G, relview $RELVIEW, both from $(git rev-parse --short HEAD)"
    stops || { echo "STOP: a spend stop is reached"; exit 1; }
    summary
    echo "graft-ops cron $(api "/workers/scripts/$OPS/schedules" | jq -c '[.result.schedules[]?.cron]')"
    api "/tokens?per_page=50" | jq -c --arg n "$DEPLOY_NAME" '{bm6_deploy_tokens: [.result[] | select(.name == $n) | .id[0:8]]}'
    echo "shards $(shards)"
    for s in $SITES graft-m6a-edge graft-m6a-workflows $OPS; do deployment "$s"; done
  } 2>&1 | logto ;;
image)
  { stops || exit 1
    T=$(git rev-parse --short HEAD)
    docker build -q -t "graft-m6a-gitshard:$T" containers/git && \
      npx --prefix tests/m0 wrangler containers push "graft-m6a-gitshard:$T" 2>&1 | grep -E "sha256|rror|Pushed|digest" | tail -3
    img=$(docker inspect --format '{{join .RepoDigests "\n"}}' "graft-m6a-gitshard:$T" | grep "registry.cloudflare.com/$CLOUDFLARE_ACCOUNT_ID/graft-m6a-gitshard@" | head -1)
    [ -n "$img" ] || { echo "STOP: no digest-pinned name in the registry"; exit 1; }
    echo "$img" >"$B/image"; echo "image ${img##*/} from $T"
  } 2>&1 | logto ;;
day)
  { stops || exit 1
    IMG=$(cat "$B/image"); git rev-parse HEAD >"$O/tree"
    for s in $SITES graft-m6a-edge; do bindings "$s" | jq -c '{script, n: (.bindings | length)}'; done
    F=$(live_formats); echo "live FORMATS_ENABLED: $F"
    case ",$F," in *,17.5,*) ;; *) F=$F,17.5 ;; esac
    echo "image ${IMG##*/}"
    node workers/site/deploy.mjs "$W/sites.json" --test-faults --anchor "$W/install.toml" --image "$IMG" \
      --vars "$W/site-vars.json" --ssh-off --export-bucket "$EXPORT" 2>&1 | grep -E "Current Version|Deployed|rror|exit" | tail -12
    echo "site exit ${PIPESTATUS[0]}"; date -u +%FT%TZ
    node tests/m1/integ/deploy-edge.mjs "$W/sites.json" graft-m6a-edge --test-faults --anchor "$W/install.toml" \
      --ssh-off --var "FORMATS_ENABLED=$F" --export-bucket "$EXPORT" 2>&1 | grep -E "Current Version|Deployed|rror" | tail -4
    echo "edge exit ${PIPESTATUS[0]}"; date -u +%FT%TZ
    echo "now FORMATS_ENABLED: $(live_formats)"
    for s in $SITES graft-m6a-edge; do
      echo "$(deployment "$s") crons $(api "/workers/scripts/$s/schedules" | jq -c '[.result.schedules[]?.cron]')"
      bindings "$s" | jq -c '{script, n: (.bindings | length), export: ([.bindings[] | select(startswith("EXPORT") or startswith("GRAFTYARD_EXPORT"))])}'
    done
    api "/r2/buckets?per_page=100" | jq -c --arg b "$EXPORT" '{export_bucket: ([.result.buckets[].name] | index($b) != null)}'
  } 2>&1 | logto ;;
bundles)
  { [ "$(git rev-parse HEAD)" = "$(cat "$O/tree")" ] || { echo "STOP: HEAD is not the tree \`day\` deployed"; exit 1; }
    (cd workers/ops && CARGO_TARGET_DIR=$HOME/.cache/m6a-ops-target worker-build --release --out-dir build --features test-faults >/dev/null 2>&1) \
      || { echo "STOP: the ops build failed"; exit 1; }
    # `FormatSet` order (family, then version), which `FormatSet::file` writes and the group hashes.
    live_formats | tr , '\n' | sort -t. -k1,1n -k2,2n -u >"$B/formats.txt"
    grep -qx 17.5 "$B/formats.txt" || { echo "STOP: the edge does not list 17.5; run day first"; exit 1; }
    for r in r1 r2; do
      d=$B/$r; rm -rf "$d"; mkdir -p "$d"/{site,workflows,edge,ops}; cp "$B/formats.txt" "$d/"
      cp workers/site/build/index.js workers/site/build/index_bg.wasm "$d/site/"
      cp workers/edge/build/index.js workers/edge/build/index_bg.wasm "$d/edge/"
      cp workers/ops/build/index.js workers/ops/build/index_bg.wasm "$d/ops/"
      # graft-ops uploads a folder's `entry.js` as its main module; entry.mjs imports ./move.mjs.
      cp workers/workflows/entry.mjs "$d/workflows/entry.js"; cp workers/workflows/move.mjs "$d/workflows/"
      [ $r = r2 ] && for w in site workflows edge ops; do echo "export const BM6 = \"bm6-r2 $w\";" >"$d/$w/marker.js"; done
      python3 - "$d" "bm6-$r $(cat "$O/tree")" <<'PY'
import hashlib, json, os, sys
d, label = sys.argv[1], sys.argv[2]
files = []
for root, _, names in os.walk(d):
    for n in names:
        p = os.path.join(root, n); rel = os.path.relpath(p, d)
        if rel not in ("release.json", "release.sig"):
            files.append({"name": rel, "sha256": hashlib.sha256(open(p, "rb").read()).hexdigest()})
open(f"{d}/release.json", "wb").write(json.dumps({"files": sorted(files, key=lambda f: f["name"]), "label": label}, sort_keys=True).encode())
PY
      openssl pkeyutl -sign -rawin -inkey "$W/release.key" -in "$d/release.json" -out "$d/release.sig"
      openssl pkeyutl -verify -rawin -pubin -inkey "$W/release.pub" -in "$d/release.json" -sigfile "$d/release.sig" >/dev/null \
        && echo "$r $(rid "$d") signed and verified: $(jq -c '[.files[].name]' "$d/release.json")"
    done
    echo "formats.txt: $(wc -l <"$B/formats.txt") formats, $(tr '\n' ' ' <"$B/formats.txt")"
    for w in site edge ops; do echo "$w wasm sha256 $(sha256sum "$B/r1/$w/index_bg.wasm" | cut -c1-16)"; done
  } 2>&1 | logto ;;
ops)
  { stops || exit 1
    deployment $OPS
    [ -s "$K/$DEPLOY_NAME.json" ] || mint $DEPLOY_NAME $SCRIPTS_WRITE $CONTAINERS_WRITE
    ( umask 077; jq -c '{value: .result.value}' "$K/$DEPLOY_NAME.json" >"$K/deploy-token.value" )
    put_secret graft-m6a-ops-deploy-token "$K/deploy-token.value"; shred -u "$K/deploy-token.value"
    api "/r2/buckets?per_page=100" | jq -e --arg b $RELEASES '[.result.buckets[].name] | index($b)' >/dev/null \
      || "$WRANGLER" r2 bucket create $RELEASES 2>&1 | tail -1
    jq --arg r $RELEASES '.releases = $r | .gate = "pass" | .step_dwell_ms = 60000 | .first_day_ms = 60000' \
      "$W/ops/exec.json" >"$W/ops/exec-bm6.json"
    jq -c '{scripts, releases, gate, cron, step_dwell_ms, first_day_ms, test_faults, recover_ms}' "$W/ops/exec-bm6.json"
    CARGO_TARGET_DIR=$HOME/.cache/m6a-ops-target node workers/ops/deploy.mjs $OPS "$STORE" graft-m6a-mint-wrap-key \
      --executor "$W/ops/exec-bm6.json" 2>&1 | grep -E "Current Version|rror" | tail -3
    deployment $OPS; echo "graft-ops cron $(api "/workers/scripts/$OPS/schedules" | jq -c '[.result.schedules[]?.cron]')"
    (cd "$B/r1" && find . -type f | sed 's#^\./##' | sort | while read -r f; do
      "$WRANGLER" r2 object put "$RELEASES/bm6-r1/$f" --file "$f" --remote >/dev/null 2>&1 && echo "put bm6-r1/$f" || echo "PUT FAILED bm6-r1/$f"
    done)
    echo "R1 $(rid "$B/r1")"
  } 2>&1 | logto ;;
put)
  { [ "${2:-}" = r2 ] || { echo "put r2"; exit 2; }
    summary | jq -c --arg r "$(rid "$B/r1" | cut -c1-8)" '{count, current, uploads, r1_uploaded: (.uploads | index($r) != null)}'
    (cd "$B/r2" && find . -type f | sed 's#^\./##' | sort | while read -r f; do
      "$WRANGLER" r2 object put "$RELEASES/bm6-r2/$f" --file "$f" --remote >/dev/null 2>&1 && echo "put bm6-r2/$f" || echo "PUT FAILED bm6-r2/$f"
    done)
    echo "R2 $(rid "$B/r2")"
  } 2>&1 | logto ;;
roll)
  r=${2:?r1 or r2}; R=$(rid "$B/$r"); RL=$O/roll-$r.log
  say() { echo "$(date -u +%FT%TZ) $*" | scrub | tee -a "$RL"; }
  say "release roll $r $R"
  i=0; held=""; since=$(date +%s)
  while true; do
    v=$(summary 2>&1) || { say "view: $v"; sleep 30; continue; }
    if [ $((i % 5)) -eq 0 ]; then
      s=$(stops); ok=$?; say "$s; $v"
      [ $ok -eq 0 ] || { say "STOP: a spend stop; graft-ops' cron off"; cronoff | scrub | tee -a "$RL"; exit 1; }
    fi
    i=$((i + 1))
    cur=$(echo "$v" | jq -c .current)
    if [ "$cur" = null ]; then
      held=""
      pages
      a=$("$G" --config "$W/install.toml" release roll "$R" --entries "$K/pages.json" --key "$W/customer.key" --edge "$EDGE" 2>&1)
      say "roll: $(echo "$a" | tr '\n' ' ')"
      echo "$a" | grep -q '"roll": *"done"' && { say "$(summary)"; say "DONE"; exit 0; }
    elif [ "$cur" != "$held" ]; then held=$cur; since=$(date +%s)
    # 30 min: a new graft-ops cron can take 15 min to fire (OPEN-QUESTIONS R52), and every step changes `current`.
    elif [ $(($(date +%s) - since)) -gt 1800 ]; then say "STALL: $cur held 30 min; stop and analyse"; exit 2
    fi
    sleep 60
  done ;;
tags)
  { for s in $SITES graft-m6a-workflows graft-m6a-edge $OPS; do tag_of "$s"; done
    summary; stops
  } 2>&1 | logto ;;
enable)
  { stops || exit 1
    v=$(summary); echo "$v"
    want=$(rid "$B/r2" | cut -c1-8)
    echo "$v" | jq -e --arg r "$want" '[.site_release["0","1","2","3"]] | all(. == $r)' >/dev/null \
      || { echo "STOP: not every site reports R2 $want"; exit 1; }
    customer "{\"FormatsEnable\": {\"format\": \"17.5\", \"release_key\": \"$W/release.key\"}}" formats-enable-17.5
    summary | jq -c '{count, enabled}'
  } 2>&1 | logto ;;
revoke)
  { f=$K/$DEPLOY_NAME.json
    code() { curl -s -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $(jq -r .result.value "$f")" "$CFAPI/tokens/verify"; }
    echo "deploy token before: $(code) (want 200)"
    revoke $DEPLOY_NAME; sleep 5
    c=$(code); echo "deploy token after: $c (want 401)"
    shred -u "$f"; echo "token file shredded"
    api "/tokens?per_page=50" | jq -c --arg n "$DEPLOY_NAME" '{bm6_deploy_tokens: [.result[] | select(.name == $n) | .id[0:8]]}'
    [ "$c" = 401 ]
  } 2>&1 | logto ;;
seed)
  { stops || exit 1
    [ -s "$O/probe-seed.json" ] || BENCH=$BENCH EDGE=$EDGE REPO=bm6 node tests/m6/hosted/bf50-probe.mjs seed "$O" | jq -c 'del(.at)'
    [ -s "$O/probe-seed.json" ] || { echo "STOP: the seed made no repository"; exit 1; }
    echo "owner $(seed login), repository $(seed full_name), chains $(jq -c .chains "$O/probe-seed.json")"
    wt=$(mktemp -d); trap 'rm -rf "$wt"' EXIT
    url="$EDGE/$(seed full_name).git"
    gitc "$PAT" clone -q "$url" "$wt/bm6" || { echo "STOP: clone failed"; exit 1; }
    push() {  # push <file> <bytes> <ref>
      head -c "$2" /dev/urandom >"$wt/bm6/$1"; git -C "$wt/bm6" add "$1"
      gitc "$PAT" -C "$wt/bm6" commit -q -m "bm6 $1"
      local t0; t0=$(date +%s%3N)
      if gitc "$PAT" -C "$wt/bm6" push -q origin "HEAD:$3" 2>"$wt/err"; then ok=true; else ok=false; fi
      jq -nc --arg f "$1" --argjson b "$2" --arg r "$3" --argjson ok $ok --argjson ms $(($(date +%s%3N) - t0)) \
        --arg tip "$(git -C "$wt/bm6" rev-parse HEAD)" --arg err "$(head -c 200 "$wt/err")" \
        '{at: (now | todate), file: $f, bytes: $b, ref: $r, ok: $ok, ms: $ms, tip: $tip, err: $err}' | tee -a "$O/seed-pushes.jsonl"
      $ok
    }
    push big-0.bin $((3 << 20)) refs/heads/main && push big-1.bin $((3 << 20)) refs/heads/main \
      && push small-2.txt 2000 refs/heads/main && git -C "$wt/bm6" checkout -q -b f && push pr.txt 500 refs/heads/f \
      || { echo "STOP: a seed push failed"; exit 1; }
    host=${EDGE#https://}; ghd=$(mktemp -d)
    pr() { GH_CONFIG_DIR=$ghd GH_HOST=$host GH_ENTERPRISE_TOKEN=$(cat "$PAT") GH_PROMPT_DISABLED=1 GH_NO_UPDATE_NOTIFIER=1 gh "$@"; }
    (cd "$wt/bm6" && pr pr create -R "$host/$(seed full_name)" --base main --head f --title "bm6 lifeboat pr" \
      --body "read me from the lifeboat") 2>&1 | sed "s#https://$host#<edge>#g" | tail -2
    rc=${PIPESTATUS[0]}; echo "gh pr create exit $rc"
    if [ "$rc" != 0 ]; then
      # What `gh pr create` sends, as bf53-seed.mjs does it, when gh itself does not get there.
      node_id=$(curl -s -H "authorization: token $(cat "$PAT")" "$EDGE/api/v3/repos/$(seed full_name)" | jq -r .node_id)
      jq -nc --arg r "$node_id" '{query: "mutation($i: CreatePullRequestInput!) { createPullRequest(input: $i) { pullRequest { number title } } }",
        variables: {i: {repositoryId: $r, baseRefName: "main", headRefName: "f", title: "bm6 lifeboat pr", body: "read me from the lifeboat"}}}' |
        curl -s -X POST -H "authorization: token $(cat "$PAT")" -H 'content-type: application/json' --data-binary @- "$EDGE/api/graphql" |
        jq -c '{createPullRequest: (.data.createPullRequest // .)}'
    fi
    pr pr list -R "$host/$(seed full_name)" 2>&1 | tail -3; rm -rf "$ghd"
    gitc "$PAT" ls-remote --heads "$url" | awk '{print $1, $2}' | tee "$O/refs-seed.txt"
  } 2>&1 | logto ;;
init)
  { stops || exit 1
    d=$B/boat-$(date -u +%H%M%S); mkdir -m 700 "$d"
    "$G" --config "$W/install.toml" lifeboat init --host lifeboat.bm6.test --key "$W/customer.key" --dir "$d" \
      --edge "$EDGE" --genesis "$W/genesis.signed"
    echo "init exit $?"; echo "$d" >"$B/boat"; ls -l "$d" | awk 'NR > 1 {print $1, $NF}'
    view 0 | jq -c '{count, last: .entries[-1].entry}'
  } 2>&1 | logto ;;
xfer)
  X=$ROOT/.amp/in/xfer
  case ${2:?seed, boat or clean} in
    seed) mkdir -p "$X"; install -m 600 "$PAT" "$X/pat"; seed login >"$X/owner"; chmod 600 "$X/owner"; ls -l "$X" | awk 'NR > 1 {print $1, $NF}' ;;
    boat) mkdir -p "$X"; d=$(cat "$B/boat"); install -m 600 "$d/lifeboat.json" "$d/lifeboat.key" "$X/"; ls -l "$X" | awk 'NR > 1 {print $1, $NF}' ;;
    clean) rm -rf "$X"; [ -s "$B/boat" ] && rm -rf "$(cat "$B/boat")" && rm "$B/boat"; echo "xfer and boat copies removed" ;;
  esac ;;
containers)
  { echo "shards $(shards)"; } 2>&1 | logto ;;
pushes)
  S=${2:?seconds}; E=${3:-20}; wt=$(mktemp -d); trap 'rm -rf "$wt"' EXIT
  url="$EDGE/$(seed full_name).git"; end=$(($(date +%s) + S)); n=0
  gitc "$PAT" clone -q "$url" "$wt/bm6" || { echo "clone failed"; exit 1; }
  while [ "$(date +%s)" -lt "$end" ]; do
    n=$((n + 1)); f="span-$(date -u +%H%M%S)-$n.bin"; bytes=$(( n % 3 == 0 ? 3 << 20 : 4000 ))
    head -c $bytes /dev/urandom >"$wt/bm6/$f"; git -C "$wt/bm6" add "$f"; gitc "$PAT" -C "$wt/bm6" commit -q -m "bm6 $f"
    t0=$(date +%s%3N)
    if gitc "$PAT" -C "$wt/bm6" push -q origin HEAD:refs/heads/main 2>"$wt/err"; then ok=true; else ok=false
      gitc "$PAT" -C "$wt/bm6" fetch -q origin main 2>/dev/null && git -C "$wt/bm6" reset -q --hard origin/main; fi
    jq -nc --argjson n $n --argjson b $bytes --argjson ok $ok --argjson ms $(($(date +%s%3N) - t0)) \
      --arg tip "$(git -C "$wt/bm6" rev-parse HEAD)" --arg err "$(head -c 200 "$wt/err")" \
      '{at: (now | todate), n: $n, bytes: $b, ok: $ok, ms: $ms, tip: $tip, err: $err}' | scrub | tee -a "$O/pushes.jsonl"
    sleep "$E"
  done ;;
*) echo "phase: start image day bundles ops roll put tags enable revoke seed init xfer containers pushes"; exit 2 ;;
esac
# A logged phase exits with its body's status, not the log's.
exit "${PIPESTATUS[0]}"
