#!/usr/bin/env bash
# The edge's clone-cache hit against a MB-MiB entry (default 300, B0-34's are 263 to 271 MB) in local workerd: seeds it,
# restarts workerd, then reads one hit to its end and prints the status, the headers that matter, the bytes read and
# workerd's peak RSS and the isolate's wasm memory after it. Exits 1 unless the hit is a whole 200 with its headers and
# the isolate's wasm memory stays under 64 MiB. WHOLE=1 builds the hit as main 6c9610bf served it (read whole), which
# fails that line: the hosted 500s were the isolate's 128 MB limit, which local workerd does not enforce. FWD=1 reads the
# hit through `on_first_chunk`, as the shard object forwards a miss for its log line, and also fails unless that
# callback ran exactly once; its body has no content-length.
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd); root=$(cd "$here/../../.." && pwd)
MB=${MB:-300}; P=${P:-8796}; W=$root/tests/m0/node_modules/.bin/wrangler
B=$(mktemp -d); S=$B/state; cp -r "$here/Cargo.toml" "$here/wrangler.toml" "$here/src" "$B/"
sed -i "s#\.\./\.\./\.\./crates#$root/crates#" "$B/Cargo.toml"
cd "$B" && CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$B/target} worker-build --release ${WHOLE:+--features whole} >"$B/build.log" 2>&1 \
  || { tail -20 "$B/build.log"; exit 1; }
dev() { "$W" dev --port $P --persist-to $S >"$B/dev.log" 2>&1 & DEV=$!
  until curl -s http://127.0.0.1:$P/nope 2>/dev/null | grep -q "not found"; do sleep 1; done; }
dev
curl -s "http://127.0.0.1:$P/seed?mb=$MB"
kill $DEV; wait $DEV 2>/dev/null; sleep 2; dev
pid=$(for p in $(pgrep -x workerd); do echo "$(awk '/VmRSS/{print $2}' /proc/$p/status) $p"; done | sort -n | tail -1 | cut -d' ' -f2)
max=0
curl -s -D "$B/h" -o "$B/body" "http://127.0.0.1:$P/hit${FWD:+?fwd=1}" & c=$!
while kill -0 $c 2>/dev/null; do r=$(awk '/VmRSS/{print $2}' /proc/$pid/status 2>/dev/null || echo 0); ((r > max)) && max=$r; sleep 0.05; done
echo "status=$(head -1 "$B/h" | tr -d '\r') $(grep -i -E '^(content-length|x-graft-clone-cache|content-type):' "$B/h" | tr -d '\r' | tr '\n' ' ')"
echo "bytes=$(stat -c %s "$B/body") want=$((MB << 20)) peak_rss_mb=$((max / 1024))"
wasm=$(curl -s "http://127.0.0.1:$P/mem"); echo "wasm_mb=$wasm"
first=$(curl -s "http://127.0.0.1:$P/first"); [ -n "${FWD:-}" ] && echo "first_chunk_callbacks=$first"
ok=1
[ -z "${FWD:-}" ] || [ "$first" = 1 ] || ok=0
# A forwarded body is a stream of unknown length, sent chunked; the miss answers it forwards stream with none anyway.
len() { [ -n "${FWD:-}" ] || grep -qi "^content-length: $((MB << 20))" "$B/h"; }
grep -q "200 OK" "$B/h" && grep -qi "^x-graft-clone-cache: hit" "$B/h" && len \
  && [ "$(stat -c %s "$B/body")" = $((MB << 20)) ] && [ "$wasm" -lt 64 ] && [ $ok = 1 ] || ok=0
kill $DEV; wait $DEV 2>/dev/null; rm -rf "$B"
[ $ok = 1 ] && echo PASS || { echo FAIL; exit 1; }
