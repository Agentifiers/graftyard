#!/usr/bin/env bash
# The lifeboat bundle end to end in local workerd, at $0 (`decisions/DECISIONS.md`, "`lifeboat-bundle`"; BF-53's and
# BM-6's lines, `build/BUILD-PLAN.md`, M6-WP3). Two sides:
#
# - **The install**, under wrangler: three site scripts of the site's own build (test-faults, exportbed's driver in
#   front, `GRAFTYARD_EXPORT=1`, site 0 binding the export bucket) and the edge's own build (test-faults), both with
#   this install's trust anchor and no `GRAFTYARD_GENESIS` stand-in, so every system record comes from a real genesis
#   the CLI made, signed, and posted. `front.mjs` stands in for the internet: R2's S3 API over the export bucket (SigV4
#   checked), and switches that stop the feed or block the edge.
# - **The lifeboat**, under raw workerd from `graftyard lifeboat bundle`: the site build as the copy's members, the edge
#   build as the read-only front (HTTPS, a local CA's certificate), and the puller; `graft-blob` beside it.
#
# Steps: genesis, a `Keys` with executor and drain seeds the bed holds, a `ReleaseUploaded` (the release the decoder
# reports), `graftyard lifeboat init`, the seed (`seed.mjs`: an owner, a repository, a PAT, two pushes with stored
# packs), a pull request through GraphQL, then `graftyard lifeboat enroll`. Then checks:
#   1. the copy learns the feed: the repository's copy at its leader's commit, both packs in graft-blob;
#   2. the token: the main edge refuses the lifeboat token (401), the lifeboat refuses the PAT (401) and a `Stored`
#      row's hash is no lifeboat credential, writes and mutations answer 503;
#   3. the decoder: the `Release` group names the lifeboat's release and a fresh `seen`, so no 7-day alarm;
#   4. the feed stopped: a third push reaches the copy through signed tail and pack pulls once the feed is stale, and
#      no leader of the repository or of any chain pulled took a `/log_rows`;
#   5. the edge blocked: enrolled `gh pr list` and `gh api` read the pull request from the lifeboat, with the lag
#      published in `X-Graftyard-Lifeboat-Lag` and `/api/v3/meta`;
#   6. a planted undecodable row halts its chain, logged as an error, and the other chains stay current;
#   and workerd logs no wasm trap on either side.
# Needs worker-build, tests/m0's wrangler and workerd, gh, openssl, python3 with cryptography, and `sudo -n` to add
# two loopback names to /etc/hosts (removed on exit). PREBUILT=<dir> takes site/ and edge/ (test-faults) and
# lifeboat/ builds from <dir>. KEEP=1 leaves both sides up. Run from the repository root.
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd); root=$(cd "$here/../.." && pwd)
P=${P:-8797}; LP=${LP:-8444}; BP=${BP:-8790}; AP=${AP:-8791}
W=${WRANGLER:-$root/tests/m0/node_modules/.bin/wrangler}; WD=$root/tests/m0/node_modules/.bin/workerd
[ -x "$W" ] && [ -x "$WD" ] || { echo "no wrangler or workerd under tests/m0 (npm ci in tests/m0)"; exit 1; }
MAIN=git.lbbed.test; BOAT=lifeboat.lbbed.test
B=${B:-$(mktemp -d)}; mkdir -p "$B/k" "$B/boat" "$B/blob"; chmod 700 "$B/k" "$B/boat"
cp "$here/front.mjs" "$B/"; cp "$root/tests/exportbed/driver.mjs" "$B/"
build() { (cd "$root/workers/$1" && worker-build --release --out-dir "$B/builds/$1" "${@:2}") >"$B/build-$1.log" 2>&1 \
  || { tail -20 "$B/build-$1.log"; exit 1; }; }
if [ -n "${PREBUILT:-}" ]; then
  mkdir -p "$B/builds"; cp -r "$PREBUILT/site" "$PREBUILT/edge" "$PREBUILT/lifeboat" "$B/builds/"
else
  build site --features test-faults; build edge --features test-faults; build lifeboat
