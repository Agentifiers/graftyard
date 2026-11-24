#!/usr/bin/env bash
# BM-5 clause 1 hosted on graft-m4a (build/BUILD-PLAN.md, M5-WP7): "A deliberately unreadable format released to
# `graft-site-a` commits nothing and moves A's groups." Stop $6 (the lead's ruling, 2026-11-24).
#
# graft-ops from this tree uploads two releases under seq tags (`decisions/DECISIONS.md`, "`release-seq`"): N, the
# site built test-faults, on all three sites; then N+1, the site built `test-format` (it also reads 0x20.0xfff0,
# `REPO_FORMAT_TEST`), on site a only. A test-faults edge with FORCE_FORMAT=32.65520 and FORCE_FORMAT_SITE=0 then
# sends a git push in that format only when the repo's leader is on site a. Pass: no entry carrying a forced push's ref
# commits on any member, and the groups a led move to b or c.
#
#   bash tests/m5/bm5/format.sh <tag> <phase> [args]
#
# Phases, in order:
#   bundles     N and N+1 under ~/.graft-m4a/bm5f: R9's formats.txt (N+1 adds 32.65520) and site/ from $OUT, signed
#   setup       graft-ops' deploy token minted and PATCHed in; graft-m4a-ops from $DEPLOY (test-faults, GATE=pass,
#               60 s dwells, 1-minute cron, three sites); N put under bm5f-n/
#   put n1      N+1 put under bm5f-n1/, once N's ReleaseUploaded committed (so N+1 takes the next seq)
#   roll <R> [site]  supervised: `release roll R` after each IntentDone until done, or until <site>'s SiteRelease is R
#   tags        every site's deployed version and its workers/tag
#   edge        a version of graft-m4a-edge from $OUT/edge with every binding kept plus FORCE_FORMAT=32.65520 and
#               FORCE_FORMAT_SITE=0, uploaded only
#   edgego      that version deployed at 100%
#   repos       fresh repos bm5f-<tag>-1..4, with each one's home and leader
#   push        per repo led by site a: pushes to unique refs bm5f/<tag>/<repo>/<i>, each answer's status, the
#               leader after each, the members' received counts by format
#   checks      every member's /log for the forced refs (committed or not), every member's /status, site a's alarms
#   edgeback    graft-m4a-edge back to eef95142 at 100% and f4e193b9 at 0%, by version id
#   park        graft-ops' cron off, graft-ops back to ac4cd9a5 by version id, its deploy token revoked and shredded
#
# Needs the graft-m4a env (tests/am1/lf13/env-m4a.sh), OUT (site-n/, site-n1/, edge/ worker-build outputs, TREE),
# DEPLOY (the worktree OUT was built from), G (graftyard), RELVIEW, NODE_MODULES. Every log goes through scrub; token
# values stay in 0600 files.
set -uo pipefail
cd "$(dirname "$0")/../../.."; ROOT=$PWD
TAG=${1:?tag}; PH=${2:?phase}
O=tests/m5/results/raw/bm5/$TAG; mkdir -p "$O"
W=$HOME/.graft-m4a; K=$W/bm5f; mkdir -p "$K"; chmod 700 "$K"
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
REPOS=$K/repos-$TAG.txt

case $PH in
bundles)
  { for r in n n1; do
      d=$K/$r; rm -rf "$d"; mkdir -p "$d/site"
      cp "$W/bf47/r9/formats.txt" "$d/"
      # formats.txt must be canonical (`FormatSet::file`: sorted by family, then version), so 32.65520 goes after 32.24.
      [ $r = n1 ] && { printf '32.65520\n' >>"$d/formats.txt"; sort -t. -k1,1n -k2,2n -o "$d/formats.txt" "$d/formats.txt"; }
      cp "$OUT/site-$r/index.js" "$OUT/site-$r/index_bg.wasm" "$d/site/"
      python3 - "$d" "bm5f-$r" <<'PY'
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
        && echo "$r $(rid "$d") signed and verified; reads $(wc -l <"$d/formats.txt") formats, last $(tail -1 "$d/formats.txt")"
      echo "  site wasm $(sha256sum "$d/site/index_bg.wasm" | cut -c1-16)"
    done
    echo "builds of $(cat "$OUT/TREE")"
  } 2>&1 | log ;;
