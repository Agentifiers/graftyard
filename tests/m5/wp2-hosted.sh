#!/usr/bin/env bash
# M5-WP2's hosted rows (BF-43, BF-8, BF-48) on graft-m3c's `Release` group, through `graft-m3c-front` in front of the
# scratch edge `graft-m3c-m5-edge`, whose versions N, N+1, and N+2 were uploaded before (tests/m5/results/m5-wp2.md,
# "Setup"). The `graftyard` CLI signs every entry; the run stands in for `graft-ops`'s executor (M5-WP3), so it deploys
# each split with `wrangler versions deploy` from the orb (the ops path) and commits the executor-signed `IntentStep`
# and `IntentDone`. No request path calls the Cloudflare API: the scripts hold no API token. Needs ~/.graft-m3c
# (install.toml, customer.key, release.key, genesis.signed, bed.token) and the version ids below. Every answer and
# event goes to $OUT (default tests/m5/results/raw/wp2); tests/m5/wp2-report.py reads them.
#
#   bash tests/m5/wp2-hosted.sh setup|bf43|bf8|bf48
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
W=${W:-$HOME/.graft-m3c}
CLI=${CLI:-$ROOT/target/release/graftyard}
OUT=${OUT:-$ROOT/tests/m5/results/raw/wp2}
K=$W/m5wp2   # this run's keys and entries, 0700
FRONT=${FRONT:-https://graft-m3c-front.<workers-subdomain>.workers.dev}
EDGE_URL=${EDGE_URL:-https://graft-m3c-m5-edge.<workers-subdomain>.workers.dev}
SCRIPT=graft-m3c-m5-edge
export RUN
V_N=${V_N:-52f05950-b49c-42b9-a4d2-7d0aafb6c894}
V_N1=${V_N1:-5cfcecc3-da40-43d7-9150-8500323e1a0c}
V_N2=${V_N2:-a02b9c73-7d7a-4bd7-9091-bb9abe7fba03}
REPO=65537 CONTROL=65538
# RUN numbers a run's intent ids and release records, so a rerun on the same group uses fresh ones.
RUN=${RUN:-0}
I0=01M7B0000000000000000${RUN}WP20 I1=01M7B0000000000000000${RUN}WP21 I2=01M7B0000000000000000${RUN}WP22
I3=01M7B0000000000000000${RUN}WP23 I3BAD=01M7B0000000000000000${RUN}WP2B I4=01M7B0000000000000000${RUN}WP24
mkdir -p "$OUT" "$K" && chmod 700 "$K"
C="--config $W/install.toml"
WR() { npx --prefix "$ROOT/tests/m0" wrangler "$@" 2>&1 | grep -v "^npm notice\|Proxy environment"; }
ms() { date +%s%3N; }
log() { echo "$(date -u +%H:%M:%S.%3N) $*" | tee -a "$OUT/hosted.log" >&2; }
event() { echo "{\"t\": $(ms), \"event\": \"$1\"${2:+, $2}}" >> "$OUT/events.jsonl"; log "event $1 ${2:-}"; }
field() { python3 -c 'import json,sys; print(json.loads(sys.argv[1]).get(sys.argv[2]) or "")' "$1" "$2"; }
seed() { [ -f "$1" ] || { python3 -c 'import os; print(os.urandom(32).hex())' > "$1"; chmod 600 "$1"; }; }
pub() { python3 - "$1" <<'EOF'
import sys
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from cryptography.hazmat.primitives import serialization
k = Ed25519PrivateKey.from_private_bytes(bytes.fromhex(open(sys.argv[1]).read().strip()))
print(k.public_key().public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw).hex())
EOF
}
entry() { echo "$1" > "$K/spec.json"; "$CLI" $C release entry "$K/spec.json" --out "$2" "${@:3}" >/dev/null; }
sign() { "$CLI" sign "$1" --out "$2" --key "$3" "${@:4}" >/dev/null; }
prev() { cat "$K/prev"; }
# post <file> <label>: posts through graft-front, appends `<label> <answer>` to answers.jsonl, and moves prev on 201.
post() {
  local a t0
  t0=$(ms)
  a=$("$CLI" $C release post "$1" --edge "$FRONT")
  echo "{\"label\": \"$2\", \"t0\": $t0, \"t1\": $(ms), \"answer\": $a}" >> "$OUT/answers.jsonl"
  log "$2 -> $a"
  [ "$(field "$a" status)" = 201 ] && field "$a" digest > "$K/prev"
  echo "$a"
}
# signed <spec> <name> <key> <label>: builds on the current prev, signs, posts.
signed() { entry "$1" "$K/$2.unsigned" --prev "$(prev)" "${@:5}"; sign "$K/$2.unsigned" "$K/$2.signed" "$3"; post "$K/$2.signed" "$4"; }
must() { [ "$(field "$1" status)" = "$2" ] || { log "expected $2: $1"; exit 1; }; }
# deploy <spec...>: the ops path's split, and the deployment list after it.
deploy() {
  local t0; t0=$(ms)
  WR versions deploy "$@" --name $SCRIPT -y >> "$OUT/wrangler.log"
  event deployed "\"split\": \"$*\", \"t0\": $t0"
}
deployments() { WR deployments list --name $SCRIPT --json > "$OUT/deployments-$1.json"; log "deployments $1: $(python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); x=d[-1] if isinstance(d,list) else d; print(x.get("id"), x.get("created_on"), [(v["version_id"][:8], v["percentage"]) for v in x["versions"]])' "$OUT/deployments-$1.json")"; }
# fault <query>: the bed's fault plan, tried up to 5 times (BF-8 run 3's driver exited on one failed call to clear it).
fault() {
  local i a
  for i in 1 2 3 4 5; do
    a=$(curl -sf -m 20 -X POST -H "x-bed-token: $(cat "$W/bed.token")" "$EDGE_URL/_bed/fault?$1") && break
    log "fault $1: try $i failed"; sleep 2
  done
  echo "${a:0:400}" >> "$OUT/faults.log"
}
sampler() { # <name> <args...>: starts tests/m5/wp2-sample.mjs until $OUT/stop-<name> exists
  rm -f "$OUT/stop-$1"
  (cd "$ROOT/tests/am1/hosted" && source ../lf13/env-m3c.sh && node "$ROOT/tests/m5/wp2-sample.mjs" "${@:2}" "$OUT/$1.jsonl" "$OUT/stop-$1" >> "$OUT/sampler.log" 2>&1 &)
}
stop() { touch "$OUT/stop-$1"; }
undo_of() { # <intent> <pin rid> <name>: the intent's pre-signed undo, re-signed by the same key (Ed25519 is deterministic, so the
  # signature is the one the intent carries, which decide checks byte for byte).
  entry "{\"Undo\": {\"intent\": \"$1\", \"entry\": {\"EdgePointer\": {\"pin\": \"$2\"}}}}" "$K/$3.unsigned"
  sign "$K/$3.unsigned" "$K/$3.signed" "$W/customer.key"
}
roll() { # <intent id> <release> <undo pin>: an EdgeRoll the customer signs, its undo pre-signed by the customer key
  echo "{\"Intent\": {\"id\": \"$1\", \"intent\": {\"EdgeRoll\": {\"release\": \"$2\"}}, \"undo\": [{\"EdgePointer\": {\"pin\": \"$3\"}}], \"undo_key\": \"$W/customer.key\"}}"
}
rid() { cat "$K/rid-$1"; }

setup() {
  # The executor and drain keys of m3c's genesis were discarded, so a customer-signed Keys at the next generation names
  # two this run holds; the release-signing key is m3c's own.
  seed "$K/executor.seed"; seed "$K/drain.seed"
  entry '{"SiteLost": {"site": 0}}' "$K/probe.unsigned" --prev 0000000000000000000000000000000000000000000000000000000000000000
  sign "$K/probe.unsigned" "$K/probe.signed" "$W/customer.key"
  local a; a=$(post "$K/probe.signed" probe); must "$a" 409
  field "$a" prev > "$K/prev"
  if [ ! -f "$K/keys1.done" ]; then
    entry "{\"Keys\": {\"executor\": \"$(pub "$K/executor.seed")\", \"drain\": \"$(pub "$K/drain.seed")\"}}" "$K/keys1.unsigned" \
      --prev "$(prev)" --keys "$W/genesis.signed"
    sign "$K/keys1.unsigned" "$K/keys1.signed" "$W/customer.key"
    must "$(post "$K/keys1.signed" keys-gen1)" 201
    touch "$K/keys1.done"
  fi
  for t in N N1 N2; do
    local v; v=$(eval echo \$V_$t)
    python3 - "$K" "$t" <<'EOF'
import hashlib, json, os, sys
k, t = sys.argv[1], sys.argv[2]
reads = "".join(f"{f}\n" for f in ["32.1", "32.4", "32.5", "32.6", "32.7"])
rec = {"label": f"m5-wp2-edge-{t}-run{os.environ.get('RUN', '0')}", "files": [{"name": "formats.txt", "sha256": hashlib.sha256(reads.encode()).hexdigest()}]}
open(f"{k}/release-{t}.json", "wb").write(json.dumps(rec, sort_keys=True).encode())
EOF
    openssl pkeyutl -sign -rawin -inkey "$W/release.key" -in "$K/release-$t.json" -out "$K/release-$t.sig"
    sha256sum "$K/release-$t.json" | cut -c1-64 > "$K/rid-$t"
    local sig; sig=$(od -An -tx1 -v "$K/release-$t.sig" | tr -d " \n")
    must "$(signed "{\"ReleaseUploaded\": {\"record\": \"$K/release-$t.json\", \"record_sig\": \"$sig\",
      \"versions\": [[\"Edge\", \"$v\"]], \"reads\": [\"32.1\", \"32.4\", \"32.5\", \"32.6\", \"32.7\"]}}" "upload-$t" "$K/executor.seed" "upload-$t")" 201
    log "release $t $(rid $t) -> edge $v"
  done
  # The group records the edge as running the release the last roll left; a roll to N with no undo, done at once
  # (N already serves 100%), makes N the release this run's rolls keep.
  must "$(signed "{\"Intent\": {\"id\": \"$I0\", \"intent\": {\"EdgeRoll\": {\"release\": \"$(rid N)\"}}}}" i0 "$W/customer.key" roll-to-n)" 201
  must "$(signed "{\"IntentDone\": {\"intent\": \"$I0\"}}" i0-done "$K/executor.seed" roll-to-n-done)" 201
}

# BF-43: EdgePointer undo under a 1% EdgeRoll with the API blocked, then the next EdgeRoll.
bf43() {
  sampler bf43 front "$FRONT" /api/v3 8
  sleep 45
  must "$(signed "$(roll $I1 "$(rid N1)" "$(rid N)")" i1 "$W/customer.key" bf43-roll-n1)" 201
  event roll1
  deploy "$V_N@99" "$V_N1@1"
  must "$(signed "{\"IntentStep\": {\"intent\": \"$I1\", \"step\": 0}}" i1-step0 "$K/executor.seed" bf43-step0)" 201
  sleep 90
  deployments before-undo
  undo_of $I1 "$(rid N)" i1-undo
  # The API blocked: the undo and the minute after it run with no Cloudflare credential in this shell.
  (
    export CLOUDFLARE_API_TOKEN=blocked CLOUDFLARE_SITE_DEPLOY_TOKEN=blocked
    event api-blocked
    WR deployments list --name $SCRIPT > "$OUT/api-blocked-probe.log" 2>&1 && { log "the API answered while blocked"; exit 1; }
    must "$(post "$K/i1-undo.signed" bf43-undo)" 201
    event undo
    sleep 60
    event api-unblocked
  )
  deployments after-undo
  # graft-ops's undo: N at 100% once the API answers, then IntentDone.
  deploy "$V_N@100"
  must "$(signed "{\"IntentDone\": {\"intent\": \"$I1\"}}" i1-done "$K/executor.seed" bf43-done)" 201
  sleep 20
  must "$(signed "$(roll $I2 "$(rid N2)" "$(rid N)")" i2 "$W/customer.key" bf43-roll-n2)" 201
  event roll2
  deploy "$V_N@99" "$V_N2@1"
  must "$(signed "{\"IntentStep\": {\"intent\": \"$I2\", \"step\": 0}}" i2-step0 "$K/executor.seed" bf43-roll2-step0)" 201
  sleep 90
  stop bf43
  event bf43-end
}

# BF-8: (A) every Release member blackholed for 10 minutes while the edge is pinned to N under a split of N@99 and
# N+2@1: requests keep the pin and none waits on the group. (B) Then the EdgeRoll whose undo names a version its
# deployment drops is refused (DropsUndoVersion); an accepted EdgeRoll to N+1 runs at 1%, a deploy outside any intent
# drops N (N+1@100, what the refused roll's deployment would have done), and the roll's undo sets EdgePointer to N,
# outside the deployment: graft-front alarms on every pinned request until N is back.
bf8() {
  sampler bf8 front "$FRONT" /api/v3 8
  sampler bf8-repo front "$FRONT" /api/v3/repos/m3org/lf13-g 1
  sleep 45
  undo_of $I2 "$(rid N)" i2-undo
  must "$(post "$K/i2-undo.signed" bf8-pin-n)" 201
  event pin
  sleep 45
  fault "site=255&chains=release&hang_ms=10000&for_ms=600000"
  event blackhole
  sleep 600
  fault "off=1"
  event blackhole-off
  sleep 30
  bf8b
}

# BF-8 (B), after the blackhole: also run alone after a (A) whose driver stopped there.
bf8b() {
  deploy "$V_N@100"
  must "$(signed "{\"IntentDone\": {\"intent\": \"$I2\"}}" i2-done "$K/executor.seed" bf8-i2-done)" 201
  local a; a=$(signed "$(roll $I3BAD "$(rid N1)" "$(rid N2)")" i3bad "$W/customer.key" bf8-roll-drops-undo)
  must "$a" 422
  event refused-roll
  must "$(signed "$(roll $I3 "$(rid N1)" "$(rid N)")" i3 "$W/customer.key" bf8-roll-n1)" 201
  event roll3
  deploy "$V_N@99" "$V_N1@1"
  must "$(signed "{\"IntentStep\": {\"intent\": \"$I3\", \"step\": 0}}" i3-step0 "$K/executor.seed" bf8-i3-step0)" 201
  sleep 30
  (cd "$ROOT" && npx --prefix tests/m0 wrangler tail graft-m3c-front --format json > "$OUT/front-tail.jsonl" 2>/dev/null &)
  deploy "$V_N1@100"
  event dropped-n
  deployments dropped-n
  sleep 20
  undo_of $I3 "$(rid N)" i3-undo
  must "$(post "$K/i3-undo.signed" bf8-pin-outside)" 201
  event pin-outside
  sleep 60
  deploy "$V_N@100"
  event restored-n
  sleep 30
  pkill -f "wrangler tail graft-m3c-front" || true
  must "$(signed "{\"IntentDone\": {\"intent\": \"$I3\"}}" i3-done "$K/executor.seed" bf8-i3-done)" 201
  # The run's last roll clears the pin; nothing deploys it, and it is done at once.
  must "$(signed "$(roll $I4 "$(rid N1)" "$(rid N)")" i4 "$W/customer.key" bf8-roll-clear)" 201
  must "$(signed "{\"IntentDone\": {\"intent\": \"$I4\"}}" i4-done "$K/executor.seed" bf8-i4-done)" 201
  sleep 15
  stop bf8; stop bf8-repo
  event bf8-end
}

# BF-48: the Release group blackholed for its own Raft calls (it can neither commit nor lead), then two customer-signed
# StaleModes on repository 65537: one whose `until` comes first (100 s), one that STALE_MAX (120 s on this install)
# ends before its `until` (900 s). A write probe goes to every member while each flag is honored.
bf48() {
  sampler bf48 member "repo:$REPO" 500
  sampler bf48-control member "repo:$CONTROL" 2000
  sleep 30
  fault "site=255&chains=release&group_only=1&hang_ms=1000&for_ms=1200000"
  event blackhole
  sleep 20
  local a
  entry "{\"StaleMode\": {\"scope\": {\"Repo\": $REPO}, \"until_s\": 100}}" "$K/stale-a.unsigned" --prev "$(prev)"
  sign "$K/stale-a.unsigned" "$K/stale-a.signed" "$W/customer.key"
  a=$(post "$K/stale-a.signed" bf48-stale-a); must "$a" 202
  event stale-a
  sleep 15; write_probe a
  sleep 120
  entry "{\"StaleMode\": {\"scope\": {\"Repo\": $REPO}, \"until_s\": 900}}" "$K/stale-b.unsigned" --prev "$(prev)"
  sign "$K/stale-b.unsigned" "$K/stale-b.signed" "$W/customer.key"
  a=$(post "$K/stale-b.signed" bf48-stale-b); must "$a" 202
  event stale-b
  sleep 15; write_probe b
  sleep 170
  fault "off=1"
  event blackhole-off
  sleep 20
  stop bf48; stop bf48-control
  event bf48-end
}
write_probe() {
  (cd "$ROOT/tests/am1/hosted" && source ../lf13/env-m3c.sh && node --input-type=module -e '
    import { member } from "./lib.mjs";
    for (const s of [0, 1, 2]) console.log(JSON.stringify({ flag: "'$1'", t: Date.now(), site: s, commit: (await member("repo:'$REPO'", s, "/commit", {}, 5000)).v }));
  ') >> "$OUT/bf48-commit.jsonl"
}

"$1"
