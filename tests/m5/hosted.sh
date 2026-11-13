#!/usr/bin/env bash
# M5-WP1's hosted rows on graft-m4a, through the product edge's `POST /api/graftyard/v1/release/entries`, with the
# `graftyard` CLI built from this branch. Needs ~/.graft-m4a (install.toml, customer.key, release.key,
# genesis.signed) and EDGE, the edge's base URL. Writes every answer to $OUT (default tests/m5/results/raw).
#
#   EDGE=https://graft-m4a-m5-edge.<workers-subdomain>.workers.dev bash tests/m5/hosted.sh
#
# BF-41 runs on an empty Release group of its own: `bf41` starts one on a throwaway install id (BF41_EDGE, an edge
# whose anchor names that install), so the m4a group, which holds its genesis, is not the empty group.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
W=${W:-$HOME/.graft-m4a}
CLI=${CLI:-$ROOT/target/release/graftyard}
OUT=${OUT:-$ROOT/tests/m5/results/raw}
K=$W/m5   # this run's keys and entries, 0600
mkdir -p "$OUT" "$K" && chmod 700 "$K"
C="--config $W/install.toml"
log() { echo "$(date -u +%H:%M:%S) $*" | tee -a "$OUT/hosted.log" >&2; }

# post <file> <label>: posts and appends `<label> <answer>` to the log, echoes the answer.
post() {
  local a
  a=$("$CLI" $C release post "$1" --edge "$EDGE")
  echo "$2 $a" >> "$OUT/answers.jsonl"
  log "$2 -> $a"
  echo "$a"
}
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
# entry <spec json> <out> [extra args]
entry() { echo "$1" > "$K/spec.json"; "$CLI" $C release entry "$K/spec.json" --out "$2" "${@:3}" >/dev/null; }
sign() { "$CLI" sign "$1" --out "$2" --key "$3" "${@:4}" >/dev/null; }

# The group's current digest: a probe the group refuses with 409 (a genesis replay names the genesis; anything stale
# names the last). Each step below binds the digest the previous 201 returned, so this is used once, at the start.
current() {
  entry '{"SiteLost": {"site": 0}}' "$K/probe.unsigned" --prev 0000000000000000000000000000000000000000000000000000000000000000
  sign "$K/probe.unsigned" "$K/probe.signed" "$W/customer.key"
  local a
  a=$(post "$K/probe.signed" probe)
  [ "$(field "$a" status)" = 409 ] || { log "probe not 409: $a"; exit 1; }
  field "$a" prev
}

setup() {
  # The executor and drain keys of m4a's genesis were discarded (--offline-keys), so a customer-signed Keys at the
  # next generation names two this run holds. The release-signing key is m4a's own.
  seed "$K/executor.seed"; seed "$K/drain.seed"
  PREV=$(current)
  log "start: prev $PREV"
  entry "{\"Keys\": {\"executor\": \"$(pub "$K/executor.seed")\", \"drain\": \"$(pub "$K/drain.seed")\"}}" "$K/keys1.unsigned" \
    --prev "$PREV" --keys "$W/genesis.signed"
  sign "$K/keys1.unsigned" "$K/keys1.signed" "$W/customer.key"
  local a; a=$(post "$K/keys1.signed" keys-gen1)
  [ "$(field "$a" status)" = 201 ] || { log "Keys refused: $a"; exit 1; }
  PREV=$(field "$a" digest)
  # A release record, signed by m4a's release key, with a formats file, uploaded by the executor key.
  python3 - "$K" <<'EOF'
import hashlib, json, sys
k = sys.argv[1]
reads = "".join(f"{f}\n" for f in ["32.1", "32.4", "32.5", "32.6", "32.7"])
rec = {"label": "m5-wp1-hosted", "files": [{"name": "formats.txt", "sha256": hashlib.sha256(reads.encode()).hexdigest()}]}
open(f"{k}/release.json", "wb").write(json.dumps(rec, sort_keys=True).encode())
EOF
  openssl pkeyutl -sign -rawin -inkey "$W/release.key" -in "$K/release.json" -out "$K/release.sig"
  RID=$(sha256sum "$K/release.json" | cut -c1-64)
  log "release $RID"
}

upload_spec() { # <record_sig hex>
  echo "{\"ReleaseUploaded\": {\"record\": \"$K/release.json\", \"record_sig\": \"$1\",
    \"versions\": [[\"Edge\", \"0e6a1b2c-0000-4000-8000-000000000001\"], [\"Site0\", \"0e6a1b2c-0000-4000-8000-000000000002\"]],
    \"reads\": [\"32.1\", \"32.4\", \"32.5\", \"32.6\", \"32.7\"]}}"
}