fi
ln -sfn "$B/builds/site" "$B/out"   # driver.mjs imports ./out/index.js
cd "$root" || exit 1
cargo build -q -p graftyard -p graftyard-lifeboat --bin graftyard --bin graft-blob || exit 1
CLI=$root/target/debug/graftyard
C=(--config "$B/k/install.toml")
F=http://127.0.0.1:$P

# Loopback names: the edge serves its API only on its own host, and gh only over HTTPS to a named host. No proxy for
# them: an orb's HTTPS proxy refuses a tunnel to a loopback name.
NO_PROXY="${NO_PROXY:-${no_proxy:-}}"; NO_PROXY="${NO_PROXY:+$NO_PROXY,}localhost,127.0.0.1,.lbbed.test"
export NO_PROXY no_proxy="$NO_PROXY"
for n in $MAIN $BOAT; do
  getent hosts $n >/dev/null || echo "127.0.0.1 $n # lifeboatbed" | sudo -n tee -a /etc/hosts >/dev/null \
    || { echo "cannot add $n to /etc/hosts (sudo -n)"; exit 1; }
done

# ---- the install: keys, install.toml, the genesis ----
K=$B/k
seed() { python3 -c 'import os; print(os.urandom(32).hex())' >"$1"; chmod 600 "$1"; }
pub() { python3 - "$1" <<'EOF'
import sys
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from cryptography.hazmat.primitives import serialization
k = Ed25519PrivateKey.from_private_bytes(bytes.fromhex(open(sys.argv[1]).read().strip()))
print(k.public_key().public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw).hex())
EOF
}
for k in customer release; do
  openssl genpkey -algorithm ed25519 -out "$K/$k.key" 2>/dev/null
  openssl pkey -in "$K/$k.key" -pubout -out "$K/$k.pub" 2>/dev/null
done
seed "$K/executor.seed"; seed "$K/drain.seed"
"$CLI" "${C[@]}" init --account bed --zone $MAIN --customer-key "$K/customer.pub" >/dev/null || exit 1
python3 - "$K/install.toml" <<'PY'
import re, sys
p = sys.argv[1]
t = "".join(l for l in open(p).read().splitlines(True) if l.strip() not in ("sites = []", "rtt = []"))
# Three sites in enam, weur, and apac as `sites add` would record them, with measured round trips for `Tri`.
for i, r in enumerate(["enam", "weur", "apac"]):
    t += f'\n[[sites]]\nid = {i}\nscript = "lbbed-{i}"\naccount = "bed"\nregion = "{r}"\nbucket = "lbbed-{i}"\nspare = false\n\n[sites.link.binding]\nmember_class = "Member"\n'
for a, b, ms in [(0, 1, 98), (0, 2, 194), (1, 2, 187)]:
    t += f"\n[[rtt]]\nfrom = {a}\npeer = {b}\nmedian_ms = {ms}\nat = 1\n"
