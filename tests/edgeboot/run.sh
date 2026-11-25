#!/usr/bin/env bash
# The edge's own build answers in local workerd without a wasm trap: builds workers/edge (test-faults, or the product
# build with PRODUCT=1), serves it with the test install's vars and no bindings, and sends a REST read, a git
# advertisement and a git fetch. With no members bound every one answers an HTTP status (503 or 404); a trap answers 500
# with "memory access out of bounds" in workerd's log. mt16's first edge (c3952949, `beside`) overflowed wasm's 1 MiB
# stack on every request, which no native test, clippy or the edgebed (which builds only `graftyard_git`) saw.
# Exits 1 on any 500 or trap. Run from the repository root.
set -uo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd); P=${P:-8798}; W=$root/tests/m0/node_modules/.bin/wrangler
B=$(mktemp -d)
feat=(--features test-faults); [ -n "${PRODUCT:-}" ] && feat=()
cd "$root/workers/edge" && worker-build --release --out-dir "$B/out" "${feat[@]}" >"$B/build.log" 2>&1 \
  || { tail -20 "$B/build.log"; exit 1; }
python3 - "$B" "$root" <<'PY'
import json, sys
b, root = sys.argv[1], sys.argv[2]
# The test install's sites and genesis, as tests/m1/integ/deploy-edge.mjs writes them, with no bindings: every member,
# bucket and Cache API call fails, which is the cold path a new isolate takes before any read answers.
sites = json.load(open(f"{root}/tests/m3/finish/sites4.json"))
genesis = json.load(open(f"{root}/tests/m1/wp7/genesis.json"))["placements"]
json.dump({"name": "edgeboot", "main": f"{b}/out/index.js", "compatibility_date": "2026-10-01",
           "vars": {"PLAN": "pro", "SSH_OPTION": "off", "EDGE_CAP_BYTES": "80000000", "FORMATS_ENABLED": "32.1",
                    "PART_BYTES": "5242880", "HOST": "edgeboot.example", "GRAFTYARD_SITES": json.dumps(sites),
                    "GRAFTYARD_GENESIS": json.dumps(genesis), "LOGINS": "{}"}},
          open(f"{b}/wrangler.json", "w"))
open(f"{b}/.dev.vars", "w").write("BED_TOKEN=edgeboot-bed\n")
PY
"$W" dev --port $P -c "$B/wrangler.json" >"$B/dev.log" 2>&1 & DEV=$!
trap 'kill $DEV 2>/dev/null; wait $DEV 2>/dev/null; rm -rf "$B"' EXIT
for _ in $(seq 90); do grep -q "Ready on" "$B/dev.log" && break; sleep 1; done
ok=1
# The bed token and `x-graft-link` (test-faults) give each git request a cold isolate's link guess, so it takes
# `beside`'s path: the guessed resolve and the gate's rows read beside the front. The product build has neither, and
# takes a cold resolve with no guess.
hdr=(-H 'git-protocol: version=2')
[ -z "${PRODUCT:-}" ] && hdr+=(-H "x-bed-token: edgeboot-bed" -H "x-graft-link: 7,3")
# The Actions OIDC issuer's discovery document answers with no token, and its JWKS answers 503 with no site bound,
# never the run service's 401 (actions-oidc).
c=$(curl -s -m 30 -o "$B/body" -w '%{http_code}' "http://127.0.0.1:$P/_services/token/.well-known/openid-configuration")
echo "GET openid-configuration: $c $(head -c 100 "$B/body")"
{ [ "$c" = 200 ] && grep -q '"jwks_uri":"https://edgeboot.example/_services/token/.well-known/jwks"' "$B/body"; } || { echo "  FAIL: discovery"; ok=0; }
c=$(curl -s -m 30 -o "$B/body" -w '%{http_code}' "http://127.0.0.1:$P/_services/token/.well-known/jwks")
echo "GET jwks: $c"; [ "$c" = 503 ] || { echo "  FAIL: jwks with no site is not 503"; ok=0; }
for p in "/api/v3/repos/o/r" "/api/graftyard/v1/release/entries?after=0" "/o/r.git/info/refs?service=git-upload-pack" \
  "/o/r.git/git-upload-pack"; do
  m=GET; [ "${p##*/}" = git-upload-pack ] && m=POST
  c=$(curl -s -m 30 -X $m -D "$B/h" -o "$B/body" -w '%{http_code}' "${hdr[@]}" "http://127.0.0.1:$P$p")
  st=$(grep -i '^server-timing' "$B/h" | tr -d '\r')
  echo "$m $p: $c $(head -c 100 "$B/body" | tr -d '\n') | ${st:0:220}"
  { [ "$c" = 500 ] || [ "$c" = 000 ]; } && ok=0
  # A git request on test-faults must have taken the guess and started the rows beside the front.
  if [ -z "${PRODUCT:-}" ] && [ "${p#/o/r.git}" != "$p" ]; then
    grep -q 'resolve_guess;dur=2' <<<"$st" && grep -q 'beside_rows;dur=' <<<"$st" \
      || { echo "  FAIL: no cold-guess path (resolve_guess 2 and beside_rows)"; ok=0; }
  fi
done
# LM-3: the controller-count route stores nothing without a graftyard-agent App JWT: no token, or one that names no
# App, is 401 before any read (`release::errors`, `ident::graftyard_agent`).
for auth in "" "Bearer not.a.jwt"; do
  for m in POST GET; do
    c=$(curl -s -m 30 -X $m -o /dev/null -w '%{http_code}' ${auth:+-H "authorization: $auth"} \
      -H 'content-type: application/json' --data-binary '{"counts":{"versions":{},"threw":0,"unversioned":{"answers":0,"fivexx":0}}}' \
      "http://127.0.0.1:$P/api/graftyard/v1/release/errors?since=0")
    echo "$m /api/graftyard/v1/release/errors (${auth:-no token}): $c"
    [ "$c" = 401 ] || { echo "  FAIL: want 401"; ok=0; }
  done
done
sleep 1
n=$(grep -c 'out of bounds\|RuntimeError' "$B/dev.log")
echo "wasm traps: $n"; [ "$n" = 0 ] || ok=0
[ $ok = 1 ] && echo PASS || { echo FAIL; exit 1; }