# BF-45: a forged ReleaseUploaded, a ReplaceMember of an undrained site, and a pointer, each signed by the executor
# key; then a correctly signed ReleaseUploaded.
bf45() {
  local good bad a
  good=$(od -An -tx1 -v "$K/release.sig" | tr -d " \n")
  bad=$(python3 -c 'import sys; b=bytearray(bytes.fromhex(sys.argv[1])); b[5]^=1; print(b.hex())' "$good")
  entry "$(upload_spec "$bad")" "$K/forged.unsigned" --prev "$PREV"
  sign "$K/forged.unsigned" "$K/forged.signed" "$K/executor.seed"
  post "$K/forged.signed" bf45-forged-upload >/dev/null
  entry '{"Intent": {"intent": {"ReplaceMember": {"lost": 1, "spare": 3}}}}' "$K/replace.unsigned" --prev "$PREV"
  sign "$K/replace.unsigned" "$K/replace.signed" "$K/executor.seed"
  post "$K/replace.signed" bf45-replace-undrained >/dev/null
  entry "{\"EdgePointer\": {\"pin\": \"$RID\"}}" "$K/pin.unsigned" --prev "$PREV"
  sign "$K/pin.unsigned" "$K/pin.signed" "$K/executor.seed"
  post "$K/pin.signed" bf45-pointer-forward >/dev/null
  entry "{\"Undo\": {\"intent\": \"00000000000000000000000001\", \"entry\": {\"EdgePointer\": {\"pin\": \"$RID\"}}}}" "$K/pinu.unsigned"
  sign "$K/pinu.unsigned" "$K/pinu.signed" "$K/executor.seed"
  post "$K/pinu.signed" bf45-pointer-undo >/dev/null
  entry "$(upload_spec "$good")" "$K/upload.unsigned" --prev "$PREV"
  sign "$K/upload.unsigned" "$K/upload.signed" "$K/executor.seed"
  a=$(post "$K/upload.signed" bf45-upload)
  [ "$(field "$a" status)" = 201 ] || { log "upload refused: $a"; exit 1; }
  PREV=$(field "$a" digest)
}

# BF-40: an intent naming a release with no ReleaseUploaded, then a second intent while one is current.
bf40() {
  local a
  entry '{"Intent": {"intent": {"EdgeRoll": {"release": "'"$(printf 'ab%.0s' {1..32})"'"}}}}' "$K/noupload.unsigned" --prev "$PREV"
  sign "$K/noupload.unsigned" "$K/noupload.signed" "$W/customer.key"
  post "$K/noupload.signed" bf40-no-upload >/dev/null
  entry "{\"Intent\": {\"id\": \"01M7A00000000000000000BF40\", \"intent\": {\"SiteDay\": {\"target\": {\"Site\": 1}, \"release\": \"$RID\"}}}}" \
    "$K/intent1.unsigned" --prev "$PREV"
  sign "$K/intent1.unsigned" "$K/intent1.signed" "$W/customer.key"
  a=$(post "$K/intent1.signed" bf40-first-intent)
  [ "$(field "$a" status)" = 201 ] || { log "first intent refused: $a"; exit 1; }
  PREV=$(field "$a" digest)
  entry "{\"Intent\": {\"intent\": {\"EdgeRoll\": {\"release\": \"$RID\"}}}}" "$K/intent2.unsigned" --prev "$PREV"
  sign "$K/intent2.unsigned" "$K/intent2.signed" "$W/customer.key"
  post "$K/intent2.signed" bf40-second-intent >/dev/null
}

# BF-42: the customer signs a Halt offline over digest d; the executor commits an IntentStep (d -> d'); the Halt meets
# 409 naming d'; signed again over d', it commits.
bf42() {
  local a stale
  entry '{"Halt": {"intent": "01M7A00000000000000000BF40"}}' "$K/halt.unsigned" --prev "$PREV"
  sign "$K/halt.unsigned" "$K/halt.signed" "$W/customer.key"
  entry '{"IntentStep": {"intent": "01M7A00000000000000000BF40", "step": 0}}' "$K/step0.unsigned" --prev "$PREV"
  sign "$K/step0.unsigned" "$K/step0.signed" "$K/executor.seed"
  a=$(post "$K/step0.signed" bf42-step)
  [ "$(field "$a" status)" = 201 ] || { log "step refused: $a"; exit 1; }
  PREV=$(field "$a" digest)
  a=$(post "$K/halt.signed" bf42-stale-halt)
  stale=$(field "$a" prev)
  [ "$(field "$a" status)" = 409 ] && [ "$stale" = "$PREV" ] && log "bf42: 409 names the current digest $PREV"
  sign "$K/halt.signed" "$K/halt2.signed" "$W/customer.key" --prev "$stale"
  a=$(post "$K/halt2.signed" bf42-resigned-halt)
  PREV=$(field "$a" digest)
  # Leave the group with nothing current: the customer aborts to the uploaded release, and the executor finishes it.
  entry "{\"Abort\": {\"intent\": \"01M7A00000000000000000BF40\", \"target\": \"$RID\"}}" "$K/abort.unsigned" --prev "$PREV"
  sign "$K/abort.unsigned" "$K/abort.signed" "$W/customer.key"
  a=$(post "$K/abort.signed" cleanup-abort); PREV=$(field "$a" digest)
  entry '{"IntentDone": {"intent": "01M7A00000000000000000BF40"}}' "$K/done.unsigned" --prev "$PREV"
  sign "$K/done.unsigned" "$K/done.signed" "$K/executor.seed"
  a=$(post "$K/done.signed" cleanup-done); PREV=$(field "$a" digest)
}

: > "$OUT/answers.jsonl"
setup
bf45
bf40
bf42
log "end: prev $PREV"