setup)
  { date -u +%FT%TZ >"$O/setup.iso"; date -u +%s%3N >"$O/start.ms"
    echo "builds of $(cat "$OUT/TREE")"
    echo "graft-m4a's day to now: \$$(PREFIX=graft-m4a python3 tests/fanout/acctcost.py | head -1 | jq -r .prefix_usd)"
    view 0 | jq -c '{count, current, drained, site_release, uploads: (.uploads | length)}'
    deployment $OPS
    [ -s "$K/deploy-token.json" ] || mint "$K/deploy-token.json" graft-m4a-bm5f-deploy $SCRIPTS_WRITE $CONTAINERS_WRITE
    id=$(api "/secrets_store/stores/$STORE/secrets?per_page=100" | jq -r '.result[] | select(.name == "graft-m4a-ops-deploy-token") | .id')
    ( umask 077; jq -c '{value: .result.value}' "$K/deploy-token.json" >"$K/deploy-token.value" )
    api "/secrets_store/stores/$STORE/secrets/$id" -X PATCH -H 'content-type: application/json' --data-binary @"$K/deploy-token.value" |
      jq -c '{patch: "graft-m4a-ops-deploy-token", ok: .success, errors: [.errors[]?.code]}'
    shred -u "$K/deploy-token.value"
    jq '.gate = "pass" | .cron = "* * * * *" | .step_dwell_ms = 60000 | .first_day_ms = 60000' \
      "$W/ops/exec-ck-pending.json" >"$W/ops/exec-bm5f.json"
    jq -c '{scripts, gate, cron, step_dwell_ms, first_day_ms, step_min_entries, test_faults}' "$W/ops/exec-bm5f.json"
    (cd "$DEPLOY" && export PATH=$HOME/.cargo/bin:$PATH \
      && ln -sfn "${NODE_MODULES:?}" tests/m0/node_modules \
      && node workers/ops/deploy.mjs $OPS $STORE graft-m4a-mint-wrap-key - graft-m4a-secrets-wrap-key-b 1 \
           --executor "$W/ops/exec-bm5f.json" 2>&1 | grep -E "Current Version|rror" | tail -3)
    deployment $OPS
    api "/workers/scripts/$OPS/schedules" | jq -c '{ops_schedules: [.result.schedules[]?.cron]}'
    put "$K/n" bm5f-n; echo "N $(rid "$K/n")"
  } 2>&1 | log ;;
put)
  { v=$(view 0); echo "$v" | jq -c --arg n "$(rid "$K/n" | cut -c1-8)" '{count, uploads: (.uploads | length), n_uploaded: (.uploads | index($n) != null)}'
    put "$K/n1" bm5f-n1; echo "N+1 $(rid "$K/n1")"
  } 2>&1 | log ;;