open(p, "w").write(t)
PY
"$CLI" "${C[@]}" genesis --out "$K/genesis.unsigned" --release-key "$K/release.pub" --offline-keys >/dev/null || exit 1
"$CLI" sign "$K/genesis.unsigned" --out "$K/genesis.signed" --key "$K/customer.key" >/dev/null || exit 1
read -r INSTALL CUSTOMER < <(python3 -c '
import re, sys; t = open(sys.argv[1]).read()
print(*(re.search(rf"^{k}\s*=\s*\"([^\"]*)\"", t, re.M).group(1) for k in ("install", "customer_key")))' "$K/install.toml")

# ---- the install's scripts ----
python3 - "$B" "$K/install.toml" "$P" "$INSTALL" "$CUSTOMER" "$MAIN" <<'PY'
import json, sys, tomllib
b, toml, port, install, customer, main = sys.argv[1:7]
t = tomllib.load(open(toml, "rb"))
sites = [{"id": s["id"], "script": s["script"], "account": s["account"], "region": s["region"], "jurisdiction": None,
          "bucket": s["bucket"], "release": None, "link": s["link"], "spare": False} for s in t["sites"]]
classes = [("MEMBER", "Member"), ("REGISTRY", "Registry"), ("SHARD", "GitShard"), ("HOOKSEQ", "HookSequencer"),
           ("SEARCH", "SearchShard"), ("INBOX", "Inbox"), ("LOGRELAY", "LogRelay"), ("BROKER", "RunnerBroker")]
buckets = [{"binding": f"BUCKET_{s['id']}", "bucket_name": s["bucket"]} for s in sites]
export = {"binding": "EXPORT", "bucket_name": "lbbed-export"}
anchor = {"GRAFTYARD_INSTALL": install, "GRAFTYARD_CUSTOMER_KEY": customer}
for s in sites:
    me = s["id"]
    json.dump({
        "name": s["script"], "main": f"{b}/driver.mjs", "compatibility_date": "2026-10-01",
        "vars": {"SITE_ID": str(me), "GRAFTYARD_SITES": json.dumps(sites), "GRAFTYARD_EXPORT": "1", **anchor},
        "durable_objects": {"bindings": [
            {"name": f"{n}_{x['id']}", "class_name": c, **({} if x["id"] == me else {"script_name": x["script"]})}
            for n, c in classes for x in sites]},
        "r2_buckets": buckets + ([export] if me == 0 else []),
        "queues": {"producers": [{"binding": "PRSYNC", "queue": f"lbbed-q{me}"}],
                   "consumers": [{"queue": f"lbbed-q{me}", "max_batch_size": 10, "max_batch_timeout": 0}]},
        "migrations": [{"tag": "v1", "new_sqlite_classes": [c for _, c in classes]}],
    }, open(f"{b}/site{me}.json", "w"))
# 17.5: Tokens 0x11.5, which `POST /api/graftyard/v1/lifeboat/tokens` needs; the rest as coldwrite's edge.
formats = "32.1,32.4,32.5,32.6,32.7,32.9,16.2,17.2,18.2,20.1,32.10,18.4,32.8,32.12,32.14,18.3,17.5"
json.dump({"name": "lbbed-edge", "main": f"{b}/builds/edge/index.js", "compatibility_date": "2026-10-01",
           "vars": {"PLAN": "pro", "SSH_OPTION": "off", "EDGE_CAP_BYTES": "80000000", "FORMATS_ENABLED": formats,
                    "PART_BYTES": "5242880", "HOST": main, "GRAFTYARD_SITES": json.dumps(sites), "LOGINS": "{}",
                    **anchor},
           "durable_objects": {"bindings": [{"name": f"{n}_{x['id']}", "class_name": c, "script_name": x["script"]}
                                            for n, c in classes[:3] for x in sites]},
           "r2_buckets": buckets + [export]},
          open(f"{b}/builds/edge/wrangler.json", "w"))
json.dump({"name": "lbbed-front", "main": f"{b}/front.mjs", "compatibility_date": "2026-10-01",
           "vars": {"FEED_BUCKET": "lbbed-export", "FEED_KEY_ID": "lbbed-key"},
           "services": [{"binding": "EDGE", "service": "lbbed-edge"}, {"binding": "SITE", "service": "lbbed-0"}],
           "durable_objects": {"bindings": [{"name": f"MEMBER_{x['id']}", "class_name": "Member",
                                             "script_name": x["script"]} for x in sites]},
           "r2_buckets": [export]},
          open(f"{b}/front.json", "w"))
open(f"{b}/sites.json", "w").write(json.dumps(sites))
PY
FEED_SECRET=$(python3 -c 'import os; print(os.urandom(20).hex())')
printf 'FEED_SECRET=%s\n' "$FEED_SECRET" >"$B/.dev.vars"
printf 'BED_TOKEN=lbbed-bed\n' >"$B/builds/edge/.dev.vars"
cd "$B" && { setsid "$W" dev --port $P -c front.json -c builds/edge/wrangler.json -c site0.json -c site1.json -c site2.json \
  --persist-to "$B/state" >"$B/dev.log" 2>&1 & DEV=$!; cd "$root"; }
WDP=""; BLOBP=""
cleanup() {
  s=$?
  [ -n "${KEEP:-}" ] && { echo "KEEP: install pid $DEV, workerd $WDP, graft-blob $BLOBP in $B"; return; }
  kill -- -$DEV 2>/dev/null; [ -n "$WDP" ] && kill $WDP 2>/dev/null; [ -n "$BLOBP" ] && kill $BLOBP 2>/dev/null
  wait 2>/dev/null
  sudo -n sed -i '/# lifeboatbed$/d' /etc/hosts
  rm -rf "$B"; exit $s
}
trap cleanup EXIT
for _ in $(seq 120); do grep -q "Ready on" "$B/dev.log" && break; sleep 1; done
grep -q "Ready on" "$B/dev.log" || { tail -30 "$B/dev.log"; exit 1; }
u() { curl -s -m 120 "$@"; }
must() { echo "$2" | grep -q "\"status\": *$1" || { echo "FAIL: expected $1: $2"; exit 1; }; }
field() { python3 -c 'import json,sys; print(json.loads(sys.argv[1]).get(sys.argv[2]) or "")' "$1" "$2"; }

echo "== genesis, Keys, ReleaseUploaded"
a=$("$CLI" "${C[@]}" release post "$K/genesis.signed" --edge "$F"); echo "genesis: $a"; must 201 "$a"
PREV=$(field "$a" digest)
python3 - "$K/install.toml" "$(sha256sum "$K/genesis.signed" | cut -c1-64)" <<'PY'
import sys
p, d = sys.argv[1], sys.argv[2]
t = open(p).read()
i = t.index("\n[[sites]]")
open(p, "w").write(t[:i] + f'\ngenesis = "{d}"\n' + t[i:])
PY
entry() { echo "$1" >"$K/spec.json"; "$CLI" "${C[@]}" release entry "$K/spec.json" --out "$K/$2.unsigned" --prev "$PREV" "${@:4}" >/dev/null \
  && "$CLI" sign "$K/$2.unsigned" --out "$K/$2.signed" --key "$3" >/dev/null \
  && a=$("$CLI" "${C[@]}" release post "$K/$2.signed" --edge "$F") && echo "$2: $a" && must 201 "$a" && PREV=$(field "$a" digest); }
entry "{\"Keys\": {\"executor\": \"$(pub "$K/executor.seed")\", \"drain\": \"$(pub "$K/drain.seed")\"}}" keys1 \
  "$K/customer.key" --keys "$K/genesis.signed" || exit 1
READS='["17.5", "32.1", "32.4", "32.5", "32.6", "32.7"]'
python3 - "$K" "$READS" <<'PY'
import hashlib, json, sys
k, reads = sys.argv[1], json.loads(sys.argv[2])
text = "".join(f"{f}\n" for f in reads)
rec = {"label": "lifeboatbed", "files": [{"name": "formats.txt", "sha256": hashlib.sha256(text.encode()).hexdigest()}]}
open(f"{k}/release.json", "wb").write(json.dumps(rec, sort_keys=True).encode())
PY
openssl pkeyutl -sign -rawin -inkey "$K/release.key" -in "$K/release.json" -out "$K/release.sig" || exit 1
REL=$(sha256sum "$K/release.json" | cut -c1-64)
entry "{\"ReleaseUploaded\": {\"record\": \"$K/release.json\", \"record_sig\": \"$(od -An -tx1 -v "$K/release.sig" | tr -d ' \n')\",
  \"versions\": [[\"Edge\", \"00000000-0000-4000-8000-00000000001b\"]], \"reads\": $READS}}" upload "$K/executor.seed" || exit 1

echo "== lifeboat init and the bundle"
"$CLI" "${C[@]}" lifeboat init --host $BOAT --key "$K/customer.key" --dir "$B/boat" --edge "$F" \
  --genesis "$K/genesis.signed" || exit 1
printf '{"FEED_URL": "http://127.0.0.1:%s/_s3", "FEED_BUCKET": "lbbed-export", "FEED_KEY_ID": "lbbed-key", "FEED_SECRET": "%s"}\n' \
  "$P" "$FEED_SECRET" >"$B/boat/feed.json"; chmod 600 "$B/boat/feed.json"
# A local CA and the lifeboat's certificate, as the host's ACME client would hold one (real ACME: R50).
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 2 -subj /CN=lbbed-ca \
  -keyout "$K/ca.key" -out "$K/ca.pem" 2>/dev/null
openssl req -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -subj /CN=$BOAT -keyout "$K/tls.key" -out "$K/tls.csr" 2>/dev/null
printf 'subjectAltName=DNS:%s\n' $BOAT >"$K/san.ext"
openssl x509 -req -in "$K/tls.csr" -CA "$K/ca.pem" -CAkey "$K/ca.key" -CAcreateserial -days 2 -extfile "$K/san.ext" \
  -out "$K/tls.pem" 2>/dev/null || exit 1
"$root/target/debug/graft-blob" --root "$B/blob" --listen 127.0.0.1:$BP >"$B/blob.log" 2>&1 & BLOBP=$!
"$CLI" lifeboat bundle --dir "$B/boat" --builds "$B/builds" --release "$REL" --listen 127.0.0.1:$LP \
  --tls-key "$K/tls.key" --tls-chain "$K/tls.pem" --blob http://127.0.0.1:$BP --admin 127.0.0.1:$AP \
  --state "$B/boat/state" --out "$B/boat/config.capnp" || exit 1
# Through the start script the bundle wrote, as an operator starts it: it raises the open-file limit or refuses.
[ -x "$B/boat/start.sh" ] || { echo "FAIL: lifeboat bundle wrote no start.sh"; exit 1; }
{ WORKERD=$WD "$B/boat/start.sh" >"$B/workerd.log" 2>&1 & WDP=$!; }
for _ in $(seq 60); do curl -s -m 2 -o /dev/null http://127.0.0.1:$AP/status && break; sleep 0.5; done
L=https://$BOAT:$LP
lb() { curl -s -m 60 --cacert "$K/ca.pem" "$@"; }
admin() { curl -s -m 120 "http://127.0.0.1:$AP$1" "${@:2}"; }

echo "== seed"
node "$here/seed.mjs" "$F" lbbed-bed "$K" || { tail -30 "$B/dev.log"; exit 1; }
read -r LOGIN REPO RID OWNER < <(python3 -c 'import json,sys; s=json.load(open(sys.argv[1])); print(s["login"], s["repo"], s["id"], s["owner"])' "$K/seed.json")
PAT=$(cat "$K/pat")
# The pull request, as `gh pr create` sends it: GraphQL `createPullRequest` on the main host.
node_id=$(u -H "host: $MAIN" -H @<(printf 'authorization: token %s\n' "$PAT") -H 'user-agent: lifeboatbed' \
  "$F/api/v3/repos/$LOGIN/$REPO" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("node_id",""))')
[ -n "$node_id" ] || { echo "FAIL: no repository node id from the main host"; exit 1; }
pr=$(u -H "host: $MAIN" -H @<(printf 'authorization: token %s\n' "$PAT") -H 'user-agent: lifeboatbed' -X POST "$F/api/graphql" \
  -d "{\"query\": \"mutation(\$i: CreatePullRequestInput!) { createPullRequest(input: \$i) { pullRequest { number } } }\",
       \"variables\": {\"i\": {\"repositoryId\": \"$node_id\", \"baseRefName\": \"main\", \"headRefName\": \"f\", \"title\": \"lifeboat pr\", \"body\": \"read me from the lifeboat\"}}}")
echo "pull request: $pr"
echo "$pr" | grep -q '"number"' || { echo "FAIL: createPullRequest"; exit 1; }

echo "== enroll"
"$CLI" lifeboat enroll --host $BOAT --edge "$F" --token-file "$K/pat" --gh-config "$K/gh" --out "$K/lifeboat.token" || exit 1
LBT=$(cat "$K/lifeboat.token")

ok=1
pass() { echo "ok    $*"; }
fail() { echo "FAIL  $*"; ok=0; }
kick() { u "$F/_sitecron" >/dev/null; }
status() { admin /status; }
# Waits up to $2 s, kicking feed rounds, until the python predicate $1 holds on the puller's status.
until_status() {
  local t0=$SECONDS
  while [ $((SECONDS - t0)) -lt "$2" ]; do
    kick; sleep 3
    status | python3 -c "import json,sys; s=json.load(sys.stdin); sys.exit(0 if ($1) else 1)" 2>/dev/null && return 0
  done
  return 1
}
held() { status | python3 -c "import json,sys; print(json.load(sys.stdin)['held'].get(sys.argv[1], 0))" "$1"; }
commit_of() { u "$F/_stats?chain=$1" | python3 -c 'import json,sys; print(max((m["commit"] or 0) for m in json.load(sys.stdin)))'; }

echo "== 1. the copy learns the feed"
RC=$(commit_of "repo:$RID")
if until_status "s['held'].get('repo:$RID', 0) >= $RC and s['packs'] >= 2 and s['owed'] == 0" 240; then
  pass "repo:$RID held at its leader's commit $RC, 2 packs kept, none owed"
else fail "the copy did not learn the feed: $(status)"; fi
python3 - "$K/seed.json" "$B/blob" <<'PY' && pass "graft-blob holds both packs with their entries' SHA-256" || fail "graft-blob"
import hashlib, json, os, sys
s = json.load(open(sys.argv[1]))
ok = True
for p in s["packs"]:
    st = p["stored"]
    f = os.path.join(sys.argv[2], f"pack/{s['id']}/{st['id']}")
    ok &= os.path.exists(f) and hashlib.sha256(open(f, "rb").read()).hexdigest() == bytes(st["checksum"]).hex()
sys.exit(0 if ok else 1)
PY

echo "== 2. tokens and writes"
c=$(u -o /dev/null -w '%{http_code}' -H "host: $MAIN" -H @<(printf 'authorization: token %s\n' "$LBT") -H 'user-agent: lifeboatbed' "$F/api/v3/user")
[ "$c" = 401 ] && pass "the main edge refuses the lifeboat token (401)" || fail "the main edge answered $c to the lifeboat token"
c=$(u -o /dev/null -w '%{http_code}' -H "host: $MAIN" -H @<(printf 'authorization: token %s\n' "$PAT") -H 'user-agent: lifeboatbed' "$F/api/v3/user")
[ "$c" = 200 ] && pass "the main edge takes the PAT (200)" || fail "the main edge answered $c to the PAT"
c=$(lb -o /dev/null -w '%{http_code}' -H @<(printf 'authorization: token %s\n' "$PAT") -H 'user-agent: lifeboatbed' "$L/api/v3/user")
[ "$c" = 401 ] && pass "the lifeboat refuses the main-host PAT (401)" || fail "the lifeboat answered $c to the PAT"
c=$(lb -o /dev/null -w '%{http_code}' -H @<(printf 'authorization: token %s\n' "$LBT") -H 'user-agent: lifeboatbed' "$L/api/v3/user")
[ "$c" = 200 ] && pass "the lifeboat takes the lifeboat token (200)" || fail "the lifeboat answered $c to its token"
c=$(lb -o /dev/null -w '%{http_code}' -X POST -H @<(printf 'authorization: token %s\n' "$LBT") -H 'user-agent: lifeboatbed' \
  "$L/api/v3/repos/$LOGIN/$REPO/issues" -d '{"title": "no"}')
[ "$c" = 503 ] && pass "a REST write on the lifeboat answers 503" || fail "a REST write answered $c"
c=$(lb -H @<(printf 'authorization: token %s\n' "$LBT") -H 'user-agent: lifeboatbed' -X POST "$L/api/graphql" \
  -d '{"query": "mutation { addComment(input: {subjectId: \"x\", body: \"no\"}) { clientMutationId } }"}' -w ' %{http_code}')
case "$c" in *" 503") pass "a GraphQL mutation on the lifeboat answers 503";; *) fail "a GraphQL mutation answered: $c";; esac
c=$(lb -o /dev/null -w '%{http_code}' -X POST -H 'user-agent: git/2.50' "$L/$LOGIN/$REPO.git/git-receive-pack")
[ "$c" = 503 ] && pass "git-receive-pack on the lifeboat answers 503" || fail "git-receive-pack answered $c"

