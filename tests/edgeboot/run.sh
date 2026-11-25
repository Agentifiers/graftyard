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
# actions-artifacts' gzip check (below): a local R2 bucket the edge's `/_bed/gzip` stores into, and a front that reaches
# the edge over a service binding and passes its answer on as graft-front does (`workers/front/src/lib.rs`, `fetch`).
w = json.load(open(f"{b}/wrangler.json"))
w["r2_buckets"] = [{"binding": "BED_ARTIFACTS", "bucket_name": "edgeboot-artifacts"}]
json.dump(w, open(f"{b}/wrangler.json", "w"))
open(f"{b}/front.js", "w").write(
    "export default { async fetch(req, env) { const r = await env.EDGE.fetch(req);"
    " return new Response(r.body, { status: r.status, headers: r.headers }); } }\n")
json.dump({"name": "edgeboot-front", "main": f"{b}/front.js", "compatibility_date": "2026-10-01",
           "services": [{"binding": "EDGE", "service": "edgeboot"}]}, open(f"{b}/front.json", "w"))
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
# actions-artifacts: a stored gzip body leaves the edge byte for byte with its Content-Length, through a front's
# service binding, as a gzipped artifact file must reach `download-artifact`, which gunzips it once. With workerd's
# default `encodeBody` the edge's answer was gzipped again: 307,964 B stored, 308,077 B chunked (`decisions/DECISIONS.md`,
# "`actions-artifacts`"). Test builds only: the route is `/_bed/gzip`.
if [ -z "${PRODUCT:-}" ]; then
  FP=$((P + 1))
  "$W" dev --port $FP -c "$B/front.json" -c "$B/wrangler.json" >"$B/front.log" 2>&1 & FRONT=$!
  trap 'kill $DEV $FRONT 2>/dev/null; wait $DEV $FRONT 2>/dev/null; rm -rf "$B"' EXIT
  for _ in $(seq 90); do grep -q "Ready on" "$B/front.log" && break; sleep 1; done
  head -c 300000 /dev/urandom | base64 >"$B/plain" && gzip -c "$B/plain" >"$B/stored.gz"
  want=$(stat -c %s "$B/stored.gz")
  c=$(curl -s -m 30 -o /dev/null -w '%{http_code}' -X PUT -H 'x-bed-token: edgeboot-bed' -H 'content-encoding: gzip' \
    --data-binary @"$B/stored.gz" "http://127.0.0.1:$FP/_bed/gzip")
  c2=$(curl -s -m 30 -D "$B/gh" -o "$B/got.gz" -w '%{http_code}' -H 'x-bed-token: edgeboot-bed' \
    -H 'accept-encoding: gzip' "http://127.0.0.1:$FP/_bed/gzip")
  len=$(grep -i '^content-length:' "$B/gh" | tr -dc 0-9)
  got=$(stat -c %s "$B/got.gz")
  echo "gzip through the front: put $c, get $c2, stored $want B, answered $got B, content-length ${len:-none}"
  { [ "$c" = 204 ] && [ "$c2" = 200 ] && cmp -s "$B/got.gz" "$B/stored.gz" && [ "$len" = "$want" ]; } \
    || { echo "  FAIL: the stored gzip did not leave the edge as stored"; ok=0; }
  # F131's REST half on the edge's own wasm: two finalized artifacts (one file gzipped as the client stores it, one
  # plain beside an empty file), each zip read back by Python's zipfile against the files' SHA-256, a one-day
  # retention, and a delete that removes one artifact's objects and row and no other's.
  head -c 250000 /dev/urandom >"$B/bin" && : >"$B/empty"
  B="$B" FP="$FP" python3 - <<'PY' || ok=0
import gzip, hashlib, io, json, os, urllib.request, zipfile
b, port = os.environ["B"], os.environ["FP"]
base = f"http://127.0.0.1:{port}/_bed/artifacts"
def call(method, q="", body=None, h=None):
    r = urllib.request.Request(f"{base}{q}", data=body, method=method, headers={"x-bed-token": "edgeboot-bed", **(h or {})})
    with urllib.request.urlopen(r, timeout=60) as a:
        return a.read()
plain, binary, empty = (open(f"{b}/{n}", "rb").read() for n in ("plain", "bin", "empty"))
z = gzip.compress(plain)
a1 = json.loads(call("PUT", "?path=one/dir/text.txt&days=1", z, {"content-encoding": "gzip", "x-file-length": str(len(plain))}))["id"]
a2 = json.loads(call("PUT", "?path=two/bin.dat", binary, {"x-file-length": str(len(binary))}))["id"]
call("PUT", "?path=two/sub/empty", empty, {"x-file-length": "0"})
rows = {r["name"]: r for r in json.loads(call("GET"))}
fail = []
day = rows["one"]["expires"] - rows["one"]["created"]
if day != 86400000 or rows["two"]["expires"] - rows["two"]["created"] != 90 * 86400000:
    fail.append(f"retention: one {day} ms, two {rows['two']['expires'] - rows['two']['created']} ms")
sha = lambda x: hashlib.sha256(x).hexdigest()
for aid, want in ((a1, {"dir/text.txt": plain}), (a2, {"bin.dat": binary, "sub/empty": empty})):
    zf = zipfile.ZipFile(io.BytesIO(call("GET", f"?zip={aid}")))
    if zf.testzip() is not None:
        fail.append(f"zip {aid}: bad CRC")
    got = {i.filename: sha(zf.read(i)) for i in zf.infolist()}
    if got != {k: sha(v) for k, v in want.items()}:
        fail.append(f"zip {aid}: {sorted(got)}")
    print(f"zip {aid}: {sorted(got)} {[i.compress_type for i in zf.infolist()]}")
call("DELETE", f"?id={a1}")
keys = json.loads(call("GET", "?keys=1"))
left = sorted({r["name"] for r in json.loads(call("GET"))})
if any("/one" in k or k.endswith(f"/a/{a1}") for k in keys) or left != ["two"] or not any("/two/" in k for k in keys):
    fail.append(f"delete: left {left}, keys {keys}")
print(f"rest artifacts: {len(keys)} objects after the delete, rows {left}")
print("  FAIL: " + "; ".join(fail) if fail else "rest artifacts ok")
raise SystemExit(1 if fail else 0)
PY
  n=$(grep -c 'out of bounds\|RuntimeError' "$B/front.log"); [ "$n" = 0 ] || ok=0
fi
sleep 1
n=$(grep -c 'out of bounds\|RuntimeError' "$B/dev.log")
echo "wasm traps: $n"; [ "$n" = 0 ] || ok=0
[ $ok = 1 ] && echo PASS || { echo FAIL; exit 1; }
