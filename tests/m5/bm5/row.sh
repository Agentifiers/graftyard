#!/usr/bin/env bash
# BM-5 clause 2 hosted on graft-m4a (build/BUILD-PLAN.md, M5-WP7): "With the vendor token revoked mid-roll, `Halt`,
# `Keys { vendor: None }`, then `Abort { intent, target: N }` returns every site and pinned script to N, and a
# vendor-signed intent submitted after `Keys { vendor: None }` is refused." Stop $5 (the lead's ruling, 2026-11-24).
#
# graft-m4a runs no vendor (`Keys.vendor` is None), so the row stands one in: a throwaway Ed25519 vendor key added by
# a customer-signed `Keys`, and a throwaway R2 token scoped to the release bucket, the vendor's release-bucket token,
# which puts R12 into the bucket and is revoked mid-roll.
#
#   bash tests/m5/bm5/row.sh <tag> <phase>
#
# Phases, in order:
#   bundle    R12 under ~/.graft-m4a/bm5: R9's formats.txt and site/ (the builds of $OUT) plus a marker, signed by m4a's
#             release key. Sites only, so `release roll R12` is three site days and no pinned script moves
#   setup     the vendor key and a customer-signed `Keys` naming it; the vendor token, minted on the install account
#             for graft-m4a-releases only, puts R12 under r12/ over S3; graft-ops' deploy token minted and PATCHed in;
#             graft-m4a-ops deployed from $DEPLOY (test-faults, GATE=pass, 60 s dwells, 1-minute cron, three sites)
#   roll      supervised: vendor-signed `release roll R12` after each IntentDone until site b's day has deployed 10%;
#             then the vendor token is revoked (and a put under it shown refused), and the customer signs `Halt`,
#             `Keys { vendor: None }` and `Abort { intent, target: R9 }`; graft-ops runs the Abort to IntentDone; a
#             vendor-signed `release roll R12` is posted and must be refused; the customer signs `release roll R9`,
#             which returns site a (the finished intent) to R9; the next one it would sign is shown, not posted
#   checks    the entries since setup, every script's deployed version and tag, the vendor token's state
#   teardown  graft-ops back to ac4cd9a5 by version id with schedules []; its deploy token revoked
#   lists     tokens with bm5 in their name, graft-ops' deployment and schedules, the group's state
#
# Needs the graft-m4a env (tests/am1/lf13/env-m4a.sh), OUT (site/ index.js and wasm, graftyard), DEPLOY (the worktree
# OUT was built from), RELVIEW, NODE_MODULES. Every log goes through scrub; token values stay in 0600 files.
set -uo pipefail
cd "$(dirname "$0")/../../.."; ROOT=$PWD
TAG=${1:?tag}; PH=${2:?phase}
O=tests/m5/results/raw/bm5/$TAG; mkdir -p "$O"
W=$HOME/.graft-m4a; K=$W/bm5; mkdir -p "$K"; chmod 700 "$K"
OUT=${OUT:?the builds}; DEPLOY=${DEPLOY:?the worktree OUT was built from}; G=$OUT/graftyard
export RELVIEW=${RELVIEW:?relview}
STORE=bfaa0c1573de4775b1b92a12fb89acb4
OPS=graft-m4a-ops; PARKED=ac4cd9a5-0fc2-400c-8dc8-fdf9391470ef; BUCKET=graft-m4a-releases
R9=e23f69e9b6073484d0925f84cf64e08d8216cba36baf1ba4b2f50aa2ce2d183e
B="https://api.cloudflare.com/client/v4/accounts/$CLOUDFLARE_ACCOUNT_ID"
api() { curl -s -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" "$B$1" "${@:2}"; }
scrub() {
  sed -e "s/$CLOUDFLARE_ACCOUNT_ID/<install-account>/g" -e "s/$CF_WORKERS_SUBDOMAIN/<install-subdomain>/g" \
      -e "s/$CF_ZONE_ID/<zone-id>/g" -e "s#$PWD/#<checkout>/#g" -e "s#$HOME/#~/#g"
}
N=$(ls "$O" | grep -c "^$PH" || true)
L="$O/$PH$( [ "$N" -gt 0 ] && echo "-$((N + 1))" ).log"
STANDIN="stand-ins: a throwaway vendor key (Keys.vendor) and a throwaway R2 token scoped to $BUCKET (the vendor's release-bucket token)"
log() { { date -u +%FT%TZ; echo "$STANDIN"; cat; } | scrub >"$L"; cat "$L"; }
[ -e tests/m0/node_modules ] || ln -sfn "${NODE_MODULES:?}" tests/m0/node_modules
SCRIPTS_WRITE=e086da7e2179491d91ee5f35b3ca210a
CONTAINERS_WRITE=bdbcd690c763475a985e8641dddc09f7
BUCKET_ITEM_WRITE=2efd5506f9c8494dacb1fa10a3e7d5b6
# mint <file> <name> <resource> <permission group id>...: an install-account token, to <file> 0600.
mint() {
  local file=$1 name=$2 res=$3; shift 3
  local groups; groups=$(printf '{"id":"%s"},' "$@"); groups="[${groups%,}]"
  ( umask 077
    jq -nc --arg n "$name" --arg r "$res" --argjson g "$groups" \
      '{name: $n, policies: [{effect: "allow", resources: {($r): "*"}, permission_groups: $g}]}' |
      api /tokens -X POST -H 'content-type: application/json' --data-binary @- >"$file" )
  jq -c --arg n "$name" '{mint: $n, ok: .success, id: (.result.id // ""), errors: [.errors[]?.code]}' "$file"
}
revoke() {
  local id; id=$(api "/tokens?per_page=50" | jq -r --arg n "$1" '.result[] | select(.name == $n) | .id')
  [ -n "$id" ] && api "/tokens/$id" -X DELETE | jq -c --arg n "$1" '{revoke: $n, ok: .success, errors: [.errors[]?.code]}' \
    || echo "{\"revoke\": \"$1\", \"found\": false}"
}
# s3put <key> <file>: a PUT into the release bucket under the vendor token over R2's S3 API (key id = token id,
# secret = SHA-256 of the token value). Prints the status only.
s3put() {
  python3 - "$K/vendor-token.json" "$CLOUDFLARE_ACCOUNT_ID" "$BUCKET" "$1" "$2" <<'PY'
import hashlib, json, sys
import boto3, botocore
t, acct, bucket, key, path = sys.argv[1:]
r = json.load(open(t))["result"]
s3 = boto3.client("s3", endpoint_url=f"https://{acct}.r2.cloudflarestorage.com", region_name="auto",
                  aws_access_key_id=r["id"], aws_secret_access_key=hashlib.sha256(r["value"].encode()).hexdigest(),
                  config=botocore.config.Config(retries={"max_attempts": 1}))
try:
    s3.put_object(Bucket=bucket, Key=key, Body=open(path, "rb").read())
    print(json.dumps({"put": key, "status": 200}))
except botocore.exceptions.ClientError as e:
    print(json.dumps({"put": key, "status": e.response["ResponseMetadata"]["HTTPStatusCode"], "code": e.response["Error"]["Code"]}))
PY
}
deployment() { api "/workers/scripts/$1/deployments" | jq -c --arg s "$1" '.result.deployments[0] | {script: $s, created_on, versions: [.versions[] | "\(.version_id[0:8]) \(.percentage)"]}'; }
tag_of() {
  local d; d=$(api "/workers/scripts/$1/deployments" | jq -c '.result.deployments[0].versions')
  for v in $(echo "$d" | jq -r '.[] | "\(.version_id)/\(.percentage)"'); do
    api "/workers/scripts/$1/versions/${v%/*}" | jq -c --arg s "$1" --arg p "${v#*/}" \
      '(.result.annotations // {}) as $a | {script: $s, version: .result.id[0:8], percentage: ($p|tonumber), tag: (($a["workers/tag"] // "")[0:8])}'
  done
}
spend() { PREFIX=graft-m4a python3 tests/fanout/acctcost.py "$(cat "$O/setup.iso")" 2>/dev/null | head -1 | jq -r .prefix_usd; }
rid() { sha256sum "$1/release.json" | cut -c1-64; }
F=$(grep -E '^(install|customer_key) ' "$W/install.toml" | sed -E 's/.*= *"([^"]*)"/\1/')
# view: the group's pages into $K/pages.json, then relview's record of it from position <from>.
view() { node tests/m5/bf46/view.mjs pages "$K/pages.json" >/dev/null && "$RELVIEW" $F "${1:-0}" <"$K/pages.json"; }
pubkey() { python3 - "$1" <<'PY'
import sys
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from cryptography.hazmat.primitives import serialization
k = Ed25519PrivateKey.from_private_bytes(bytes.fromhex(open(sys.argv[1]).read().strip()))
print(k.public_key().public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw).hex())
PY
}
# customer <spec json> <label>: one entry bound to the group's last digest, signed by the customer key, posted.
customer() {
  local v; v=$(view 0)
  echo "$v" | jq -r .keys_entry | python3 -c "import sys; sys.stdout.buffer.write(bytes.fromhex(sys.stdin.read().strip()))" >"$K/keys.entry"
  echo "$1" >"$K/spec.json"
  "$G" --config "$W/install.toml" release entry "$K/spec.json" --out "$K/$2.unsigned" \
    --prev "$(echo "$v" | jq -r .prev)" --keys "$K/keys.entry" >/dev/null &&
    "$G" sign "$K/$2.unsigned" --out "$K/$2.signed" --key "$W/customer.key" >/dev/null &&
    echo "$2: $("$G" --config "$W/install.toml" release post "$K/$2.signed" --edge "$GRAFT_EDGE" 2>&1 | tr '\n' ' ')"
}
# roll <release> <key file> [extra]: `graftyard release roll` on fresh pages.
roll() {
  node tests/m5/bf46/view.mjs pages "$K/pages.json" >/dev/null
  "$G" --config "$W/install.toml" release roll "$1" --entries "$K/pages.json" --key "$2" --edge "$GRAFT_EDGE" "${@:3}" 2>&1
}