echo "== 3. the decoder"
if until_status "s['decoder_at'] > 0" 60; then
  v=$(u -X POST -H 'x-bed-token: lbbed-bed' "$F/_bed/identity" -d '{"chain": "release", "read": {}}')
  python3 - "$v" "$BOAT" "$REL" <<'PY' && pass "the Release group names the lifeboat's release, seen within the run: no 7-day alarm" || fail "decoder: $v"
import json, sys, time
v, host, rel = json.loads(sys.argv[1]), sys.argv[2], sys.argv[3]
lb = (v.get("value") or {}).get("lifeboats", {}).get(host) or {}
print("  lifeboats:", json.dumps(lb))
sys.exit(0 if lb.get("release") == rel and lb.get("seen") and time.time() * 1000 - lb["seen"] < 600_000 else 1)
PY
else fail "no LifeboatDecoder committed: $(status)"; fi

echo "== 4. the feed stopped: tail and packs from followers"
u "$F/_stats?chain=repo:$RID&reset=1" >/dev/null
for c in $(status | python3 -c 'import json,sys; print(*json.load(sys.stdin)["held"])'); do u "$F/_stats?chain=$c&reset=1" >/dev/null; done
# The leaders as the window starts; a `/log_rows` counted on any of them, or on whoever leads at the end, is a leader read.
LEADERS=$(for c in $(status | python3 -c 'import json,sys; print(*json.load(sys.stdin)["held"])'); do
  u "$F/_stats?chain=$c" | python3 -c 'import json,sys; c=sys.argv[1]; print(*[f"{c}@{m["site"]}" for m in json.load(sys.stdin) if m["role"] == "leader"])' "$c"; done)
