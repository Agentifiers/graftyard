#!/usr/bin/env bash
# Step 4's end-to-end proof on a fresh throwaway install, graft-m1-gen-{a,b,c} plus the edge graft-m1-gen-edge, all
# product builds (no test-faults): `graftyard init`, `sites add` x3, `genesis`, `sign`, an edge with the install's
# anchor, `install --genesis` twice (201, then the 409 replay), a tampered genesis (refused locally) and the same
# tampered bytes posted straight to the edge (422), then a read of the system groups through the edge's normal path
# (a git request for an owner that does not exist reads the install's `Names` login shard). Then everything is
# deleted. Usage: throwaway.sh run | throwaway.sh delete. Work files under $W (~/.graft-gen).
set -uo pipefail
export PATH=$HOME/.cargo/bin:$PATH
ROOT=/home/user/workspace/repo
W=$HOME/.graft-gen
CLI=$ROOT/target/release/graftyard
EDGE=graft-m1-gen-edge
# The git cache shard's image, which `sites add` deploys as each site's `GitShard` container (the test install's).
IMAGE=${IMAGE:-registry.cloudflare.com/<cf-account-id>/graft-m1-wp6-bed-gitshard-git@sha256:b7bbc1f6644c428a7f27495fe676fa3d6772b02c73030ffcab3039b56e46b43e}
CF=https://api.cloudflare.com/client/v4/accounts/$CLOUDFLARE_ACCOUNT_ID
api() { curl -s -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" "$@"; }
step() { echo; echo "=== $* ($(date -u +%T))"; }

run() {
  mkdir -p "$W" && cd "$W" || exit 2
  if [ ! -f customer.key ]; then
    openssl genpkey -algorithm ed25519 -out customer.key && openssl pkey -in customer.key -pubout -out customer.pub
    openssl genpkey -algorithm ed25519 -out release.key && openssl pkey -in release.key -pubout -out release.pub
  fi
  step init
  [ -f install.toml ] || "$CLI" init --account "$CLOUDFLARE_ACCOUNT_ID" --zone git.m1gen.example --customer-key customer.pub || return 1
  step sites add
  for s in a:enam b:weur c:apac; do
    "$CLI" sites add --script "graft-m1-gen-${s%%:*}" --region "${s#*:}" --bundle /tmp/site-prod --image "$IMAGE" || return 1
  done
  step genesis
  [ -f genesis.unsigned ] || "$CLI" genesis --out genesis.unsigned --release-key release.pub || return 1
  step sign
  [ -f genesis.signed ] || "$CLI" sign genesis.unsigned --out genesis.signed --key customer.key || return 1
  step edge
  api "$CF/workers/scripts/graft-m1-gen-a/settings" | python3 -c '
import json,sys; b=json.load(sys.stdin)["result"]["bindings"]
print(next(x["text"] for x in b if x["name"]=="GRAFTYARD_SITES"))' > sites.json || return 1
  (cd "$ROOT" && node tests/m1/integ/deploy-edge.mjs "$W/sites.json" "$EDGE" --anchor "$W/install.toml") 2>&1 |
    grep -E "Uploaded|Version ID|ERROR|anchor" || return 1
  sleep 10
  URL=https://$EDGE.<workers-subdomain>.workers.dev
  step "install (tampered: refused locally)"
  python3 -c 'import sys; b=bytearray(open("genesis.signed","rb").read()); b[100]^=1; open("tampered.signed","wb").write(b)'
  "$CLI" install --genesis tampered.signed --edge "$URL"
  step "tampered bytes straight to the edge"
  curl -s -w ' HTTP %{http_code}\n' -X POST -H 'content-type: application/octet-stream' --data-binary @tampered.signed \
    "$URL/api/graftyard/v1/release/entries"
  step "install"
  "$CLI" install --genesis genesis.signed --edge "$URL" || return 1
  step "install again (replay)"
  "$CLI" install --genesis genesis.signed --edge "$URL"
  step "the endpoint's own answer to the replay"
  curl -s -w ' HTTP %{http_code}\n' -X POST -H 'content-type: application/octet-stream' --data-binary @genesis.signed \
    "$URL/api/graftyard/v1/release/entries" | head -c 3000
  step "a git request through the system groups (no such owner: 404 after a Names read)"
  curl -s -A git/2.56 -w ' HTTP %{http_code}\n' "$URL/nobody/x.git/info/refs?service=git-upload-pack"
  step "each site's GitShard: its own binding, container, migration, cron, and container application"
  for s in a b c; do
    api "$CF/workers/scripts/graft-m1-gen-$s/settings" | python3 -c '
import json,sys; b=json.load(sys.stdin)["result"]["bindings"]
print(sys.argv[1], [(x["name"], x.get("script_name"), x["namespace_id"]) for x in b if x.get("class_name")=="GitShard"])' "graft-m1-gen-$s" | tee -a shard.txt
    ns=$(tail -1 shard.txt | grep -oE "[0-9a-f]{32}" | head -1)
    api "$CF/containers/applications/$ns" | python3 -c 'import json,sys; r=json.load(sys.stdin)["result"]; print(" app", r["name"], r["scheduling_policy"], r["durable_objects"])'
    api "$CF/workers/scripts/graft-m1-gen-$s/schedules" | python3 -c 'import json,sys; print(" crons", [x["cron"] for x in json.load(sys.stdin)["result"]["schedules"]])'
  done
  api "$CF/workers/scripts" | python3 -c '
import json,sys; print([(s["id"], s.get("migration_tag")) for s in json.load(sys.stdin)["result"] if s["id"].startswith("graft-m1-gen-") and s["id"] != "graft-m1-gen-edge"])'
  step "a clone through the gen install's own GitShard path (no such owner, so 404 after the Names read)"
  curl -s -A git/2.56 -w ' HTTP %{http_code}\n' "$URL/nobody/y.git/info/refs?service=git-upload-pack"
  step "settings: no test-only variable on the product scripts"
  for s in a b c; do
    api "$CF/workers/scripts/graft-m1-gen-$s/settings" | python3 -c '
import json,sys; print(sys.argv[1], sorted(x["name"] for x in json.load(sys.stdin)["result"]["bindings"]))' "graft-m1-gen-$s"
  done
  echo; echo "run ok"
}

delete() {
  cd "$W" 2>/dev/null || true
  for s in $EDGE graft-m1-gen-a graft-m1-gen-b graft-m1-gen-c graft-m1-gen-a-probe graft-m1-gen-b-probe graft-m1-gen-c-probe; do
    echo "delete script $s: $(api -X DELETE "$CF/workers/scripts/$s?force=true" | python3 -c 'import json,sys; r=json.load(sys.stdin); print(r["success"], [e["message"] for e in r["errors"]])')"
  done
  # The GitShard container applications, by name (`sites add` names them <script>-gitshard).
  for id in $(api "$CF/containers/applications" | python3 -c '
import json,sys; print(" ".join(a["id"] for a in json.load(sys.stdin)["result"] if a["name"].startswith("graft-m1-gen-")))'); do
    echo "delete container application $id: $(api -X DELETE "$CF/containers/applications/$id" | python3 -c 'import json,sys; r=json.load(sys.stdin); print(r.get("success"), r.get("errors"))')"
  done
  for b in graft-m1-gen-a-packs graft-m1-gen-b-packs graft-m1-gen-c-packs; do
    echo "delete bucket $b: $(api -X DELETE "$CF/r2/buckets/$b" | python3 -c 'import json,sys; r=json.load(sys.stdin); print(r["success"], [e["message"] for e in r["errors"]])')"
  done
  # Only this install's store, `graft-ops-<install id>` (cli/src/genesis.rs).
  inst=$(sed -n 's/^install *= *"\(.*\)"/\1/p' install.toml 2>/dev/null | tr A-Z a-z)
  for id in $(api "$CF/secrets_store/stores" | python3 -c '
import json,sys; print(" ".join(s["id"] for s in json.load(sys.stdin)["result"] if s["name"] == "graft-ops-" + sys.argv[1]))' "$inst"); do
    # A store with secrets in it is refused (`store_not_empty`): its secrets go first.
    for sid in $(api "$CF/secrets_store/stores/$id/secrets" | python3 -c 'import json,sys; print(" ".join(s["id"] for s in json.load(sys.stdin)["result"]))'); do
      echo "delete secret $sid: $(api -X DELETE "$CF/secrets_store/stores/$id/secrets/$sid" | python3 -c 'import json,sys; r=json.load(sys.stdin); print(r["success"], [e["message"] for e in r["errors"]])')"
    done
    echo "delete secrets store $id: $(api -X DELETE "$CF/secrets_store/stores/$id" | python3 -c 'import json,sys; r=json.load(sys.stdin); print(r["success"], [e["message"] for e in r["errors"]])')"
  done
  echo "left: scripts $(api "$CF/workers/scripts" | python3 -c 'import json,sys; print([s["id"] for s in json.load(sys.stdin)["result"] if s["id"].startswith("graft-m1-gen")])')," \
    "buckets $(api "$CF/r2/buckets?per_page=1000" | python3 -c 'import json,sys; print([b["name"] for b in json.load(sys.stdin)["result"]["buckets"] if b["name"].startswith("graft-m1-gen")])')"
}

"${1:?run or delete}"
