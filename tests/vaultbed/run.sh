#!/usr/bin/env bash
# The vault Worker end to end in local workerd, at $0. Four builds run as one install and one vault account:
# exportbed's three site scripts (workers/site, test-faults, `tests/exportbed/driver.mjs` in front of site 0) write the
# change feed; the edge's own build (workers/edge, test-faults) serves the export endpoints from them, with the
# install's anchor and a real `jti` row; and the vault's own build (workers/vault) pulls through `front.mjs`, which
# stands in for the internet between the accounts, under an install-scoped token from `graftyard vault token`.
#
# Before the feed's first round the bed also plants one object of a repository chain whose two entries name stored
# packs, with the packs and their `Committed` markers in site 0's bucket: one pack's bytes are the entry's (9 MiB, two
# of the vault's upload parts), the other's are not. The round folds it in as an object written before its state.
#
# Passes only if the vault's pass stores every `export/` object byte for byte under `feed/`, the leader-led chain's
# cursor at its commit, the good pack with the entry's SHA-256, never the bad one (counted missed), and a later round's
# object on its next pass; then, after `DELETE export/token` and READ_CACHE, a pass fails with the 401 named and stores
# nothing new; and workerd logs no wasm trap. Exits 1 otherwise.
# Needs worker-build, tests/m0's wrangler, and openssl. PREBUILT=<dir> takes site/, edge/, and vault/ builds from
# <dir> instead (each `worker-build --release --out-dir <dir>/<name>`, site and edge with test-faults). Run from the
# repository root.
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd); root=$(cd "$here/../.." && pwd)
P=${P:-8796}; W=${WRANGLER:-$root/tests/m0/node_modules/.bin/wrangler}
[ -x "$W" ] || { echo "no wrangler at $W (npm ci in tests/m0, or WRANGLER=)"; exit 1; }
B=$(mktemp -d); mkdir -p "$B/fx" "$B/keys"
cp "$here/front.mjs" "$B/"; cp "$root/tests/exportbed/driver.mjs" "$B/"
build() { (cd "$root/workers/$1" && worker-build --release --out-dir "$B/$1" "${@:2}") >"$B/build-$1.log" 2>&1 \
  || { tail -20 "$B/build-$1.log"; exit 1; }; }
if [ -n "${PREBUILT:-}" ]; then
  cp -r "$PREBUILT/site" "$PREBUILT/edge" "$PREBUILT/vault" "$B/"
else
  build site --features test-faults; build edge --features test-faults; build vault
fi
ln -s "$B/site" "$B/out"   # driver.mjs imports ./out/index.js
cd "$root" || exit 1
# The install: a customer key, install.toml, and an install-scoped export token, as the operator makes them.
openssl genpkey -algorithm ed25519 -out "$B/keys/customer.pem" 2>/dev/null
openssl pkey -in "$B/keys/customer.pem" -pubout -out "$B/keys/customer.pub" 2>/dev/null
cargo run -q -p graftyard -- --config "$B/install.toml" init --account bed --zone vaultbed.example \
  --customer-key "$B/keys/customer.pub" >/dev/null || exit 1
cargo run -q -p graftyard -- --config "$B/install.toml" vault token --key "$B/keys/customer.pem" --scope install \
  --holder vault --days 2 --out "$B/keys/export.token" >/dev/null || exit 1
cargo run -q -p graftyard-vault --example vaultbed -- "$B/fx" $(( $(date +%s) / 60 - 5 )) || exit 1
python3 - "$B" "$root" "$P" <<'PY'
import json, re, sys
b, root, port = sys.argv[1], sys.argv[2], sys.argv[3]
toml = open(f"{b}/install.toml").read()
anchor = {k: re.search(rf'^{k}\s*=\s*"([^"]*)"', toml, re.M).group(1) for k in ("install", "customer_key")}
sites = json.load(open(f"{root}/tests/m3/finish/sites4.json"))[:3]
for s in sites:
    s["script"], s["bucket"] = f"vaultbed-{s['id']}", f"vaultbed-{s['id']}"