u "$F/_feed?stop=1" >/dev/null
node "$here/seed.mjs" push "$F" lbbed-bed "$K" || fail "the third push"
RC=$(commit_of "repo:$RID")
t0=$SECONDS
if until_status "s['held'].get('repo:$RID', 0) >= $RC and s['packs'] >= 3 and s['owed'] == 0" 180; then
  pass "repo:$RID reached $RC and the third pack was kept with the feed stopped ($((SECONDS - t0)) s)"
else fail "the copy did not catch up from the edge: $(status)"; fi
python3 - "$F" "$LEADERS" $(status | python3 -c 'import json,sys; print(*json.load(sys.stdin)["held"])') <<'PY' || ok=0
import json, sys, urllib.request
front, before, chains = sys.argv[1], set(sys.argv[2].split()), sys.argv[3:]
leader_reads, follower_reads = 0, 0
for c in chains:
    for m in json.load(urllib.request.urlopen(f"{front}/_stats?chain={c}")):
        n = m["inbound"].get("/log_rows", 0)
        if m["role"] == "leader" or f"{c}@{m['site']}" in before:
            leader_reads += n
        else:
            follower_reads += n
good = leader_reads == 0 and follower_reads > 0
print(("ok    " if good else "FAIL  ") + f"/log_rows on leaders {leader_reads}, on followers {follower_reads}, over {len(chains)} chains")
sys.exit(0 if good else 1)
PY
echo "  feed: $(u "$F/_feed")"