roll)
  R=$(rid "$K/${3:?n or n1}"); UNTIL=${4:-}; RL=$O/roll-$3.log; STOP=${STOP_USD:-6}
  say() { echo "$(date -u +%FT%TZ) $*" | scrub | tee -a "$RL"; }
  say "release roll $3 $R${UNTIL:+ until site $UNTIL reports it}, stop \$$STOP from $(cat "$O/setup.iso")"
  i=0
  while true; do
    v=$(view 0 2>&1 | jq -c '{count, current, drained, site_release, uploads: (.uploads | length)}') || { say "view: $v"; sleep 30; continue; }
    if [ $((i % 5)) -eq 0 ]; then
      s=$(spend); say "spend \$$s; $v"
      if python3 -c "import sys; sys.exit(0 if float('${s:-0}') >= $STOP else 1)"; then say "STOP at \$$s"; exit 1; fi
    fi
    i=$((i + 1))
    if [ "$(echo "$v" | jq -c .current)" = null ]; then
      if [ -n "$UNTIL" ] && [ "$(echo "$v" | jq -r ".site_release[\"$UNTIL\"]")" = "${R:0:8}" ]; then say "site $UNTIL reports $3: $v"; say "DONE"; exit 0; fi
      a=$(roll "$R"); say "roll: $(echo "$a" | tr '\n' ' ')"
      echo "$a" | grep -q '"roll": *"done"' && { say "DONE"; exit 0; }
    fi
    sleep 60
  done ;;
tags)
  { for x in a b c; do tag_of graft-m4a-site-$x; done; tag_of $EDGE; deployment $OPS
    view 0 | jq -c '{count, current, drained, site_release}'
    echo "spend since setup: \$$(spend)"
  } 2>&1 | log ;;
edge)
  { live=$(api "/workers/scripts/$EDGE/deployments" | jq -r '.result.deployments[0].versions | max_by(.percentage) | .version_id')
    echo "edge live ${live:0:8}"; tag_of $EDGE
    node - "$OUT/edge" "$EDGE" "$live" >"$K/edge-upload.json" <<'JS'
import { readFileSync } from "node:fs";
const [dir, name, live] = process.argv.slice(2);
const B = `https://api.cloudflare.com/client/v4/accounts/${process.env.CLOUDFLARE_ACCOUNT_ID}`;
const H = { authorization: `Bearer ${process.env.CLOUDFLARE_API_TOKEN}` };
const one = await (await fetch(`${B}/workers/scripts/${name}/versions/${live}`, { headers: H })).json();
const rt = one.result.resources.script_runtime;
const meta = {
  main_module: "index.js",
  keep_bindings: ["plain_text", "secret_text", "json", "durable_object_namespace", "service", "r2_bucket", "queue",
    "secrets_store_secret", "pipelines", "version_metadata", "assets", "workflow"],
  keep_assets: true,
  bindings: [{ type: "plain_text", name: "FORCE_FORMAT", text: "32.65520" }, { type: "plain_text", name: "FORCE_FORMAT_SITE", text: "0" }],
  annotations: { "workers/tag": "bm5f-edge", "workers/message": "BM-5 clause 1: FORCE_FORMAT aimed at site a" },
  compatibility_date: rt.compatibility_date, ...(rt.compatibility_flags ? { compatibility_flags: rt.compatibility_flags } : {}),
  ...(rt.limits ? { limits: rt.limits } : {}),
};
const form = new FormData();
form.append("metadata", JSON.stringify(meta));
form.append("index.js", new Blob([readFileSync(`${dir}/index.js`)], { type: "application/javascript+module" }), "index.js");
form.append("index_bg.wasm", new Blob([readFileSync(`${dir}/index_bg.wasm`)], { type: "application/wasm" }), "index_bg.wasm");
const r = await (await fetch(`${B}/workers/scripts/${name}/versions`, { method: "POST", headers: H, body: form })).json();
console.log(JSON.stringify({ ok: r.success, id: r.result?.id, errors: r.errors }));
JS
    cat "$K/edge-upload.json"; nv=$(jq -r .id "$K/edge-upload.json")
    api "/workers/scripts/$EDGE/versions/$nv" | jq -c '{vars: [.result.resources.bindings[] | select(.type=="plain_text") | .name] | sort, types: ([.result.resources.bindings[].type] | group_by(.) | map({(.[0]): length}) | add)}'
    echo "$nv" >"$K/edge-version"
  } 2>&1 | log ;;