case $PH in
keys)
  { customer "{\"Keys\": {\"vendor\": \"$(pubkey "$K/vendor.seed")\"}}" keys-vendor
    view 0 | jq -c '{count, keys: {generation: .keys.generation, vendor: (.keys.vendor // "" | .[0:12])}}'
  } 2>&1 | log ;;
bundle)
  { d=$K/r12; rm -rf "$d"; mkdir -p "$d/site"
    cp "$W/bf47/r9/formats.txt" "$d/"
    cp "$OUT/site/index.js" "$OUT/site/index_bg.wasm" "$d/site/"
    echo 'export const BM5 = "bm5-r12 site";' >"$d/site/marker.js"
    python3 - "$d" <<'PY'
import hashlib, json, os, sys
d = sys.argv[1]
files = []
for root, _, names in os.walk(d):
    for n in names:
        p = os.path.join(root, n); rel = os.path.relpath(p, d)
        if rel not in ("release.json", "release.sig"):
            files.append({"name": rel, "sha256": hashlib.sha256(open(p, "rb").read()).hexdigest()})
rec = {"files": sorted(files, key=lambda f: f["name"]), "label": "bm5-r12"}
open(f"{d}/release.json", "wb").write(json.dumps(rec, sort_keys=True).encode())
PY
    openssl pkeyutl -sign -rawin -inkey "$W/release.key" -in "$d/release.json" -out "$d/release.sig"
    openssl pkeyutl -verify -rawin -pubin -inkey "$W/release.pub" -in "$d/release.json" -sigfile "$d/release.sig" >/dev/null \
      && echo "r12 $(rid "$d") signed and verified: $(jq -c '[.files[].name]' "$d/release.json")"
    echo "builds of $(cat "$OUT/TREE"); site wasm $(sha256sum "$OUT/site/index_bg.wasm" | cut -c1-16)"
  } 2>&1 | log ;;