echo "== 5. the edge blocked: gh against the lifeboat"
u "$F/_edge?block=1" >/dev/null
export GH_CONFIG_DIR=$K/gh GH_HOST=$BOAT:$LP SSL_CERT_FILE=$K/ca.pem GH_NO_UPDATE_NOTIFIER=1 GH_PROMPT_DISABLED=1
unset GH_TOKEN GITHUB_TOKEN GH_ENTERPRISE_TOKEN GITHUB_ENTERPRISE_TOKEN
out=$(/usr/bin/gh pr list -R "$LOGIN/$REPO" 2>&1); rc=$?
echo "$out" | sed 's/^/  gh: /'
[ $rc = 0 ] && echo "$out" | grep -q "lifeboat pr" && pass "enrolled gh pr list reads the pull request from the lifeboat with the edge blocked" \
  || fail "gh pr list (rc $rc)"
h=$(lb -D - -o "$B/meta.json" -H @<(printf 'authorization: token %s\n' "$LBT") -H 'user-agent: lifeboatbed' "$L/api/v3/meta" | tr -d '\r' | grep -i '^x-graftyard-lifeboat-lag:')
echo "  $h; meta: $(python3 -c 'import json,sys; v=json.load(open(sys.argv[1])); print({k: v[k] for k in v if "lifeboat" in k.lower()})' "$B/meta.json")"
[ -n "$h" ] && pass "the lag is published" || fail "no lag header"
echo "  edge: $(u "$F/_edge")"
u "$F/_edge?block=0" >/dev/null; u "$F/_feed?stop=0" >/dev/null