edgego)
  { nv=$(cat "$K/edge-version")
    jq -nc --arg v "$nv" '{strategy: "percentage", versions: [{version_id: $v, percentage: 100}], annotations: {"workers/message": "BM-5 clause 1: FORCE_FORMAT edge"}}' |
      api "/workers/scripts/$EDGE/deployments" -X POST -H 'content-type: application/json' --data-binary @- | jq -c '{deploy: .success, errors: [.errors[]?.code]}'
    deployment $EDGE; tag_of $EDGE
  } 2>&1 | log ;;
repos)
  { names=$(for i in 1 2 3 4; do echo "bm5f-$TAG-$i"; done)
    node tests/am1/runs/mkrepos.mjs $names
    : >"$REPOS"
    for n in $names; do
      id=$(awk -v n="$n" '$1 == n {print $2}' "$M3_STATE/am1-runs-repos.txt"); echo "$n $id" >>"$REPOS"
      node -e '
        import("./tests/am1/hosted/lib.mjs").then(async ({ statuses }) => {
          const s = await statuses(process.argv[1]);
          console.log(JSON.stringify({ chain: process.argv[1], home: s.find((x) => x.config)?.config?.config?.normal?.home,
            members: s.map((x) => [x.site, x.role, x.term, x.leader]) }));
        });' "repo:$id"
    done
  } 2>&1 | log ;;
push)
  RL=$O/push.log
  say() { echo "$(date -u +%FT%TZ) $*" | scrub | tee -a "$RL"; }
  TOK=$(awk '$1 == "admin" {print $2}' "$M3_STATE/m3-tokens.txt")
  BED=$(cat "$BED_TOKEN_FILE")
  st() { node -e '
    import("./tests/am1/hosted/lib.mjs").then(async ({ statuses }) => {
      const s = await statuses(process.argv[1]);
      console.log(JSON.stringify(s.map((x) => [x.site, x.role, x.term, x.leader, x.commit])));
    });' "$1"; }
  recv() { curl -s -X POST -H "x-bed-token: $BED" "$GRAFT_EDGE/_bed/status?repo=$1" | jq -c '[.received // {} | to_entries[] | select(.key | test("32\\.(1|2|65520)$")) | "\(.key)=\(.value)"]'; }
  while read -r name id; do
    d=$K/git-$name; rm -rf "$d"
    git -c credential.helper= clone -q "https://x:$TOK@${GRAFT_EDGE#https://}/m3org/$name.git" "$d" 2>&1 | sed "s/$TOK/<tok>/g" | head -2
    say "== $name repo:$id leader $(st "repo:$id") recv $(recv "$id")"
    for i in 1 2 3 4 5 6; do
      (cd "$d" && git commit -q --allow-empty -m "bm5f $TAG $name $i")
      out=$(cd "$d" && GIT_TRACE_CURL=1 GIT_TRACE_CURL_NO_DATA=1 git -c credential.helper= push --porcelain \
        "https://x:$TOK@${GRAFT_EDGE#https://}/m3org/$name.git" "HEAD:refs/heads/bm5f/$TAG/$name/$i" 2>&1)
      code=$(grep -oE "Recv header: HTTP/[0-9.]+ [0-9]+" <<<"$out" | tail -1 | grep -oE "[0-9]+$")
      say "push $name $i: HTTP $code; $(grep -m1 -E '^(error|fatal|!|=|\*)' <<<"$out" | sed "s/$TOK/<tok>/g" | cut -c1-160); leader $(st "repo:$id"); recv $(recv "$id")"
      sleep 3
    done
  done <"$REPOS"
  say "DONE" ;;
checks)
  { while read -r name id; do
      node -e '
        import("./tests/am1/hosted/lib.mjs").then(async ({ statuses, member, voterAt }) => {
          const [chain, prefix] = process.argv.slice(1);
          const s = await statuses(chain);
          console.log(JSON.stringify({ chain, members: s.map((x) => ({ site: x.site, role: x.role, term: x.term, leader: x.leader, commit: x.commit, last: x.last })) }));
          for (const site of [0, 1, 2]) {
            const r = await member(chain, site, "/log?from=1&n=500", undefined, 30_000, voterAt(chain, site)).catch((e) => ({ v: String(e) }));
            const rows = Array.isArray(r.v) ? r.v : [];
            const forced = rows.filter((e) => JSON.stringify(e.refs ?? []).includes(prefix));
            console.log(JSON.stringify({ site, entries: rows.length, forced: forced.map((e) => ({ index: e.index, committed: e.committed, refs: e.refs })) }));
          }
          const st = await member(chain, 0, "/stats", undefined, 30_000, voterAt(chain, 0)).catch((e) => ({ v: {} }));
          console.log(JSON.stringify({ site0_alarms: (st.v?.alarms ?? []).slice(-6) }));
        });' "repo:$id" "bm5f/$TAG/"
    done <"$REPOS"
    s=$(cat "$O/start.ms"); now=$(date -u +%s%3N)
    echo "== graft-m4a-site-a member alarms (Workers Logs)"
    python3 - graft-m4a-site-a "$s" "$now" <<'PY'
import json, os, sys, urllib.request
acct, tok = os.environ["CLOUDFLARE_ACCOUNT_ID"], os.environ["CLOUDFLARE_API_TOKEN"]
script, frm, to = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
URL = f"https://api.cloudflare.com/client/v4/accounts/{acct}/workers/observability/telemetry/query"
seen = 0
for needle in ("NoLead", "names no release seq"):
    body = {"queryId": "q", "timeframe": {"from": frm, "to": to}, "view": "events", "limit": 50,
            "parameters": {"filters": [{"key": "$metadata.service", "operation": "eq", "type": "string", "value": script},
                                       {"key": "$metadata.message", "operation": "includes", "type": "string", "value": needle}]}}
    r = urllib.request.Request(URL, data=json.dumps(body).encode(), method="POST",
                               headers={"authorization": f"Bearer {tok}", "content-type": "application/json"})
    ev = json.load(urllib.request.urlopen(r, timeout=60)).get("result", {}).get("events", {}).get("events", [])
    print(json.dumps({"needle": needle, "lines": len(ev)}))
    for e in sorted(ev, key=lambda e: e["timestamp"])[:8]:
        print(json.dumps({"at": e["timestamp"], "msg": (e.get("$metadata", {}).get("message") or "")[:400]}))
PY
  } 2>&1 | log ;;