setup)
  { date -u +%FT%TZ >"$O/setup.iso"; date -u +%s%3N >"$O/start.ms"
    echo "graft-m4a's day to now: \$$(PREFIX=graft-m4a python3 tests/fanout/acctcost.py | head -1 | jq -r .prefix_usd)"
    view 0 | jq -c '{count, current, drained, keys, site_release, rolled}'
    deployment $OPS
    [ -s "$K/vendor.seed" ] || ( umask 077; python3 -c 'import os; print(os.urandom(32).hex())' >"$K/vendor.seed" )
    echo "vendor key $(pubkey "$K/vendor.seed" | cut -c1-12)…"
    customer "{\"Keys\": {\"vendor\": \"$(pubkey "$K/vendor.seed")\"}}" keys-vendor
    view 0 | jq -c '{count, keys: {generation: .keys.generation, vendor: (.keys.vendor // "" | .[0:12])}}'
    # The vendor's release-bucket token: Bucket Item Write on graft-m4a-releases only.
    [ -s "$K/vendor-token.json" ] || mint "$K/vendor-token.json" graft-m4a-bm5-vendor \
      "com.cloudflare.edge.r2.bucket.${CLOUDFLARE_ACCOUNT_ID}_default_$BUCKET" $BUCKET_ITEM_WRITE
    sleep 10
    (cd "$K/r12" && find . -type f | sed 's#^\./##' | sort | while read -r f; do s3put "r12/$f" "$f"; done)
    echo "R12 $(rid "$K/r12")"
    # graft-ops: a deploy token, then this tree with GATE=pass.
    [ -s "$K/deploy-token.json" ] || mint "$K/deploy-token.json" graft-m4a-bm5-deploy \
      "com.cloudflare.api.account.$CLOUDFLARE_ACCOUNT_ID" $SCRIPTS_WRITE $CONTAINERS_WRITE
    id=$(api "/secrets_store/stores/$STORE/secrets?per_page=100" | jq -r '.result[] | select(.name == "graft-m4a-ops-deploy-token") | .id')
    ( umask 077; jq -c '{value: .result.value}' "$K/deploy-token.json" >"$K/deploy-token.value" )
    api "/secrets_store/stores/$STORE/secrets/$id" -X PATCH -H 'content-type: application/json' --data-binary @"$K/deploy-token.value" |
      jq -c '{patch: "graft-m4a-ops-deploy-token", ok: .success, errors: [.errors[]?.code]}'
    shred -u "$K/deploy-token.value"
    jq '.gate = "pass" | .cron = "* * * * *" | .step_dwell_ms = 60000 | .first_day_ms = 60000' \
      "$W/ops/exec-ck-pending.json" >"$W/ops/exec-bm5.json"
    jq -c '{scripts, gate, cron, step_dwell_ms, first_day_ms, step_min_entries, test_faults}' "$W/ops/exec-bm5.json"
    (cd "$DEPLOY" && export PATH=$HOME/.cargo/bin:$PATH CARGO_TARGET_DIR=$DEPLOY/target \
      && ln -sfn "${NODE_MODULES:?}" tests/m0/node_modules \
      && node workers/ops/deploy.mjs $OPS $STORE graft-m4a-mint-wrap-key - graft-m4a-secrets-wrap-key-b 1 \
           --executor "$W/ops/exec-bm5.json" 2>&1 | grep -E "Current Version|rror" | tail -3)
    deployment $OPS
    api "/workers/scripts/$OPS/schedules" | jq -c '{ops_schedules: [.result.schedules[]?.cron]}'
  } 2>&1 | log ;;
