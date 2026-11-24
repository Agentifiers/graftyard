#!/usr/bin/env bash
# `source-record` in local workerd (`tests/m6/results/m6-wp1.md`, "Bootstrap refusals on cold isolates"): three site
# scripts, the site's own build (workers/site, test-faults), bound to each other's classes as f113bed binds them, with
# a real trust anchor and no GRAFTYARD_GENESIS stand-in, so a member reads the system records from the `Release`
# group as a hosted install's does. The driver:
#   1. creates the `Release` group from a genesis signed by this run's customer key (`graftyard genesis`, `sign`),
#      joining each member with the genesis as `/join`'s `genesis`, and commits the genesis through its leader;
#   2. joins `tokens:7`'s site-0 bootstrap member with the genesis's `Tokens` record, which writes A's registry row;
#   3. restarts workerd, so site 1's isolate is cold: it holds no view and no genesis;
#   4. joins `tokens:7`'s site-1 member with the same record (must join), then `tokens:8`'s site-0 member and, after
#      another restart, its site-1 member with the record's `at` changed (must refuse `notTheSourcesRecord`).
# On 05ea366d step 4's first join is refused: the cold isolate's `system_record` builds the record from A's row,
# whose `at` is 0. Exits 1 unless the same-record join is accepted and the other record refused.
#   bash tests/sourcebed/run.sh            # build, serve, drive, stop
#   OUT=dir bash tests/sourcebed/run.sh    # reuse a site build in dir/out
# Needs worker-build, cargo, openssl and tests/m0's wrangler. Run from the repository root.
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd); root=$(cd "$here/../.." && pwd)
P=${P:-8795}; W=${WRANGLER:-$root/tests/m0/node_modules/.bin/wrangler}
[ -x "$W" ] || { echo "no wrangler at $W (npm ci in tests/m0, or WRANGLER=)"; exit 1; }
B=${OUT:-$(mktemp -d)}; mkdir -p "$B/keys"; cp "$here/driver.mjs" "$B/"
if [ ! -f "$B/out/index.js" ]; then
  (cd "$root/workers/site" && worker-build --release --out-dir "$B/out" --features test-faults) >"$B/build.log" 2>&1 \
    || { tail -20 "$B/build.log"; exit 1; }