echo "== 6. a planted undecodable row halts its chain"
NAMES=$(status | python3 -c 'import json,sys; print(next(c for c in json.load(sys.stdin)["held"] if c.startswith("names:")))')
H=$(held "$NAMES")
KEY=$(u "$F/_export" | python3 -c 'import json,sys; m, q = json.load(sys.stdin)[-1].split("/")[1:]; print(f"export/{m}/{int(q) + 1000:010}")')
python3 - "$NAMES" "$((H + 1))" "$B/planted.bin" <<'PY'
import hashlib, json, struct, sys
chain, idx, out = sys.argv[1], int(sys.argv[2]), sys.argv[3]
def frame(b): return struct.pack(">I", len(b)) + b + hashlib.sha256(b).digest()
head = json.dumps({"batch": {"minute": 0, "seq": 0, "chains": [[chain, idx]]}, "entries": [[chain, idx, 1]]}).encode()
open(out, "wb").write(frame(head) + frame(b"\xff not a log entry"))
PY
python3 - "$B/planted.bin" "$KEY" <<'PY'
import json, struct, sys
p, key = sys.argv[1], sys.argv[2]
b = open(p, "rb").read()
n = struct.unpack(">I", b[:4])[0]
head = json.loads(b[4:4 + n])
_, minute, seq = key.split("/")
head["batch"]["minute"], head["batch"]["seq"] = int(minute), int(seq)
import hashlib
h = json.dumps(head).encode()
open(p, "wb").write(struct.pack(">I", len(h)) + h + hashlib.sha256(h).digest() + b[4 + n + 32:])
PY
u -X POST --data-binary "@$B/planted.bin" "$F/_put?key=$KEY" >/dev/null
if until_status "'$NAMES' in s['halted']" 60; then
  pass "$NAMES halted at $((H + 1)) and stays at $(held "$NAMES")"
else fail "no halt: $(status)"; fi
grep -q "lifeboat halt $NAMES" "$B/workerd.log" && pass "the halt is logged as an error" || fail "no halt line in workerd's log"
sleep 12
c=$(lb -o /dev/null -w '%{http_code}' -H @<(printf 'authorization: token %s\n' "$LBT") -H 'user-agent: lifeboatbed' "$L/api/v3/repos/$LOGIN/$REPO/pulls/1")
[ "$c" = 200 ] && pass "the other chains still serve (pulls/1 200)" || fail "pulls/1 answered $c after the halt"

n=$(cat "$B/dev.log" "$B/workerd.log" | grep -c 'out of bounds\|RuntimeError'); echo "wasm traps: $n"; [ "$n" = 0 ] || ok=0
grep -E "lifeboat_pass" "$B/workerd.log" | tail -2 | cut -c1-400 | sed 's/^/log: /'
[ $ok = 1 ] && echo PASS || { echo FAIL; exit 1; }