roll)
  R12=$(rid "$K/r12"); RL=$O/roll.log; STOP=${STOP_USD:-5}
  say() { echo "$(date -u +%FT%TZ) $*" | scrub | tee -a "$RL"; }
  cur() { view "${1:-0}" | jq -c '{count, current, drained, site_release, vendor: (.keys.vendor // null | if . then .[0:12] else null end), keys_gen: .keys.generation}'; }
  intent_of() { view 0 | jq -r '[.entries[] | select(.kind == "Intent")] | last | .entry | capture("id (?<id>[0-9A-Z]{26})").id'; }
  split_b() { api "/workers/scripts/graft-m4a-site-b/deployments" | jq -c '[.result.deployments[0].versions[] | .percentage] | sort'; }
  say "$STANDIN"; say "BM-5 clause 2: release roll R12 $R12 (vendor-signed), stop \$$STOP from $(cat "$O/setup.iso")"
  spent() { python3 -c "import sys; sys.exit(0 if float('${1:-0}') >= $STOP else 1)"; }
  # 1. Vendor-signed roll until site b's day stands at 10%.
  i=0
  while true; do
    v=$(cur) || { say "view failed"; sleep 20; continue; }
    if [ $((i % 6)) -eq 0 ]; then s=$(spend); say "spend \$$s; $v"; spent "$s" && { say "STOP at \$$s"; exit 1; }; fi
    i=$((i + 1))
    if [ "$(echo "$v" | jq -c .current)" = null ]; then
      say "vendor roll: $(roll "$R12" "$K/vendor.seed" | tr '\n' ' ')"
    elif echo "$v" | jq -e '.current.intent | test("Site\\(SiteId\\(1\\)\\)")' >/dev/null \
      && [ "$(echo "$v" | jq .current.next)" -ge 3 ]; then
      say "site b's day at 10%: split $(split_b); $v"; break
    fi
    sleep 10
  done
  # 2. Mid-roll: the vendor token revoked, then Halt, Keys { vendor: None }, Abort { intent, target: R9 }.
  ID=$(intent_of); say "intent $ID"
  say "$(revoke graft-m4a-bm5-vendor)"; sleep 15
  echo "after revocation" >"$K/after.txt"; say "vendor token put after revocation: $(s3put r12/after-revoke.txt "$K/after.txt")"
  say "$(customer "{\"Halt\": {\"intent\": \"$ID\"}}" halt)"
  say "$(customer '{"Keys": {"vendor": null}}' keys-novendor)"
  say "$(customer "{\"Abort\": {\"intent\": \"$ID\", \"target\": \"$R9\"}}" abort)"
  say "after the three: $(cur) split b $(split_b)"
  # 3. graft-ops runs the Abort to IntentDone.
  while true; do
    v=$(cur) || { sleep 20; continue; }
    [ "$(echo "$v" | jq -c .current)" = null ] && { say "abort done: $v; split b $(split_b)"; break; }
    say "aborting: $(echo "$v" | jq -c .current)"; sleep 20
  done
  # 4. A vendor-signed intent after Keys { vendor: None }: refused.
  say "vendor roll after Keys { vendor: None }: $(roll "$R12" "$K/vendor.seed" | tr '\n' ' ')"
  # 5. The customer returns the finished intent's site: release roll R9 signs SiteDay a.
  say "customer roll R9: $(roll "$R9" "$W/customer.key" | tr '\n' ' ')"
  sleep 30
  while true; do
    v=$(cur) || { sleep 20; continue; }
    if [ $((i % 6)) -eq 0 ]; then s=$(spend); spent "$s" && { say "STOP at \$$s"; exit 1; }; fi
    i=$((i + 1))
    [ "$(echo "$v" | jq -c .current)" = null ] && { say "customer day done: $v"; break; }
    sleep 20
  done
  # 6. What release roll R9 would sign next, written out and not posted.
  say "next for R9 (not posted): $(roll "$R9" "$W/customer.key" --out "$K/next.signed" | tr '\n' ' ')"
  say "DONE" ;;
