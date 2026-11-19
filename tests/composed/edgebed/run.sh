#!/usr/bin/env bash
# edge-compose gate, local half (decisions/DECISIONS.md, `edge-compose`, "Gate"): the edge's composed answer, built in
# local workerd by graftyard_git::host::manifest_answer from a real container's chain packs and compose=only&tail=1
# answer, must be byte-identical to the container's own composed answer, and a client's index-pack --strict and fsck
# must take it with the wanted commit's tree. At three stages: a base alone, two parts and a want past the chain, and
# after a fold. Then the declines: a chain pack missing, a chain pack whose count is not the manifest's, another
# network's manifest. The chain carries a 3 MiB random blob, so each answer spans about 50 side-band packets and R2
# reads each base in several chunks. Exits 1 on any mismatch. KEEP=1 keeps the temp dir and prints its path.
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd); root=$(cd "$here/../../.." && pwd)
P=${P:-8797}; W=$root/tests/m0/node_modules/.bin/wrangler
B=$(mktemp -d); S=$B/state; D=$B/chain-out
cp -r "$here/Cargo.toml" "$here/wrangler.toml" "$here/src" "$B/"
sed -i "s#\.\./\.\./\.\./crates#$root/crates#" "$B/Cargo.toml"
(cd "$root/containers/git/server" && EDGEBED_OUT=$D go test -count=1 -run TestWriteEdgeBedChain . >"$B/go.log" 2>&1) \
  || { tail -20 "$B/go.log"; exit 1; }
cd "$B" && CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$B/target} worker-build --release >"$B/build.log" 2>&1 \
  || { tail -20 "$B/build.log"; exit 1; }
"$W" dev --port $P --persist-to $S >"$B/dev.log" 2>&1 & DEV=$!
trap 'kill $DEV 2>/dev/null; wait $DEV 2>/dev/null; if [ -n "${KEEP:-}" ]; then echo "bed dir $B"; else rm -rf "$B"; fi' EXIT
until curl -s http://127.0.0.1:$P/nope 2>/dev/null | grep -q "not found"; do sleep 1; done
u=http://127.0.0.1:$P
put() { curl -sf -X PUT --data-binary @"$2" "$u/put?key=$1" >/dev/null || { echo "put $1 failed"; exit 1; }; }
for f in "$D"/chain/*.pack; do put "compose/7/$(basename "$f")" "$f"; done
ok=1
check() {   # stage: the edge's answer against the container's, then a client's index-pack and fsck
  local s=$1 want tree got
  put "m/$s" "$D/manifest-$s"
  curl -s -D "$B/h-$s" -o "$B/got-$s" "$u/composed?net=7&key=m/$s"
  if ! cmp -s "$B/got-$s" "$D/composed-$s"; then
    echo "$s: FAIL, $(stat -c %s "$B/got-$s") bytes against the container's $(stat -c %s "$D/composed-$s")"; ok=0; return
  fi
  grep -qi "^content-length: $(stat -c %s "$D/composed-$s")" "$B/h-$s" || { echo "$s: FAIL, no exact content-length"; ok=0; return; }
  want=$(cat "$D/want-$s"); tree=$(cat "$D/tree-$s")
  python3 - "$B/got-$s" "$B/pack-$s" <<'EOF' || { echo "$s: FAIL, not a v2 answer"; ok=0; return; }
import sys
b, pack, inpack = open(sys.argv[1], 'rb').read(), b'', False
while b:
    n = int(b[:4], 16)
    if n < 4:
        b = b[4:]; continue
    p, b = b[4:n], b[n:]
    if inpack and p[:1] == b'\x01': pack += p[1:]
    elif p == b'packfile\n': inpack = True
open(sys.argv[2], 'wb').write(pack)
EOF
  local r=$B/clone-$s; git init -q --bare "$r"; echo "$want" >"$r/shallow"
  git -C "$r" index-pack --strict --stdin <"$B/pack-$s" >/dev/null 2>"$B/ix-$s" || { echo "$s: FAIL, index-pack: $(cat "$B/ix-$s")"; ok=0; return; }
  git -C "$r" update-ref refs/heads/main "$want"
  [ -z "$(git -C "$r" fsck --strict --no-dangling 2>&1)" ] || { echo "$s: FAIL, fsck"; ok=0; return; }
  got=$(git -C "$r" rev-parse "$want^{tree}")
  [ "$got" = "$tree" ] || { echo "$s: FAIL, tree $got, want $tree"; ok=0; return; }
  echo "$s: PASS, $(stat -c %s "$B/got-$s") bytes, byte-identical, index-pack --strict and fsck took it"
}
for s in base parts fold; do check $s; done
declined() {   # what: the edge must decline, answering 404 and no pack
  local c; c=$(curl -s -o /dev/null -w '%{http_code}' "$u/composed?net=$2&key=$3")
  [ "$c" = 404 ] && echo "$1: PASS, declined" || { echo "$1: FAIL, answered $c"; ok=0; }
}
declined "another network's manifest" 8 m/parts
part=$(ls "$D"/chain/part-*.pack | head -1); pk="compose/7/$(basename "$part")"
manifest_of_part=$(grep -l "$(basename "$part")" "$D"/manifest-* | head -1); ms=${manifest_of_part##*-}
python3 -c "
import sys; b = bytearray(open(sys.argv[1], 'rb').read()); b[11] ^= 1; open(sys.argv[2], 'wb').write(b)" "$part" "$B/bad.pack"
put "$pk" "$B/bad.pack"
declined "a chain pack whose count is not the manifest's ($ms)" 7 "m/$ms"
curl -sf "$u/delete?key=$pk" >/dev/null
declined "a chain pack missing ($ms)" 7 "m/$ms"
wasm=$(curl -s "$u/mem"); echo "wasm_mb=$wasm"
[ "$wasm" -lt 32 ] || { echo "wasm memory $wasm MiB: the pack went through the isolate"; ok=0; }
[ $ok = 1 ] && echo PASS || { echo FAIL; exit 1; }