classes = [("MEMBER", "Member"), ("REGISTRY", "Registry"), ("SHARD", "GitShard"), ("HOOKSEQ", "HookSequencer"),
           ("SEARCH", "SearchShard"), ("INBOX", "Inbox"), ("LOGRELAY", "LogRelay"), ("BROKER", "RunnerBroker")]
buckets = [{"binding": f"BUCKET_{s['id']}", "bucket_name": s["bucket"]} for s in sites]
export = {"binding": "EXPORT", "bucket_name": "vaultbed-export"}
genesis = json.load(open(f"{root}/tests/m1/wp7/genesis.json"))["placements"]
# The site scripts, as tests/exportbed/run.sh builds them, with the test install's genesis (`deploy.mjs --genesis`)
# so the edge's `jti` reads reach a Tokens shard.
for s in sites:
    me = s["id"]
    json.dump({
        "name": s["script"], "main": f"{b}/driver.mjs", "compatibility_date": "2026-10-01",
        "vars": {"SITE_ID": str(me), "GRAFTYARD_SITES": json.dumps(sites), "GRAFTYARD_EXPORT": "1",
                 "GRAFTYARD_GENESIS": json.dumps(genesis)},
        "durable_objects": {"bindings": [
            {"name": f"{n}_{t['id']}", "class_name": c, **({} if t["id"] == me else {"script_name": t["script"]})}
            for n, c in classes for t in sites]},
        "r2_buckets": buckets + ([export] if me == 0 else []),
        "queues": {"producers": [{"binding": "PRSYNC", "queue": f"vaultbed-q{me}"}],
                   "consumers": [{"queue": f"vaultbed-q{me}", "max_batch_size": 10, "max_batch_timeout": 0}]},
        "migrations": [{"tag": "v1", "new_sqlite_classes": [c for _, c in classes]}],
    }, open(f"{b}/site{me}.json", "w"))
# The edge, as tests/m1/integ/deploy-edge.mjs binds it: every site's classes by script, every site's bucket, the
# export bucket, the test install's genesis, this install's anchor, and Tokens 0x11.2 (17.2) so `DELETE export/token`
# can commit its `JtiRevoked`.
json.dump({"name": "vaultbed-edge", "main": f"{b}/edge/index.js", "compatibility_date": "2026-10-01",
           "vars": {"PLAN": "pro", "SSH_OPTION": "off", "EDGE_CAP_BYTES": "80000000", "FORMATS_ENABLED": "32.1,17.2",
                    "PART_BYTES": "5242880", "HOST": "vaultbed.example", "GRAFTYARD_SITES": json.dumps(sites),
                    "GRAFTYARD_GENESIS": json.dumps(genesis), "LOGINS": "{}",
                    "GRAFTYARD_INSTALL": anchor["install"], "GRAFTYARD_CUSTOMER_KEY": anchor["customer_key"]},
           "durable_objects": {"bindings": [{"name": f"{n}_{t['id']}", "class_name": c, "script_name": t["script"]}
                                            for n, c in classes[:3] for t in sites]},
           "r2_buckets": buckets + [export]},
          open(f"{b}/edge/wrangler.json", "w"))
json.dump({"name": "vaultbed-vault", "main": f"{b}/vault/index.js", "compatibility_date": "2026-10-01",
           "vars": {"GRAFTYARD_URL": f"http://127.0.0.1:{port}"},
           "r2_buckets": [{"binding": "VAULT", "bucket_name": "vaultbed-vault"}]},
          open(f"{b}/vault/wrangler.json", "w"))
json.dump({"name": "vaultbed-front", "main": f"{b}/front.mjs", "compatibility_date": "2026-10-01",
           "services": [{"binding": "EDGE", "service": "vaultbed-edge"}, {"binding": "VAULT", "service": "vaultbed-vault"},
                        {"binding": "SITE", "service": "vaultbed-0"}],
           "r2_buckets": [{"binding": "SITE0", "bucket_name": "vaultbed-0"}, export,
                          {"binding": "VAULTB", "bucket_name": "vaultbed-vault"}]},
          open(f"{b}/front.json", "w"))
