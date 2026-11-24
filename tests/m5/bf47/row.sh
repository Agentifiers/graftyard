#!/usr/bin/env bash
# BF-47 hosted on graft-m4a (build/BUILD-PLAN.md, M5-WP5; tests/m5/results/m5-wp5-vault.md, "BF-47 hosted"): one
# `graftyard release roll R9` end to end, `SiteDay { Workflows }` and the vault step included. Pass: the order is
# DESIGN's Release roll, and the vault is upgraded with no install token. Fail: any other order, or an install token.
#
# Two stand-ins, labelled in every log:
#   - the vault account is the RUNNER account (FRAME.md, "Operator decision, 2026-11-23, the runner account stands in
#     as BF-47's vault account"), which also holds graft-runners, so it is not isolated as a vault account would be;
#   - graft-m4a-workflows is a minimal Worker (a fetch handler that answers its version_metadata tag, no cron, queue,
#     Durable Object, container or bucket), which tests the roll's order, not Workflows.
#
#   bash tests/m5/bf47/row.sh <tag> <phase>
#
# Phases, in order:
#   bundles   build R8 and R9 under ~/.graft-m4a/bf47 from $OUT (the builds of one merged-main tree), each with
#             formats.txt, site/, workflows/, edge/, ops/ and vault/; R9 = R8 plus a marker in every folder; both signed
#             by m4a's release key
#   setup     (row 47a) mint the vault token on the runner account (Workers Scripts Write and R2 Storage Write, that
#             account only); deploy the Workflows stand-in with R8's workflows/; `vault init` with R8 into the runner
#             account under that token
#   ops       (row 47b) mint graft-ops' deploy token into graft-m4a-ops-deploy-token; deploy graft-m4a-ops from the
#             same tree (test-faults, GATE=pass, 60 s dwells, 1-minute cron, OPS_SCRIPTS with workflows, edge, ops);
#             put R9 into graft-m4a-releases under r9/
#   roll      the loop, as a supervised service: every 60 s read the group; with nothing current, run `release roll R9`
#             with the vault step (install token as CLOUDFLARE_API_TOKEN, the vault's as CLOUDFLARE_VAULT_API_TOKEN);
#             stop at `{"roll": "done"}`, or at 47b's $5 stop from the ops phase (cron off). Then once more for
#             `{"vault": "at"}`. The probe runs after it, once the vault step is due and no intent is owed
#   probe     the fail line: `release roll` with CLOUDFLARE_VAULT_API_TOKEN set to the install's token, and with the
#             install's account as --vault-account, each against a local recorder as the API and the edge
#   checks    the entries since setup, the vault's workers/tag, every non-read on the runner account from its audit
#             log with the API token that made it, and the Workflows stand-in's tag
#   teardown  graft-ops parked on this build with GATE=pending and no cron; its deploy token revoked; the vault in
#             the operator's order (lock rules to none, empty, bucket, secret, script), the vault token revoked; the
#             Workflows stand-in deleted
#   lists     buckets, scripts and tokens on both accounts, the vault bucket's lock rules, graft-ops' schedules
#
# Needs: the graft-m4a env (tests/am1/lf13/env-m4a.sh) with CLOUDFLARE_ACCOUNT_ID, CLOUDFLARE_API_TOKEN,
# CLOUDFLARE_RUNNER_ACCOUNT_ID and CLOUDFLARE_RUNNER_API_TOKEN; OUT (site/, edge/, ops/ worker-build outputs, vault/
# with entry.js and its wasm, graftyard, and TREE), DEPLOY (the merged-main worktree they were built from), RELVIEW,
# NODE_MODULES. Every log goes through scrub; token values are read from 0600 files and sent only as headers or bodies.
set -uo pipefail
cd "$(dirname "$0")/../../.."; ROOT=$PWD
TAG=${1:?tag}; PH=${2:?phase}
O=tests/m5/results/raw/bf47/$TAG; mkdir -p "$O"
W=$HOME/.graft-m4a; K=$W/bf47; mkdir -p "$K"; chmod 700 "$K"
OUT=${OUT:?the builds}; DEPLOY=${DEPLOY:?the merged-main worktree}; G=$OUT/graftyard
export RELVIEW=${RELVIEW:?tests/m3/hosted/tools relview}
STORE=bfaa0c1573de4775b1b92a12fb89acb4
OPS=graft-m4a-ops; WF=graft-m4a-workflows; V=graft-m4a-vault-standin
RA=$CLOUDFLARE_RUNNER_ACCOUNT_ID
B="https://api.cloudflare.com/client/v4/accounts/$CLOUDFLARE_ACCOUNT_ID"
RB="https://api.cloudflare.com/client/v4/accounts/$RA"
api() { curl -s -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" "$B$1" "${@:2}"; }
rapi() { curl -s -H "Authorization: Bearer $CLOUDFLARE_RUNNER_API_TOKEN" "$RB$1" "${@:2}"; }
scrub() {
  sed -e "s/$CLOUDFLARE_ACCOUNT_ID/<install-account>/g" -e "s/$RA/<runner-account>/g" \
      -e "s/$CF_WORKERS_SUBDOMAIN/<install-subdomain>/g" -e "s/$CF_RUNNER_WORKERS_SUBDOMAIN/<runner-subdomain>/g" \
      -e "s/$CF_ZONE_ID/<zone-id>/g" -e "s#$PWD/#<checkout>/#g" -e "s#$HOME/#~/#g"
}
N=$(ls "$O" | grep -c "^$PH" || true)
L="$O/$PH$( [ "$N" -gt 0 ] && echo "-$((N + 1))" ).log"
STANDIN="stand-ins: the vault account is the runner account (not isolated from graft-runners); $WF is a minimal Worker, not Workflows"
log() { { date -u +%FT%TZ; echo "$STANDIN"; cat; } | scrub >"$L"; cat "$L"; }
[ -e tests/m0/node_modules ] || ln -sfn "${NODE_MODULES:?a tests/m0/node_modules with wrangler}" tests/m0/node_modules
wrangler() { npx --prefix "$ROOT/tests/m0" wrangler "$@"; }
SCRIPTS_WRITE=e086da7e2179491d91ee5f35b3ca210a
CONTAINERS_WRITE=bdbcd690c763475a985e8641dddc09f7
R2_WRITE=bf7481a1826f439697cb59a20b22293e
# mint <api fn> <account> <file> <name> <permission group id>...: an account token on that account only, to <file> 0600.
mint() {
  local f=$1 acct=$2 file=$3 name=$4; shift 4
  local groups; groups=$(printf '{"id":"%s"},' "$@"); groups="[${groups%,}]"
  ( umask 077
    jq -nc --arg n "$name" --arg a "com.cloudflare.api.account.$acct" --argjson g "$groups" \
      '{name: $n, policies: [{effect: "allow", resources: {($a): "*"}, permission_groups: $g}]}' |
      $f /tokens -X POST -H 'content-type: application/json' --data-binary @- >"$file" )
  jq -c --arg n "$name" '{mint: $n, ok: .success, id: (.result.id // "" | .[0:8]), errors: [.errors[]?.code]}' "$file"
}
# revoke <api fn> <name>
revoke() {
  local id; id=$($1 "/tokens?per_page=50" | jq -r --arg n "$2" '.result[] | select(.name == $n) | .id')
  [ -n "$id" ] && $1 "/tokens/$id" -X DELETE | jq -c --arg n "$2" '{revoke: $n, ok: .success, errors: [.errors[]?.code]}' \
    || echo "{\"revoke\": \"$2\", \"found\": false}"
}
vtoken() { jq -r .result.value "$K/vault-token.json"; }
cronoff() {
  api "/workers/scripts/$OPS/schedules" -X PUT -H 'content-type: application/json' --data '[{"cron":"0 0 1 1 *"}]' >/dev/null
  sleep 2
  api "/workers/scripts/$OPS/schedules" -X PUT -H 'content-type: application/json' --data '[]' | jq -c '{ops_schedules: .result.schedules}'
}
deployment() { api "/workers/scripts/$1/deployments" | jq -c --arg s "$1" '.result.deployments[0] | {script: $s, created_on, versions: [.versions[] | "\(.version_id[0:8]) \(.percentage)"]}'; }
# tag_of <api fn> <script>: the workers/tag of each version the script's deployment serves.
tag_of() {
  local d; d=$($1 "/workers/scripts/$2/deployments" | jq -c '.result.deployments[0].versions')
  for v in $(echo "$d" | jq -r '.[] | "\(.version_id)/\(.percentage)"'); do
    $1 "/workers/scripts/$2/versions/${v%/*}" | jq -c --arg s "$2" --arg p "${v#*/}" \
      '(.result.annotations // .result.metadata.annotations // {}) as $a | {script: $s, version: .result.id[0:8], percentage: ($p|tonumber), tag: ($a["workers/tag"] // null), message: ($a["workers/message"] // null)}'
  done
}
# spend <from file>: graft-m4a's spend since the time in that file (setup.iso for 47a, ops.iso for 47b).
spend() { PREFIX=graft-m4a python3 tests/fanout/acctcost.py "$(cat "$O/$1")" 2>/dev/null | head -1 | jq -r .prefix_usd; }
rid() { sha256sum "$1/release.json" | cut -c1-64; }

case $PH in
bundles)
  { echo "builds of $(cat "$OUT/TREE")"
    for r in r8 r9; do
      d=$K/$r; rm -rf "$d"; mkdir -p "$d"/{site,workflows,edge,ops,vault}
      # formats.txt: graft-m4a's 26 edge formats plus 65000.1, which BF-7 enabled for good, as every m4a release must.
      R2_ACCOUNT=$CLOUDFLARE_ACCOUNT_ID R2_TOKEN_VAR=CLOUDFLARE_API_TOKEN \
        python3 tests/m5/vaulthosted/r2.py graft-m4a-releases get r6/formats.txt "$d/formats.txt" >/dev/null
      for w in site edge ops; do cp "$OUT/$w/index.js" "$OUT/$w/index_bg.wasm" "$d/$w/"; done
      cp "$OUT"/vault/* "$d/vault/"
      cat >"$d/workflows/entry.js" <<EOF
// BF-47's Workflows stand-in ($r): it tests the roll's order, not Workflows. No cron, queue, Durable Object,
// container or bucket; it answers the version it runs, whose tag is the release graft-ops uploaded it under.
const MARKER = "bf47-$r";
export default {
  async fetch(_req, env) {
    return Response.json({ standin: "workflows", marker: MARKER, version: env.VERSION?.id ?? null, tag: env.VERSION?.tag ?? null });
  },
};
EOF
      if [ $r = r9 ]; then
        for w in site edge ops vault; do echo "export const BF47 = \"bf47-r9 $w\";" >"$d/$w/marker.js"; done
      fi
      python3 - "$d" "bf47-$r" <<'PY'
import hashlib, json, os, sys
d, label = sys.argv[1], sys.argv[2]
files = []
for root, _, names in os.walk(d):
    for n in names:
        p = os.path.join(root, n); rel = os.path.relpath(p, d)
        if rel in ("release.json", "release.sig"):
            continue
        files.append({"name": rel, "sha256": hashlib.sha256(open(p, "rb").read()).hexdigest()})
rec = {"files": sorted(files, key=lambda f: f["name"]), "label": label}
open(f"{d}/release.json", "wb").write(json.dumps(rec, sort_keys=True).encode())
PY
      openssl pkeyutl -sign -rawin -inkey "$W/release.key" -in "$d/release.json" -out "$d/release.sig"
      openssl pkeyutl -verify -rawin -pubin -inkey "$W/release.pub" -in "$d/release.json" -sigfile "$d/release.sig" >/dev/null \
        && echo "$r $(rid "$d") signed and verified: $(jq -c '[.files[].name]' "$d/release.json")"
    done
    for w in site edge ops; do sha256sum "$OUT/$w/index_bg.wasm" | cut -c1-16 | sed "s/^/$w wasm sha256 /"; done
  } 2>&1 | log ;;
setup)
  { git rev-parse --short HEAD; echo "builds of $(cat "$OUT/TREE")"
    date -u +%FT%TZ >"$O/setup.iso"; date -u +%s%3N >"$O/start.ms"
    echo "graft-m4a's day to now: \$$(PREFIX=graft-m4a python3 tests/fanout/acctcost.py | head -1 | jq -r .prefix_usd)"
    rapi "/workers/scripts" | jq -c '{runner_scripts_before: [.result[]?.id]}'
    rapi "/r2/buckets" | jq -c '{runner_buckets_before: [.result.buckets[]?.name]}'
    # The vault's own token, on the runner account only, never the runner's token or the install's.
    [ -s "$K/vault-token.json" ] || mint rapi "$RA" "$K/vault-token.json" graft-m4a-bf47-vault $SCRIPTS_WRITE $R2_WRITE
    jq -c '{vault_token_id: .result.id}' "$K/vault-token.json"
    # The Workflows stand-in, with R8's workflows/ as its first version (N for SiteDay { Workflows }).
    mkdir -p "$K/wf"; cp "$K/r8/workflows/entry.js" "$K/wf/entry.js"
    jq -n --arg n $WF --arg m "$K/wf/entry.js" '{name: $n, main: $m, compatibility_date: "2026-10-01", workers_dev: true,
      preview_urls: false, version_metadata: {binding: "VERSION"}}' >"$K/wf/wrangler.json"
    (cd "$K/wf" && wrangler deploy -c wrangler.json 2>&1 | grep -E "Uploaded|Deployed|Current Version|rror" | tail -3)
    deployment $WF
    api "/workers/scripts/$WF/schedules" | jq -c '{workflows_schedules: [.result.schedules[]?.cron]}'
    # The vault, at R8, under its own token.
    CLOUDFLARE_VAULT_API_TOKEN=$(vtoken) "$G" --config "$W/install.toml" vault init --account "$RA" \
      --key "$W/customer.key" --release "$K/r8" --release-key "$W/release.pub" --bucket $V --script $V \
      --edge "$GRAFT_EDGE" --days 1
    echo "vault init exit $?"
    tag_of rapi $V
    R2_ACCOUNT=$RA R2_TOKEN_VAR=CLOUDFLARE_RUNNER_API_TOKEN python3 tests/m5/vaulthosted/r2.py $V lock-get
  } 2>&1 | log ;;
ops)
  { echo "builds of $(cat "$OUT/TREE")"; date -u +%FT%TZ >"$O/ops.iso"
    echo "row 47a spend: \$$(spend setup.iso)"; deployment $OPS
    mint api "$CLOUDFLARE_ACCOUNT_ID" "$K/deploy-token.json" graft-m4a-bf47-deploy $SCRIPTS_WRITE $CONTAINERS_WRITE
    id=$(api "/secrets_store/stores/$STORE/secrets?per_page=100" | jq -r '.result[] | select(.name == "graft-m4a-ops-deploy-token") | .id')
    ( umask 077; jq -c '{value: .result.value}' "$K/deploy-token.json" >"$K/deploy-token.value" )
    api "/secrets_store/stores/$STORE/secrets/$id" -X PATCH -H 'content-type: application/json' --data-binary @"$K/deploy-token.value" |
      jq -c '{patch: "graft-m4a-ops-deploy-token", ok: .success, errors: [.errors[]?.code]}'
    shred -u "$K/deploy-token.value"
    jq '.scripts += {"workflows": "graft-m4a-workflows", "edge": "graft-m4a-edge", "ops": "graft-m4a-ops"}
        | .gate = "pass" | .cron = "* * * * *" | .step_dwell_ms = 60000 | .first_day_ms = 60000' \
      "$W/ops/exec-ck-pending.json" >"$W/ops/exec-bf47.json"
    jq -c '{scripts, gate, cron, step_dwell_ms, first_day_ms, test_faults, slots}' "$W/ops/exec-bf47.json"
    (cd "$DEPLOY" && export PATH=$HOME/.cargo/bin:$PATH CARGO_TARGET_DIR=$DEPLOY/target \
      && ln -sfn "${NODE_MODULES:?}" tests/m0/node_modules \
      && node workers/ops/deploy.mjs $OPS $STORE graft-m4a-mint-wrap-key - graft-m4a-secrets-wrap-key-b 1 \
           --executor "$W/ops/exec-bf47.json" 2>&1 | grep -E "Current Version|rror" | tail -3)
    deployment $OPS
    api "/workers/scripts/$OPS/schedules" | jq -c '{ops_schedules: [.result.schedules[]?.cron]}'
    # R9 into the release bucket; graft-ops uploads its six versions and commits ReleaseUploaded on its own.
    (cd "$K/r9" && find . -type f | sed 's#^\./##' | sort | while read -r f; do
      wrangler r2 object put "graft-m4a-releases/r9/$f" --file "$f" --remote >/dev/null 2>&1 && echo "put r9/$f" || echo "PUT FAILED r9/$f"
    done)
    echo "R9 $(rid "$K/r9")"
  } 2>&1 | log ;;
roll)
  R9=$(rid "$K/r9"); RL=$O/roll.log; STOP=${STOP_USD:-5}
  say() { echo "$(date -u +%FT%TZ) $*" | scrub | tee -a "$RL"; }
  say "$STANDIN"; say "release roll R9 $R9, row 47b stop \$$STOP from $(cat "$O/ops.iso")"
  roll() {
    CLOUDFLARE_VAULT_API_TOKEN=$(vtoken) "$G" --config "$W/install.toml" release roll "$R9" --entries "$K/pages.json" \
      --key "$W/customer.key" --edge "$GRAFT_EDGE" --vault-account "$RA" --vault-release "$K/r9" \
      --release-key "$W/release.pub" --vault-script $V 2>&1
  }
  i=0
  while true; do
    v=$(node tests/m5/bf46/view.mjs pages "$K/pages.json" 2>&1) || { say "view: $v"; sleep 30; continue; }
    if [ $((i % 5)) -eq 0 ]; then
      s=$(spend ops.iso); say "spend \$$s; $v"
      if python3 -c "import sys; sys.exit(0 if float('${s:-0}') >= $STOP else 1)"; then
        say "STOP: \$$s reached the \$$STOP stop; graft-ops' cron off"; cronoff | scrub | tee -a "$RL"; exit 1
      fi
    fi
    i=$((i + 1))
    if [ "$(echo "$v" | jq -c .current)" = null ]; then
      a=$(roll); say "roll: $(echo "$a" | tr '\n' ' ')"
      if echo "$a" | grep -q '"roll": *"done"'; then
        sleep 5; node tests/m5/bf46/view.mjs pages "$K/pages.json" >/dev/null
        say "again: $(roll | tr '\n' ' ')"; say "DONE"; exit 0
      fi
    fi
    sleep 60
  done ;;
probe)
  { R9=$(rid "$K/r9")
    port=8797; : >"$K/recorder.log"
    python3 - "$port" "$K/recorder.log" <<'PY' &
import http.server, sys
port, path = int(sys.argv[1]), sys.argv[2]
class H(http.server.BaseHTTPRequestHandler):
    def any(self):
        open(path, "a").write(f"{self.command} {self.path}\n"); self.send_response(599); self.end_headers()
    do_GET = do_POST = do_PUT = do_DELETE = do_PATCH = any
    def log_message(self, *a): pass
http.server.HTTPServer(("127.0.0.1", port), H).serve_forever()
PY
    rec=$!; sleep 1
    node tests/m5/bf46/view.mjs pages "$K/pages.json"
    echo "fail line 1: CLOUDFLARE_VAULT_API_TOKEN set to the install's token (CLOUDFLARE_API_TOKEN), the API and the edge a local recorder"
    GRAFTYARD_CF_API=http://127.0.0.1:$port CLOUDFLARE_VAULT_API_TOKEN=$CLOUDFLARE_API_TOKEN "$G" --config "$W/install.toml" \
      release roll "$R9" --entries "$K/pages.json" --key "$W/customer.key" --edge http://127.0.0.1:$port \
      --vault-account "$RA" --vault-release "$K/r9" --release-key "$W/release.pub" --vault-script $V
    echo "exit $?; requests the recorder saw: $(wc -l <"$K/recorder.log")"
    echo "fail line 2: --vault-account the install's own account, under the vault's token"
    GRAFTYARD_CF_API=http://127.0.0.1:$port CLOUDFLARE_VAULT_API_TOKEN=$(vtoken) "$G" --config "$W/install.toml" \
      release roll "$R9" --entries "$K/pages.json" --key "$W/customer.key" --edge http://127.0.0.1:$port \
      --vault-account "$CLOUDFLARE_ACCOUNT_ID" --vault-release "$K/r9" --release-key "$W/release.pub" --vault-script $V
    echo "exit $?; requests the recorder saw: $(wc -l <"$K/recorder.log")"
    cat "$K/recorder.log"; kill $rec
  } 2>&1 | log ;;
checks)
  { start=$(cat "$O/start.ms")
    echo "== the group's entries since setup"; node tests/m5/bf46/view.mjs entries "$start"
    echo "== the vault's deployed version"; tag_of rapi $V
    echo "== the Workflows stand-in"; tag_of api $WF
    curl -s "https://$WF.$CF_WORKERS_SUBDOMAIN.workers.dev/"; echo
    echo "== the edge and graft-ops"; tag_of api graft-m4a-edge; tag_of api $OPS
    for s in a b c; do tag_of api graft-m4a-site-$s; done
    echo "== tokens: the install's, the vault's, the runner's own"
    curl -s -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" "$B/tokens/verify" | jq -c '{install_token: .result.id}'
    jq -c '{vault_token: .result.id}' "$K/vault-token.json"
    curl -s -H "Authorization: Bearer $CLOUDFLARE_RUNNER_API_TOKEN" "$RB/tokens/verify" | jq -c '{runner_token: .result.id}'
    echo "== every write on the runner account since setup, from its audit log, with the token that made it"
    python3 - "$(cat "$O/setup.iso")" <<'PY'
import collections, datetime, json, os, urllib.parse, urllib.request
acct, tok = os.environ["CLOUDFLARE_RUNNER_ACCOUNT_ID"], os.environ["CLOUDFLARE_RUNNER_API_TOKEN"]
since = __import__("sys").argv[1]
before = datetime.datetime.now(datetime.UTC).strftime("%Y-%m-%dT%H:%M:%SZ")
rows, cursor = [], None
while True:
    q = {"since": since, "before": before, "limit": 1000, **({"cursor": cursor} if cursor else {})}
    url = f"https://api.cloudflare.com/client/v4/accounts/{acct}/logs/audit?" + urllib.parse.urlencode(q)
    v = json.load(urllib.request.urlopen(urllib.request.Request(url, headers={"authorization": f"Bearer {tok}"}), timeout=60))
    rows += v.get("result") or []
    cursor = (v.get("result_info") or {}).get("cursor")
    if not cursor or not v.get("result"):
        break
writes = [r for r in rows if (r.get("action") or {}).get("type") != "view"]
by = collections.Counter()
for r in sorted(writes, key=lambda r: r["action"]["time"]):
    t = (r.get("actor") or {}).get("token") or {}
    res = r.get("resource") or {}
    by[(t.get("id"), t.get("name"))] += 1
    print(json.dumps({"time": r["action"]["time"], "what": r["action"].get("description"), "result": r["action"].get("result"),
                      "resource": f"{res.get('product')}/{res.get('type')} {res.get('id')}", "token": t.get("id"), "token_name": t.get("name"),
                      "context": (r.get("actor") or {}).get("context")}))
print(json.dumps({"writes": len(writes), "reads": len(rows) - len(writes), "by_token": [[k[0], k[1], n] for k, n in by.items()]}))
PY
    echo "row 47b spend so far: \$$(spend ops.iso); 47a and 47b: \$$(spend setup.iso)"
  } 2>&1 | log ;;
teardown)
  { cronoff
    # graft-ops parked on this build (R9's), GATE=pending, no cron, no slots, OPS_SCRIPTS the three sites.
    (cd "$DEPLOY" && export PATH=$HOME/.cargo/bin:$PATH CARGO_TARGET_DIR=$DEPLOY/target \
      && node workers/ops/deploy.mjs $OPS $STORE graft-m4a-mint-wrap-key - graft-m4a-secrets-wrap-key-b 1 \
           --executor "$W/ops/exec-ck-pending.json" 2>&1 | grep -E "Current Version|rror" | tail -3)
    cronoff; deployment $OPS
    revoke api graft-m4a-bf47-deploy
    # The vault, in the operator's order.
    r2v() { R2_ACCOUNT=$RA R2_TOKEN_VAR=CLOUDFLARE_RUNNER_API_TOKEN python3 tests/m5/vaulthosted/r2.py $V "$@"; }
    r2v lock-clear; r2v lock-get; r2v empty; r2v bucket-delete
    rapi "/workers/scripts/$V/secrets/EXPORT_TOKEN" -X DELETE | jq -c '{vault_secret_delete: .success, errors: [.errors[]?.code]}'
    rapi "/workers/scripts/$V" -X DELETE | jq -c '{vault_script_delete: .success, errors: [.errors[]?.code]}'
    revoke rapi graft-m4a-bf47-vault
    api "/workers/scripts/$WF" -X DELETE | jq -c '{workflows_standin_delete: .success, errors: [.errors[]?.code]}'
    for f in "$K"/*.json; do [ -f "$f" ] && shred -u "$f"; done; rm -rf "$K/wf"
    echo "token files shredded"
  } 2>&1 | log ;;
lists)
  { echo "== runner account (the vault stand-in)"
    rapi "/r2/buckets" | jq -c '{buckets: [.result.buckets[]?.name]}'
    rapi "/workers/scripts" | jq -c '{scripts: [.result[]?.id]}'
    rapi "/tokens?per_page=50" | jq -c '{tokens: [.result[]?.name]}'
    R2_ACCOUNT=$RA R2_TOKEN_VAR=CLOUDFLARE_RUNNER_API_TOKEN python3 tests/m5/vaulthosted/r2.py $V lock-get
    echo "== install account"
    api "/workers/scripts" | jq -c '{workflows_standin: ([.result[]?.id] | index("graft-m4a-workflows"))}'
    api "/tokens?per_page=50" | jq -c '{bf47_tokens: [.result[] | select(.name | startswith("graft-m4a-bf47")) | .name]}'
    api "/workers/scripts/$OPS/schedules" | jq -c '{ops_schedules: [.result.schedules[]?.cron]}'
    deployment $OPS
    node tests/m5/bf46/view.mjs slots
  } 2>&1 | log ;;
*) echo "phase: bundles setup ops roll probe checks teardown lists"; exit 2 ;;
esac