checks)
  { s=$(cat "$O/start.ms"); view 0 | jq -c --argjson s "$s" '{count, current, drained, site_release, rolled, keys}'
    echo "== entries since setup"
    view 0 | jq -c --argjson s "$s" '.entries[] | select(.at >= $s) | {index, at: (.at/1000|todate), kind, sigs, entry: (.entry[0:120])}'
    echo "== deployments"; for x in a b c; do tag_of graft-m4a-site-$x; done; tag_of graft-m4a-edge; deployment $OPS
    echo "== tokens"; api "/tokens?per_page=50" | jq -c '[.result[] | select(.name|test("bm5")) | {name, status}]'
    echo "spend since setup: \$$(spend)"
  } 2>&1 | log ;;
teardown)
  { api "/workers/scripts/$OPS/schedules" -X PUT -H 'content-type: application/json' --data '[{"cron":"0 0 1 1 *"}]' >/dev/null
    sleep 2
    api "/workers/scripts/$OPS/schedules" -X PUT -H 'content-type: application/json' --data '[]' | jq -c '{ops_schedules: .result.schedules}'
    jq -nc --arg v "$PARKED" '{strategy: "percentage", versions: [{version_id: $v, percentage: 100}], annotations: {"workers/message": "BM-5 teardown: back to the parked version"}}' |
      api "/workers/scripts/$OPS/deployments" -X POST -H 'content-type: application/json' --data-binary @- | jq -c '{deploy: .success, errors: [.errors[]?.code]}'
    deployment $OPS
    revoke graft-m4a-bm5-deploy; revoke graft-m4a-bm5-vendor
    shred -u "$K/deploy-token.json" "$K/vendor-token.json" 2>/dev/null; echo "token files shredded"
  } 2>&1 | log ;;
lists)
  { api "/tokens?per_page=50" | jq -c '{bm5_tokens: [.result[] | select(.name|test("bm5")) | {name, status}]}'
    deployment $OPS; api "/workers/scripts/$OPS/schedules" | jq -c '{ops_schedules: .result.schedules}'
    view 0 | jq -c '{count, current, drained, site_release, vendor: .keys.vendor, keys_gen: .keys.generation}'
  } 2>&1 | log ;;
*) echo "unknown phase $PH"; exit 2 ;;
esac