PY
# The token reaches the vault as its secret, as `graftyard vault init` writes it, and nowhere else.
printf 'EXPORT_TOKEN=%s\n' "$(cat "$B/keys/export.token")" >"$B/vault/.dev.vars"
printf 'BED_TOKEN=vaultbed-bed\n' >"$B/edge/.dev.vars"
setsid "$W" dev --port $P -c "$B/front.json" -c "$B/edge/wrangler.json" -c "$B/vault/wrangler.json" -c "$B/site0.json" \
  -c "$B/site1.json" -c "$B/site2.json" --persist-to "$B/state" >"$B/dev.log" 2>&1 & DEV=$!
trap 's=$?; [ -n "${KEEP_LOG:-}" ] && cp "$B/dev.log" "$KEEP_LOG"; kill -- -$DEV 2>/dev/null; wait $DEV 2>/dev/null; rm -rf "$B"; exit $s' EXIT
for _ in $(seq 120); do grep -q "Ready on" "$B/dev.log" && break; sleep 1; done
F="http://127.0.0.1:$P"
u() { curl -s -m 120 "$@"; }
plant() { u -X POST --data-binary "@$1" "$F/_put?b=$2&key=$3${4:+&meta=$4}" -o /dev/null; }
read -r OBJ GOODK GOODM GOODMETA BADK BADM BADMETA CHAIN < <(python3 - "$B/fx/plant.json" <<'PY'
import json, sys, urllib.parse
p = json.load(open(sys.argv[1]))
m = lambda x: urllib.parse.quote(json.dumps(x["meta"]))
print(p["object"], p["good"]["pack"], p["good"]["marker"], m(p["good"]), p["bad"]["pack"], p["bad"]["marker"], m(p["bad"]),
      p["chain"])
PY
)
: >"$B/empty"
plant "$B/fx/object.bin" EXPORT "$OBJ"
plant "$B/fx/good.pack" SITE0 "$GOODK"; plant "$B/empty" SITE0 "$GOODM" "$GOODMETA"
plant "$B/fx/bad.pack" SITE0 "$BADK"; plant "$B/empty" SITE0 "$BADM" "$BADMETA"
ok=1
# The feed: a leader-led chain commits and reports; the cron's round writes its object.
d=$(u "$F/_site/drive?chain=repo:900001"); echo "drive: $d"
grep -q '"put":200' <<<"$d" || { echo "FAIL: the put failed"; ok=0; }
sleep 5; u "$F/_sitecron?cron=*/15+*+*+*+*" >/dev/null; sleep 5
check() { # $1: label; compares the vault's feed/ with the install's export/ and checks the planted chain's packs.
  python3 - "$B" "$1" "$(u "$F/_export")" "$(u "$F/_vault")" <<'PY'
import json, sys
b, label, exp, v = sys.argv[1], sys.argv[2], json.loads(sys.argv[3]), json.loads(sys.argv[4])
p = json.load(open(f"{b}/fx/plant.json"))
keys = {o["key"]: o for o in v["objects"]}
ok = True
def check(cond, what):
    global ok
    print(f"{label}: " + ("ok   " if cond else "FAIL ") + what)
    ok &= bool(cond)
feed = {"export/" + k[len("feed/"):]: o["sha256"] for k, o in keys.items() if k.startswith("feed/")}
check(len(exp) >= 2 and feed == {o["key"]: o["sha256"] for o in exp},
      f"every export/ object ({len(exp)}) is under feed/, byte for byte ({len(feed)})")
check(keys.get(p["good"]["pack"], {}).get("sha256") == p["good"]["meta"]["checksum"],
      "the good pack is stored with the entry's SHA-256, through two upload parts")
check(p["bad"]["pack"] not in keys, "the bad pack is never stored")
st = v.get("state") or {}
check(st.get("missed") == 1, f"one copy counted missed (got {st.get('missed')})")
cur = st.get("feed", {}).get("cursors", {})
check(cur.get(p["chain"]) == 2 and cur.get("repo:900001", 0) >= 2, f"cursors {cur}")
check(all(k.startswith(("feed/", "entries/", "snapshot/", "pack/", "state/")) for k in keys),
      "every key is under a locked prefix or state/")
open(f"{b}/vault-{label}.json", "w").write(json.dumps(v))
sys.exit(0 if ok else 1)
PY
}
r=$(u "$F/_cron"); echo "vault cron 1: $r"
check pass1 || ok=0
# A later commit: the next round's object reaches the vault on its next pass.
d=$(u "$F/_site/drive?chain=repo:900002"); sleep 5; u "$F/_sitecron?cron=*/15+*+*+*+*" >/dev/null; sleep 5
r=$(u "$F/_cron"); echo "vault cron 2: $r"
check pass2 || ok=0
# A pass from an older state (none): it writes every object again, each create-only put is refused, and the pass
# takes the same objects as stored and moves on. Local R2 has no locks, so the refusal here is the precondition's; the
# hosted row repeats this under the lock, where it is a 409. Every stored object carries its SHA-256 as metadata. The
# bad pack is owed again once its record is gone, so it is fetched and counted missed again; it is still not stored.
u "$F/_dropstate" >/dev/null
r=$(u "$F/_cron"); echo "vault cron rerun: $r"
u "$F/_vault" >"$B/vault-rerun.json"
python3 - "$B/vault-pass2.json" "$B/vault-rerun.json" "$F" <<'PY' || ok=0
import json, sys, urllib.parse, urllib.request
a, b, front = json.load(open(sys.argv[1])), json.load(open(sys.argv[2])), sys.argv[3]
was = {o["key"]: o["sha256"] for o in a["objects"] if not o["key"].startswith("state/")}
now = {o["key"]: o["sha256"] for o in b["objects"] if not o["key"].startswith("state/")}
same = was == now and b["state"] is not None and b["state"]["feed"] == a["state"]["feed"]
meta = [json.load(urllib.request.urlopen(f"{front}/_meta?key={urllib.parse.quote(k)}")) for k in was]
tagged = all(m and m["sha256"] == was[k] for k, m in zip(was, meta))
print("rerun: " + ("ok   " if same and tagged else "FAIL ") + f"a pass from no state stored nothing new ({len(now)} objects) and every object carries its sha256 ({tagged})")
sys.exit(0 if same and tagged else 1)
PY
grep -q "holds another object\|vault pass failed" "$B/dev.log" && { echo "FAIL: the rerun pass failed"; ok=0; }
# Revocation: the token revokes itself through the edge; after READ_CACHE the vault's pass is refused, names the 401,
# and stores nothing more, though a new object waits in the feed.
rv=$(u -o /dev/null -w '%{http_code}' -X DELETE -H "authorization: Bearer $(cat "$B/keys/export.token")" \
  -H 'user-agent: graftyard-vaultbed' "$F/api/graftyard/v1/export/token"); echo "revoke: $rv"