edgeback)
  { jq -nc --arg a "$EDGE_N" --arg b "$EDGE_KEPT" '{strategy: "percentage", versions: [{version_id: $a, percentage: 100}, {version_id: $b, percentage: 0}], annotations: {"workers/message": "BM-5 clause 1: edge back"}}' |
      api "/workers/scripts/$EDGE/deployments" -X POST -H 'content-type: application/json' --data-binary @- | jq -c '{deploy: .success, errors: [.errors[]?.code]}'
    deployment $EDGE
  } 2>&1 | log ;;
park)
  { api "/workers/scripts/$OPS/schedules" -X PUT -H 'content-type: application/json' --data '[{"cron":"0 0 1 1 *"}]' >/dev/null
    sleep 2
    api "/workers/scripts/$OPS/schedules" -X PUT -H 'content-type: application/json' --data '[]' | jq -c '{ops_schedules: .result.schedules}'
    jq -nc --arg v "$PARKED" '{strategy: "percentage", versions: [{version_id: $v, percentage: 100}], annotations: {"workers/message": "BM-5 clause 1: parked"}}' |
      api "/workers/scripts/$OPS/deployments" -X POST -H 'content-type: application/json' --data-binary @- | jq -c '{deploy: .success, errors: [.errors[]?.code]}'
    deployment $OPS
    revoke graft-m4a-bm5f-deploy; shred -u "$K/deploy-token.json" 2>/dev/null; echo "token file shredded"
    echo "spend since setup: \$$(spend)"
  } 2>&1 | log ;;
*) echo "unknown phase $PH"; exit 2 ;;
esac