fi
G() { (cd "$root" && cargo run -q -p graftyard -- --config "$B/install.toml" "$@"); }
rm -f "$B/install.toml" "$B/genesis.unsigned" "$B/genesis.signed" "$B"/keys/*
openssl genpkey -algorithm ed25519 -out "$B/keys/customer.pem" 2>/dev/null
openssl pkey -in "$B/keys/customer.pem" -pubout -out "$B/keys/customer.pub" 2>/dev/null
openssl genpkey -algorithm ed25519 -out "$B/keys/release.pem" 2>/dev/null
openssl pkey -in "$B/keys/release.pem" -pubout -out "$B/keys/release.pub" 2>/dev/null
G init --account bed --zone sourcebed.example --customer-key "$B/keys/customer.pub" >/dev/null || exit 1
# The three sites and their measured round trips, as `sites add` records them.
python3 - "$B" "$root" <<'PY'
import json, sys
b, root = sys.argv[1], sys.argv[2]
sites = json.load(open(f"{root}/tests/m3/finish/sites4.json"))[:3]
toml = "".join(l + "\n" for l in open(f"{b}/install.toml").read().splitlines() if l not in ("sites = []", "rtt = []"))
for s in sites:
    toml += f'\n[[sites]]\nid = {s["id"]}\nscript = "sourcebed-{s["id"]}"\naccount = "bed"\nregion = "{s["region"]}"\n'
    toml += f'bucket = "sourcebed-{s["id"]}"\nspare = false\n\n[sites.link.binding]\nmember_class = "Member"\n'
for a, p, ms in [(0, 1, 90), (0, 2, 180), (1, 2, 170)]:
    toml += f"\n[[rtt]]\nfrom = {a}\npeer = {p}\nmedian_ms = {ms}\nat = 1\n"
open(f"{b}/install.toml", "w").write(toml)
PY
G genesis --out "$B/genesis.unsigned" --release-key "$B/keys/release.pub" --offline-keys >/dev/null || exit 1
G sign "$B/genesis.unsigned" --out "$B/genesis.signed" --key "$B/keys/customer.pem" >/dev/null || exit 1
python3 - "$B" "$root" <<'PY'
import json, re, sys
b, root = sys.argv[1], sys.argv[2]
toml = open(f"{b}/install.toml").read()
anchor = {k: re.search(rf'^{k}\s*=\s*"([^"]*)"', toml, re.M).group(1) for k in ("install", "customer_key")}
sites = json.load(open(f"{root}/tests/m3/finish/sites4.json"))[:3]
for s in sites:
    s["script"] = f"sourcebed-{s['id']}"
classes = [("MEMBER", "Member"), ("REGISTRY", "Registry"), ("SHARD", "GitShard"), ("HOOKSEQ", "HookSequencer"),
           ("SEARCH", "SearchShard"), ("INBOX", "Inbox"), ("LOGRELAY", "LogRelay"), ("BROKER", "RunnerBroker")]
for s in sites:
    me = s["id"]
    json.dump({
        "name": s["script"], "main": f"{b}/driver.mjs", "compatibility_date": "2026-10-01",
        "vars": {"SITE_ID": str(me), "GRAFTYARD_SITES": json.dumps(sites), "PING_MS": "40",
                 "GRAFTYARD_INSTALL": anchor["install"], "GRAFTYARD_CUSTOMER_KEY": anchor["customer_key"]},
        "durable_objects": {"bindings": [
            {"name": f"{n}_{t['id']}", "class_name": c, **({} if t["id"] == me else {"script_name": t["script"]})}
            for n, c in classes for t in sites]},
        "r2_buckets": [{"binding": f"BUCKET_{t['id']}", "bucket_name": f"sourcebed-{t['id']}"} for t in sites],
        "queues": {"producers": [{"binding": "PRSYNC", "queue": f"sourcebed-q{me}"}]},
        "migrations": [{"tag": "v1", "new_sqlite_classes": [c for _, c in classes]}],
    }, open(f"{b}/wrangler{me}.json", "w"))
PY
DEV=
dev() {
  setsid "$W" dev --port $P -c "$B/wrangler0.json" -c "$B/wrangler1.json" -c "$B/wrangler2.json" --persist-to "$B/state" \
    >>"$B/dev.log" 2>&1 & DEV=$!
  for _ in $(seq 120); do curl -s -m 2 "http://127.0.0.1:$P/nope" >/dev/null 2>&1 && return; sleep 1; done
}
stop() { [ -n "$DEV" ] && { kill -- -"$DEV" 2>/dev/null; wait "$DEV" 2>/dev/null; DEV=; }; }
trap 's=$?; stop; [ -z "${OUT:-}" ] && rm -rf "$B"; exit $s' EXIT
rm -rf "$B/state"; : >"$B/dev.log"
GHEX=$(python3 -c "import sys; print(open(sys.argv[1], 'rb').read().hex())" "$B/genesis.signed")
drive() { python3 "$here/drive.py" "$P" "$GHEX" "$@"; }
dev
drive release || exit 1          # 1 and 2: the Release group, then tokens:7's and tokens:8's members on site 0
stop; sleep 1; dev               # 3: site 1's isolate is cold
drive same; same=$?              # 4: tokens:7 on site 1 with the genesis's record: must join
stop; sleep 1; dev
drive other; other=$?            #    tokens:8 on site 1 with the record's `at` moved: must refuse
n=$(grep -c 'out of bounds\|RuntimeError\|panicked' "$B/dev.log"); echo "wasm traps: $n"
[ $same = 0 ] && [ $other = 0 ] && [ "$n" = 0 ] && echo PASS || { echo FAIL; exit 1; }