[ "$rv" = 204 ] || { echo "FAIL: DELETE export/token answered $rv"; ok=0; }
d=$(u "$F/_site/drive?chain=repo:900003"); sleep 5; u "$F/_sitecron?cron=*/15+*+*+*+*" >/dev/null; sleep 6
r=$(u "$F/_cron"); echo "vault cron 3: $r"
grep -q "vault pass failed: .*401" "$B/dev.log" || { echo "FAIL: no pass refused with the 401 named"; ok=0; }
python3 - "$B/vault-rerun.json" "$(u "$F/_vault")" "$(u "$F/_export")" <<'PY' || ok=0
import json, sys
a, b, exp = json.load(open(sys.argv[1])), json.loads(sys.argv[2]), json.loads(sys.argv[3])
same = [o["key"] for o in a["objects"]] == [o["key"] for o in b["objects"]] and a["state"] == b["state"]
waiting = len(exp) > sum(1 for o in a["objects"] if o["key"].startswith("feed/"))
print("revoked: " + ("ok   " if same and waiting else "FAIL ") + f"nothing stored and the state unmoved, with a new object waiting ({waiting})")
sys.exit(0 if same and waiting else 1)
PY
grep -E "vault pass|vault: " "$B/dev.log" | sed 's/^/log: /' | head -8
n=$(grep -c 'out of bounds\|RuntimeError' "$B/dev.log"); echo "wasm traps: $n"; [ "$n" = 0 ] || ok=0
[ $ok = 1 ] && echo PASS || { echo FAIL; exit 1; }
