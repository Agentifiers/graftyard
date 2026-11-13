#!/usr/bin/env bash
# BF-41 on an empty `Release` group: a throwaway product install, graft-m5-bf41-{a,b,c} plus the edge
# graft-m5-bf41-edge, every script a product build (no test-faults) carrying the install's anchor. Two genesis entries
# go to its empty group: one signed by another key (a self-consistent genesis naming that key as customer), then the
# matching one, then the other again. Only the matching one may commit; the 409 replay at the end names its digest at
# the index it committed at, so nothing else did. `release post` sends the genesis, not
# `install`, so the 3,072 Tokens shards are not created. Then everything is deleted (tests/m1/integ/throwaway.sh).
#   bash tests/m5/bf41.sh run | bash tests/m5/bf41.sh delete
set -uo pipefail
export PATH=$HOME/.cargo/bin:$PATH
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
W=$HOME/.graft-m5-bf41
CLI=${CLI:-$ROOT/target/release/graftyard}
OUT=${OUT:-$ROOT/tests/m5/results/raw}
P=graft-m5-bf41
EDGE=$P-edge
EDGE2=$P-edge2
# m4a's git image (product), already in the account's registry.
IMAGE=${IMAGE:-registry.cloudflare.com/<cf-account-id>/graft-m1-site-gitshard-git@sha256:099a022c23a31d2ca50ccb745a1b002cbe8af1f4c571fff713bc1a8b698a1440}
CF=https://api.cloudflare.com/client/v4/accounts/$CLOUDFLARE_ACCOUNT_ID
api() { curl -s -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" "$@"; }
step() { echo; echo "=== $* ($(date -u +%T))"; }
post() { "$CLI" --config "$W/install.toml" release post "$1" --edge "https://${2:-$EDGE}.<workers-subdomain>.workers.dev"; }

run() {
  mkdir -p "$W" "$OUT" && cd "$W" || exit 2
  for k in customer release other; do
    [ -f $k.key ] || { openssl genpkey -algorithm ed25519 -out $k.key && openssl pkey -in $k.key -pubout -out $k.pub; }
  done
  step init
  [ -f install.toml ] || "$CLI" init --account "$CLOUDFLARE_ACCOUNT_ID" --zone git.m5bf41.example --customer-key customer.pub || return 1
  step "sites add (product bundle workers/site/build)"
  for s in a:enam b:weur c:apac; do
    "$CLI" sites add --script "$P-${s%%:*}" --region "${s#*:}" --bundle "$ROOT/workers/site/build" --image "$IMAGE" || return 1
  done
  step "genesis: the matching one, and another key's"
  [ -f genesis.unsigned ] || "$CLI" genesis --out genesis.unsigned --release-key release.pub --offline-keys || return 1
  "$CLI" sign genesis.unsigned --out genesis.signed --key customer.key || return 1
  # The same install and sites, with `other` as the customer key: a genesis that verifies under its own key.
  sed "s|^customer_key = .*|customer_key = \"$(openssl pkey -pubin -in other.pub -outform DER | tail -c 32 | od -An -tx1 -v | tr -d ' \n')\"|" \
    install.toml > other.toml
  [ -f other.unsigned ] || "$CLI" --config other.toml genesis --out other.unsigned --release-key release.pub --offline-keys || return 1
  "$CLI" sign other.unsigned --out other.signed --key other.key || return 1
  step edge
  api "$CF/workers/scripts/$P-a/settings" | python3 -c '
import json,sys; b=json.load(sys.stdin)["result"]["bindings"]
print(next(x["text"] for x in b if x["name"]=="GRAFTYARD_SITES"))' > sites.json || return 1
  (cd "$ROOT" && node tests/m1/integ/deploy-edge.mjs "$W/sites.json" "$EDGE" --anchor "$W/install.toml") 2>&1 |
    grep -E "Uploaded|Version ID|ERROR" || return 1
  # A second edge whose anchor names the other key, so that genesis passes an edge's check and reaches the members,
  # whose own anchor is this install's: the group, not only the edge, must refuse it.
  (cd "$ROOT" && node tests/m1/integ/deploy-edge.mjs "$W/sites.json" "$EDGE2" --anchor "$W/other.toml") 2>&1 |
    grep -E "Uploaded|Version ID|ERROR" || return 1
  sleep 15
  step "1. another key's genesis to the empty group, through the edge with this install's anchor"
  post other.signed
  step "2. the same, through the edge that accepts it: the members must refuse"
  post other.signed "$EDGE2"
  step "3. the matching genesis"
  post genesis.signed
  step "4. another key's genesis again, through the edge that accepts it"
  post other.signed "$EDGE2"
  step "5. the matching genesis again: 409 naming its own digest, at the index it committed at"
  post genesis.signed
  echo; echo "run ok"
}

delete() {
  cd "$W" 2>/dev/null || true
  for s in $EDGE $EDGE2 $P-a $P-b $P-c $P-a-probe $P-b-probe $P-c-probe; do
    echo "delete script $s: $(api -X DELETE "$CF/workers/scripts/$s?force=true" | python3 -c 'import json,sys; r=json.load(sys.stdin); print(r["success"], [e["message"] for e in r["errors"]])')"
  done
  for id in $(api "$CF/containers/applications" | python3 -c '
import json,sys; print(" ".join(a["id"] for a in json.load(sys.stdin)["result"] if a["name"].startswith(sys.argv[1])))' "$P-"); do
    echo "delete container application $id: $(api -X DELETE "$CF/containers/applications/$id" | python3 -c 'import json,sys; r=json.load(sys.stdin); print(r.get("success"), r.get("errors"))')"
  done
  for b in $P-a-packs $P-b-packs $P-c-packs; do
    echo "delete bucket $b: $(api -X DELETE "$CF/r2/buckets/$b" | python3 -c 'import json,sys; r=json.load(sys.stdin); print(r["success"], [e["message"] for e in r["errors"]])')"
  done
  echo "left: scripts $(api "$CF/workers/scripts" | python3 -c 'import json,sys; print([s["id"] for s in json.load(sys.stdin)["result"] if s["id"].startswith(sys.argv[1])])' "$P"),"\
    "buckets $(api "$CF/r2/buckets?per_page=1000" | python3 -c 'import json,sys; print([b["name"] for b in json.load(sys.stdin)["result"]["buckets"] if b["name"].startswith(sys.argv[1])])' "$P")"
}

"${1:?run or delete}"
